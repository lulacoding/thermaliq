use super::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use thermaliq_common::{FanKind, MetricKind, SensorKind};

const FAN: &str = "lhm_nct6687d_0_fan_5";
const PUMP: &str = "lhm_nct6687d_0_fan_1";

#[derive(Default)]
struct Shared {
    requests: Mutex<Vec<Value>>,
    scripted: Mutex<VecDeque<Result<Value, String>>>,
    hang_on: Mutex<Option<String>>,
    starts: AtomicUsize,
    stops: AtomicUsize,
    fail_start: AtomicBool,
    die_next: AtomicBool,
    bad_id_next: AtomicBool,
}

impl Shared {
    fn ops(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r["op"].as_str().unwrap().to_owned())
            .collect()
    }

    fn sets(&self) -> Vec<(String, u64)> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "set")
            .map(|r| {
                (
                    r["fan"].as_str().unwrap().to_owned(),
                    r["duty"].as_u64().unwrap(),
                )
            })
            .collect()
    }
}

struct FakeConnection(Arc<Shared>);

impl BridgeConnection for FakeConnection {
    fn exchange(&mut self, line: &str) -> std::io::Result<String> {
        let request: Value = serde_json::from_str(line).unwrap();
        let op = request["op"].as_str().unwrap().to_owned();
        self.0.requests.lock().unwrap().push(request.clone());
        if self.0.die_next.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("pipe broke"));
        }
        if self.0.hang_on.lock().unwrap().as_deref() == Some(op.as_str()) {
            thread::sleep(Duration::from_secs(60));
        }
        let id = if self.0.bad_id_next.swap(false, Ordering::SeqCst) {
            json!(999_999)
        } else {
            request["id"].clone()
        };
        let mut reply = match op.as_str() {
            "hello" => json!({
                "ok": true, "protocol": 1, "lhm": "0.9.7-pre742",
                "controllable": [FAN, PUMP], "refused_chips": [],
            }),
            "snapshot" => match self.0.scripted.lock().unwrap().pop_front() {
                Some(Ok(v)) => v,
                Some(Err(e)) => json!({"ok": false, "error": e}),
                None => snapshot_json(),
            },
            _ => match self.0.scripted.lock().unwrap().pop_front() {
                Some(Ok(v)) => v,
                Some(Err(e)) => json!({"ok": false, "error": e}),
                None => json!({"ok": true}),
            },
        };
        reply["id"] = id;
        Ok(reply.to_string())
    }
}

struct FakeStarter(Arc<Shared>);

impl BridgeStarter for FakeStarter {
    fn start(&mut self) -> Result<Box<dyn BridgeConnection>, String> {
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_start.load(Ordering::SeqCst) {
            return Err("no bridge".into());
        }
        Ok(Box::new(FakeConnection(self.0.clone())))
    }

    fn stop(&mut self) {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
    }
}

fn snapshot_json() -> Value {
    // The shape `bridges/lhm-bridge` sends (its Protocol.cs and README.md).
    json!({
        "ok": true,
        "sensors": [
            {"id": "lhm_amdcpu_0_temperature_2", "label": "Core (Tctl/Tdie)", "kind": "cpu", "temp_celsius": 54.5},
            {"id": "lhm_nct6687d_0_temperature_1", "label": "System", "kind": "other", "temp_celsius": 37.0},
        ],
        "fans": [
            {"id": FAN, "label": "System Fan #4", "kind": "fan", "rpm": 850.4, "duty_percent": 59.6, "controllable": true},
            {"id": PUMP, "label": "Pump Fan", "kind": "pump", "rpm": 2100.0, "duty_percent": 100.0, "controllable": true},
        ],
        "metrics": [
            {"id": "lhm_amdcpu_0_load_0", "kind": "cpu_load", "value": 12.5},
            {"id": "lhm_amdcpu_0_power_0", "kind": "cpu_power", "value": 45.0},
        ],
        "degraded": {},
    })
}

fn timing() -> Timing {
    Timing {
        call: Duration::from_millis(200),
        heartbeat: Duration::from_millis(20),
        retry: Duration::from_millis(20),
    }
}

fn make() -> (Arc<Shared>, LhmBridge) {
    let shared = Arc::new(Shared::default());
    let bridge = LhmBridge::with_timing(Box::new(FakeStarter(shared.clone())), timing()).unwrap();
    (shared, bridge)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting: {what}");
}

#[test]
fn start_says_hello_and_lists_the_controllable_fans() {
    let (shared, b) = make();
    assert_eq!(shared.ops()[0], "hello");
    assert_eq!(b.capabilities().controllable_fans, [FAN, PUMP]);
}

#[test]
fn start_fails_when_the_bridge_cannot_start_so_fans_are_absent() {
    let shared = Arc::new(Shared::default());
    shared.fail_start.store(true, Ordering::SeqCst);
    assert!(LhmBridge::with_timing(Box::new(FakeStarter(shared)), timing()).is_err());
}

#[test]
fn start_refuses_another_protocol_version() {
    let reply = parse_hello(&json!({"ok": true, "protocol": 2, "controllable": []}));
    assert!(reply.is_err());
    let good = parse_hello(&json!({"ok": true, "protocol": 1, "controllable": [FAN]}));
    assert_eq!(good.unwrap(), [FAN]);
}

#[test]
fn snapshot_maps_sensors_fans_and_metrics() {
    let (_, b) = make();
    let s = b.snapshot().unwrap();
    assert_eq!(s.sensors[0].kind, SensorKind::Cpu);
    assert_eq!(s.sensors[0].temp_celsius, 54.5);
    assert_eq!(s.sensors[1].kind, SensorKind::Other);
    assert_eq!(s.fans[0].id, FAN);
    assert_eq!(s.fans[0].rpm, 850);
    assert_eq!(s.fans[0].duty_percent, 60);
    assert_eq!(s.fans[0].kind, FanKind::Fan);
    assert_eq!(s.fans[1].kind, FanKind::Pump);
    assert_eq!(s.metrics[0].kind, MetricKind::CpuLoad);
    assert_eq!(s.metrics[0].value, 12.5);
    assert_eq!(s.metrics[1].kind, MetricKind::CpuPower);
}

#[test]
fn a_missing_or_invalid_reading_is_skipped_never_zero() {
    let (shared, b) = make();
    shared.scripted.lock().unwrap().push_back(Ok(json!({
        "ok": true,
        "sensors": [{"id": "s", "label": "s", "kind": "gpu", "temp_celsius": 40.0}],
        "fans": [
            {"id": "no_rpm", "label": "a", "kind": "fan", "rpm": null, "duty_percent": 50.0, "controllable": true},
            {"id": "no_duty", "label": "b", "kind": "fan", "rpm": 800.0, "duty_percent": null, "controllable": false},
            {"id": "bad_duty", "label": "c", "kind": "fan", "rpm": 800.0, "duty_percent": 180.0, "controllable": true},
            {"id": "bad_rpm", "label": "d", "kind": "fan", "rpm": -5.0, "duty_percent": 50.0, "controllable": true},
            {"id": "ok", "label": "e", "kind": "fan", "rpm": 0.0, "duty_percent": 0.0, "controllable": true},
        ],
        "metrics": [
            {"id": "m1", "kind": "cpu_load", "value": 140.0},
            {"id": "m2", "kind": "gpu_load", "value": 10.0},
            {"id": "m3", "kind": "nope", "value": 10.0},
        ],
        "degraded": {},
    })));
    let s = b.snapshot().unwrap();
    assert_eq!(s.fans.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["ok"]);
    assert!(s.metrics.iter().all(|m| m.id == "m2"), "{:?}", s.metrics);
    // A bridge can't report a GPU core: only its own CPU sensor may trip S2's CPU rule.
    assert_eq!(s.sensors[0].kind, SensorKind::Other);
}

#[test]
fn a_set_reaches_the_bridge_and_a_refusal_comes_back() {
    let (shared, b) = make();
    b.set_fan_duty(FAN, 60).unwrap();
    assert_eq!(shared.sets(), [(FAN.to_owned(), 60)]);
    shared
        .scripted
        .lock()
        .unwrap()
        .push_back(Err("pump may not go below 20%".into()));
    assert!(b.set_fan_duty(PUMP, 0).unwrap_err().0.contains("pump"));
}

#[test]
fn a_fan_the_bridge_did_not_list_never_reaches_it() {
    let (shared, b) = make();
    assert!(b.set_fan_duty("other", 50).is_err());
    assert!(b.restore_auto("other").is_err());
    assert!(!shared.ops().iter().any(|o| o == "set" || o == "restore"));
}

#[test]
fn a_duty_over_100_never_reaches_the_bridge() {
    let (shared, b) = make();
    assert!(b.set_fan_duty(FAN, 101).is_err());
    assert!(shared.sets().is_empty());
}

#[test]
fn heartbeats_flow_with_no_other_calls() {
    let (shared, _b) = make();
    wait_until("3 heartbeats", || {
        shared.ops().iter().filter(|o| *o == "heartbeat").count() >= 3
    });
}

#[test]
fn a_wedged_call_times_out_instead_of_blocking() {
    let (shared, b) = make();
    *shared.hang_on.lock().unwrap() = Some("set".into());
    let began = std::time::Instant::now();
    let err = b.set_fan_duty(FAN, 60).unwrap_err();
    assert!(began.elapsed() < Duration::from_secs(5), "{err}");
    assert!(err.0.contains("timed out"), "{err}");
}

#[test]
fn the_default_call_timeout_is_two_seconds_and_the_heartbeat_one() {
    assert_eq!(Timing::default().call, Duration::from_secs(2));
    assert_eq!(Timing::default().heartbeat, Duration::from_secs(1));
}

#[test]
fn a_reply_with_the_wrong_id_is_an_error_and_drops_the_connection() {
    let (shared, b) = make();
    shared.bad_id_next.store(true, Ordering::SeqCst);
    assert!(b.set_fan_duty(FAN, 60).is_err());
    wait_until("restart", || shared.starts.load(Ordering::SeqCst) >= 2);
}

#[test]
fn a_lost_connection_fails_calls_then_restarts_the_bridge() {
    let (shared, b) = make();
    shared.die_next.store(true, Ordering::SeqCst);
    assert!(b.snapshot().is_err());
    wait_until("restart", || shared.starts.load(Ordering::SeqCst) >= 2);
    wait_until("back up", || b.snapshot().is_ok());
    assert!(shared.stops.load(Ordering::SeqCst) >= 1);
    // A restarted bridge is asked hello again.
    assert!(shared.ops().iter().filter(|o| *o == "hello").count() >= 2);
}

#[test]
fn degraded_headers_from_the_bridge_are_reported() {
    let (shared, b) = make();
    let mut with = snapshot_json();
    with["degraded"] = json!({FAN: "still manual after hand-back"});
    shared.scripted.lock().unwrap().push_back(Ok(with));
    b.snapshot().unwrap();
    let reasons = b.degraded_reasons();
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains(FAN) && reasons[0].contains("still manual"));
    b.snapshot().unwrap();
    assert!(b.degraded_reasons().is_empty());
}

#[test]
fn a_failed_restore_stays_degraded_until_a_later_one_verifies() {
    let (shared, b) = make();
    shared
        .scripted
        .lock()
        .unwrap()
        .push_back(Err("still manual".into()));
    assert!(b.restore_auto(FAN).is_err());
    assert_eq!(shared.ops().last().unwrap(), "restore");
}

#[test]
fn suspend_hands_back_stops_the_bridge_and_does_not_restart_it() {
    let (shared, b) = make();
    b.suspend().unwrap();
    assert!(shared.ops().contains(&"shutdown".to_owned()));
    assert_eq!(shared.stops.load(Ordering::SeqCst), 1);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(shared.starts.load(Ordering::SeqCst), 1);
    assert!(b.snapshot().is_err());
}

#[test]
fn a_failed_hand_back_on_suspend_is_reported_but_the_bridge_still_stops() {
    let (shared, b) = make();
    shared
        .scripted
        .lock()
        .unwrap()
        .push_back(Err("1 fan(s) did not hand back".into()));
    assert!(b.suspend().is_err());
    assert_eq!(shared.stops.load(Ordering::SeqCst), 1);
}

#[test]
fn while_suspended_writes_fail_and_restore_is_done_because_the_bios_has_the_fan() {
    let (shared, b) = make();
    b.suspend().unwrap();
    assert!(b.set_fan_duty(FAN, 60).unwrap_err().0.contains("anti-cheat"));
    b.restore_auto(FAN).unwrap();
    assert!(shared.sets().is_empty());
}

#[test]
fn resume_starts_the_bridge_again() {
    let (shared, b) = make();
    b.suspend().unwrap();
    b.resume();
    wait_until("restart", || shared.starts.load(Ordering::SeqCst) == 2);
    wait_until("snapshot", || b.snapshot().is_ok());
    b.set_fan_duty(FAN, 60).unwrap();
}

#[test]
fn dropping_the_backend_shuts_the_bridge_down() {
    let (shared, b) = make();
    drop(b);
    wait_until("shutdown", || shared.ops().contains(&"shutdown".to_owned()));
    wait_until("stopped", || shared.stops.load(Ordering::SeqCst) == 1);
}

#[test]
fn a_garbled_reply_is_an_error() {
    assert!(parse_reply("not json", 1).is_err());
    assert!(parse_reply(r#"{"id":1}"#, 1).is_err());
    assert!(parse_reply(r#"{"id":1,"ok":false}"#, 1).is_err());
    assert!(parse_reply(r#"{"id":2,"ok":true}"#, 1).is_err());
    assert!(parse_reply(r#"{"id":1,"ok":true}"#, 1).is_ok());
}
