//! Motherboard fans through `bridges/lhm-bridge` (SPEC §5.1, M12). The protocol is in that
//! folder's README. The daemon supplies how to start the bridge, because the path and ACL checks
//! and the pipe live there; this file holds everything that can be tested without a bridge.

use crate::{Capabilities, HalError, HardwareLayer};
use serde_json::{json, Value};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thermaliq_common::{
    FanKind, FanReading, HardwareSnapshot, MetricKind, MetricReading, SensorKind, SensorReading,
};

const PROTOCOL: u64 = 1;
/// Starting the bridge opens LHM, which can be slow on a cold start. Not a per-tick call.
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// One connected bridge. A blocking request and its reply, as JSON lines.
pub trait BridgeConnection: Send {
    fn exchange(&mut self, line: &str) -> io::Result<String>;
}

pub trait BridgeStarter: Send + 'static {
    /// Starts a bridge and connects to it. An error means motherboard fans are absent for now.
    fn start(&mut self) -> Result<Box<dyn BridgeConnection>, String>;
    /// The connection is finished with. The bridge exits by itself when its client goes.
    fn stop(&mut self);
    /// Kills a bridge that stopped answering, and says whether it has gone. Its record of the
    /// originals stays, so the next bridge hands the fans back. `start` must refuse to run a
    /// second bridge while this one is alive: both would write to one chip.
    fn kill(&mut self) -> bool;
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub call: Duration,
    pub heartbeat: Duration,
    pub retry: Duration,
    /// How long one request may go unanswered before the bridge counts as hung.
    pub hang: Duration,
    /// `shutdown` restores every header in turn, so it gets longer than a single request.
    pub shutdown: Duration,
}

impl Default for Timing {
    /// SPEC §5.1: every call answers within 2 s, and a heartbeat goes out at least every 1 s.
    fn default() -> Self {
        Self {
            call: Duration::from_secs(2),
            heartbeat: Duration::from_secs(1),
            retry: Duration::from_secs(5),
            hang: Duration::from_secs(5),
            shutdown: Duration::from_secs(6),
        }
    }
}

enum Msg {
    Call(Value, Sender<Result<Value, String>>),
    Suspend(Sender<Result<(), String>>),
    Resume,
}

/// The bridge runs on one worker thread, so callers give up at their deadline whatever the pipe
/// does. The bridge's own watchdog can't cover a request stuck inside LHM or PawnIO, because both
/// hold one lock, so the worker kills a bridge that stops answering (S6).
pub struct LhmBridge {
    worker: Sender<Msg>,
    timing: Timing,
    controllable: Vec<String>,
    /// In anti-cheat mode: the bridge is stopped on purpose and the BIOS has the fans.
    suspended: AtomicBool,
    degraded: Mutex<Vec<String>>,
    /// Why the worker killed the bridge, until a new one says hello.
    killed: Arc<Mutex<Option<String>>>,
}

impl LhmBridge {
    pub fn new(starter: Box<dyn BridgeStarter>) -> Result<Self, HalError> {
        Self::with_timing(starter, Timing::default())
    }

    pub fn with_timing(starter: Box<dyn BridgeStarter>, timing: Timing) -> Result<Self, HalError> {
        let (worker, jobs) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let killed = Arc::new(Mutex::new(None));
        let reason = killed.clone();
        thread::Builder::new()
            .name("lhm-bridge".into())
            .spawn(move || Worker::new(starter, timing, reason).run(&jobs, &ready))
            .map_err(|e| HalError(format!("cannot start the bridge thread: {e}")))?;
        let controllable = started
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| HalError("the bridge did not start in time".into()))?
            .map_err(HalError)?;
        Ok(Self {
            worker,
            timing,
            controllable,
            suspended: AtomicBool::new(false),
            degraded: Mutex::new(Vec::new()),
            killed,
        })
    }

    /// Anti-cheat mode: hands every fan back and stops the bridge. The bridge stays stopped until
    /// `resume`, even if the hand-back reports a failure, which comes back as the error.
    pub fn suspend(&self) -> Result<(), String> {
        self.suspended.store(true, Ordering::SeqCst);
        let (reply, answer) = mpsc::channel();
        self.worker
            .send(Msg::Suspend(reply))
            .map_err(|_| "the bridge thread has stopped".to_owned())?;
        answer
            .recv_timeout(self.timing.shutdown + self.timing.call)
            .map_err(|_| "the bridge did not stop in time".to_owned())?
    }

    pub fn resume(&self) {
        self.suspended.store(false, Ordering::SeqCst);
        let _ = self.worker.send(Msg::Resume);
    }

    fn call(&self, request: Value) -> Result<Value, HalError> {
        let (reply, answer) = mpsc::channel();
        self.worker
            .send(Msg::Call(request, reply))
            .map_err(|_| HalError("the bridge thread has stopped".into()))?;
        match answer.recv_timeout(self.timing.call) {
            Ok(result) => result.map_err(HalError),
            Err(RecvTimeoutError::Timeout) => Err(HalError(format!(
                "bridge call timed out after {:?}",
                self.timing.call
            ))),
            Err(RecvTimeoutError::Disconnected) => {
                Err(HalError("the bridge thread dropped the call".into()))
            }
        }
    }

    fn check_known(&self, fan: &str) -> Result<(), HalError> {
        if self.controllable.iter().any(|f| f == fan) {
            Ok(())
        } else {
            Err(HalError(format!("the bridge does not control fan '{fan}'")))
        }
    }

    fn is_suspended(&self) -> bool {
        self.suspended.load(Ordering::SeqCst)
    }
}

impl HardwareLayer for LhmBridge {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        if self.is_suspended() {
            return Err(HalError("anti-cheat mode: the bridge is stopped".into()));
        }
        let reply = self.call(json!({"op": "snapshot"}))?;
        let reasons = degraded_of(&reply);
        *self.degraded.lock().unwrap_or_else(|e| e.into_inner()) = reasons;
        Ok(parse_snapshot(&reply))
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        self.check_known(fan)?;
        if duty > 100 {
            return Err(HalError(format!("duty {duty} is outside 0-100")));
        }
        if self.is_suspended() {
            return Err(HalError(format!(
                "anti-cheat mode: the BIOS has fan '{fan}'"
            )));
        }
        self.call(json!({"op": "set", "fan": fan, "duty": duty}))
            .map(drop)
    }

    fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
        self.check_known(fan)?;
        if self.is_suspended() {
            return Ok(()); // suspending already handed it back, or reported why not
        }
        self.call(json!({"op": "restore", "fan": fan})).map(drop)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            controllable_fans: self.controllable.clone(),
        }
    }

    fn degraded_reasons(&self) -> Vec<String> {
        let mut reasons = self
            .degraded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        reasons.extend(
            self.killed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        );
        reasons
    }
}

/// A connection on its own thread, so a request that never returns can't stop the worker from
/// killing the bridge. The thread ends when this is dropped, or when a kill breaks the pipe.
struct Link {
    lines: Sender<String>,
    replies: Receiver<io::Result<String>>,
}

impl Link {
    fn new(mut connection: Box<dyn BridgeConnection>) -> Result<Self, String> {
        let (lines, requests) = mpsc::channel::<String>();
        let (answers, replies) = mpsc::channel();
        thread::Builder::new()
            .name("lhm-bridge-pipe".into())
            .spawn(move || {
                for line in requests {
                    if answers.send(connection.exchange(&line)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| format!("cannot start the pipe thread: {e}"))?;
        Ok(Self { lines, replies })
    }

    fn exchange(&self, line: String, limit: Duration) -> Result<String, Reply> {
        self.lines
            .send(line)
            .map_err(|_| Reply::Broken("the pipe thread has stopped".into()))?;
        match self.replies.recv_timeout(limit) {
            Ok(reply) => reply.map_err(|e| Reply::Broken(e.to_string())),
            Err(RecvTimeoutError::Timeout) => Err(Reply::Hung),
            Err(RecvTimeoutError::Disconnected) => {
                Err(Reply::Broken("the pipe thread has stopped".into()))
            }
        }
    }
}

struct Worker {
    starter: Box<dyn BridgeStarter>,
    timing: Timing,
    connection: Option<Link>,
    next_id: u64,
    suspended: bool,
    last_try: Instant,
    killed: Arc<Mutex<Option<String>>>,
}

impl Worker {
    fn new(
        starter: Box<dyn BridgeStarter>,
        timing: Timing,
        killed: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            starter,
            timing,
            connection: None,
            next_id: 1,
            suspended: false,
            last_try: Instant::now(),
            killed,
        }
    }

    fn run(mut self, jobs: &mpsc::Receiver<Msg>, ready: &Sender<Result<Vec<String>, String>>) {
        let controllable = self.connect();
        let failed = controllable.is_err();
        let _ = ready.send(controllable);
        if failed {
            return;
        }
        loop {
            if self.connection.is_none()
                && !self.suspended
                && self.last_try.elapsed() >= self.timing.retry
            {
                if let Err(e) = self.connect() {
                    eprintln!("lhm bridge did not restart: {e}");
                }
            }
            let wait = if self.connection.is_some() {
                self.timing.heartbeat
            } else {
                self.timing.retry
            };
            match jobs.recv_timeout(wait) {
                Ok(Msg::Call(request, reply)) => {
                    let _ = reply.send(self.request(request));
                }
                Ok(Msg::Suspend(reply)) => {
                    self.suspended = true;
                    let _ = reply.send(self.shut_down());
                }
                Ok(Msg::Resume) => {
                    self.suspended = false;
                    self.last_try = Instant::now()
                        .checked_sub(self.timing.retry)
                        .unwrap_or_else(Instant::now);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.connection.is_some() {
                        let _ = self.request(json!({"op": "heartbeat"}));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = self.shut_down();
                    return;
                }
            }
        }
    }

    fn connect(&mut self) -> Result<Vec<String>, String> {
        self.last_try = Instant::now();
        self.connection = Some(Link::new(self.starter.start()?)?);
        let hello = self
            .request(json!({"op": "hello", "protocol": PROTOCOL}))
            .and_then(|reply| parse_hello(&reply));
        if hello.is_err() && self.connection.is_some() {
            self.connection = None;
            self.starter.stop();
        }
        if hello.is_ok() {
            *self.killed.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        hello
    }

    /// A refusal keeps the connection. A broken pipe or a garbled reply drops it, because the
    /// next reply might then answer the wrong request. No answer at all gets the bridge killed.
    fn request(&mut self, mut request: Value) -> Result<Value, String> {
        let Some(link) = self.connection.as_ref() else {
            return Err("the bridge is down".into());
        };
        let op = request["op"].as_str().unwrap_or("?").to_owned();
        let limit = match op.as_str() {
            "hello" => START_TIMEOUT, // a new bridge opens LHM and hands back before it answers
            "shutdown" => self.timing.shutdown,
            _ => self.timing.hang,
        };
        let id = self.next_id;
        self.next_id += 1;
        request["id"] = json!(id);
        let outcome = link
            .exchange(request.to_string(), limit)
            .and_then(|line| parse_reply(&line, id));
        match outcome {
            Ok(value) => Ok(value),
            Err(Reply::Refused(e)) => Err(e),
            Err(Reply::Broken(e)) => {
                self.connection = None;
                self.starter.stop();
                self.last_try = Instant::now();
                Err(format!("bridge connection lost: {e}"))
            }
            Err(Reply::Hung) => {
                self.kill_hung(&op);
                Err(format!("the bridge hung on '{op}'"))
            }
        }
    }

    fn kill_hung(&mut self, op: &str) {
        self.connection = None;
        let reason = if self.starter.kill() {
            format!("lhm-bridge: hung on '{op}', so it was killed. A new one hands its fans back")
        } else {
            format!("lhm-bridge: hung on '{op}' and can't be killed, so its fans may be stuck until a reboot")
        };
        eprintln!("{reason}");
        *self.killed.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
        self.last_try = Instant::now();
    }

    fn shut_down(&mut self) -> Result<(), String> {
        if self.connection.is_none() {
            return Err("the bridge was already down, so its hand-back is unconfirmed".into());
        }
        let result = self.request(json!({"op": "shutdown"})).map(drop);
        if self.connection.take().is_some() {
            self.starter.stop();
        }
        result
    }
}

#[derive(Debug)]
enum Reply {
    Refused(String),
    Broken(String),
    Hung,
}

fn parse_reply(line: &str, id: u64) -> Result<Value, Reply> {
    let reply: Value =
        serde_json::from_str(line).map_err(|e| Reply::Broken(format!("bad reply: {e}")))?;
    if reply["id"].as_u64() != Some(id) {
        return Err(Reply::Broken(format!(
            "reply for {} to request {id}",
            reply["id"]
        )));
    }
    match reply["ok"].as_bool() {
        Some(true) => Ok(reply),
        Some(false) => Err(Reply::Refused(
            reply["error"]
                .as_str()
                .unwrap_or("the bridge refused, with no reason")
                .to_owned(),
        )),
        None => Err(Reply::Broken("reply has no ok field".into())),
    }
}

fn parse_hello(reply: &Value) -> Result<Vec<String>, String> {
    if reply["protocol"].as_u64() != Some(PROTOCOL) {
        return Err(format!(
            "bridge speaks protocol {}, expected {PROTOCOL}",
            reply["protocol"]
        ));
    }
    let fans = reply["controllable"]
        .as_array()
        .ok_or("hello has no controllable list")?;
    Ok(fans
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}

fn degraded_of(reply: &Value) -> Vec<String> {
    reply["degraded"].as_object().map_or_else(Vec::new, |map| {
        map.iter()
            .map(|(fan, why)| format!("{fan}: {}", why.as_str().unwrap_or("unknown")))
            .collect()
    })
}

/// Anything missing or impossible is left out, never reported as 0 (a stopped fan or idle PC).
fn parse_snapshot(reply: &Value) -> HardwareSnapshot {
    let items = |name: &str| reply[name].as_array().map_or(&[][..], Vec::as_slice);
    let text = |v: &Value, name: &str| v[name].as_str().map(str::to_owned);
    HardwareSnapshot {
        sensors: items("sensors")
            .iter()
            .filter_map(|s| {
                Some(SensorReading {
                    id: text(s, "id")?,
                    label: text(s, "label")?,
                    // Never Gpu: the bridge doesn't read GPUs, and this drives S2.
                    kind: if s["kind"] == "cpu" {
                        SensorKind::Cpu
                    } else {
                        SensorKind::Other
                    },
                    #[allow(clippy::cast_possible_truncation)]
                    temp_celsius: s["temp_celsius"].as_f64()? as f32,
                })
            })
            .collect(),
        fans: items("fans")
            .iter()
            .filter_map(|f| {
                let rpm = f["rpm"]
                    .as_f64()
                    .filter(|r| (0.0..=f64::from(u32::MAX)).contains(r))?;
                let duty = f["duty_percent"]
                    .as_f64()
                    .filter(|d| (0.0..=100.0).contains(d))?;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(FanReading {
                    id: text(f, "id")?,
                    label: text(f, "label")?,
                    rpm: rpm.round() as u32,
                    duty_percent: duty.round() as u8,
                    kind: if f["kind"] == "pump" {
                        FanKind::Pump
                    } else {
                        FanKind::Fan
                    },
                })
            })
            .collect(),
        metrics: items("metrics")
            .iter()
            .filter_map(|m| {
                let kind: MetricKind = serde_json::from_value(m["kind"].clone()).ok()?;
                let value = m["value"].as_f64().filter(|v| kind.is_valid(*v))?;
                Some(MetricReading {
                    id: text(m, "id")?,
                    kind,
                    value,
                })
            })
            .collect(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "lhm_tests.rs"]
mod tests;
