#![allow(clippy::unwrap_used)]
use std::time::Duration;
use thermaliq_common::Control;
use thermaliq_engine::{FanControl, Source};

fn ctl(up: u8, down: u8, min: u8, zero: Option<(u8, u8)>) -> FanControl {
    FanControl::new(&Control {
        fan: "f".into(),
        curve: "c".into(),
        step_up_pct: up,
        step_down_pct: down,
        min_pwm: min,
        start_percent: zero.map(|z| z.0),
        stop_percent: zero.map(|z| z.1),
    })
}
const T0: Duration = Duration::ZERO;
fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn lower_priority_never_overwrites_higher() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Agent, 80, Some(s(60))).unwrap();
    c.submit(Source::Profile, 20, None).unwrap();
    c.submit(Source::Schedule, 30, None).unwrap();
    assert_eq!(c.tick(T0), Some(80));
    c.submit(Source::AppRule, 50, None).unwrap();
    assert_eq!(c.tick(T0), Some(80));
    c.submit(Source::Override, 10, Some(s(60))).unwrap();
    assert_eq!(c.tick(T0), Some(10));
}

#[test]
fn same_source_resubmit_replaces() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Profile, 20, None).unwrap();
    c.submit(Source::Profile, 40, None).unwrap();
    assert_eq!(c.tick(T0), Some(40));
}

#[test]
fn overrides_expire_and_fall_back() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Profile, 20, None).unwrap();
    c.submit(Source::Override, 90, Some(s(10))).unwrap();
    assert_eq!(c.tick(s(9)), Some(90));
    assert_eq!(c.tick(s(10)), Some(20));
}

#[test]
fn override_and_agent_need_an_expiry() {
    let mut c = ctl(100, 100, 0, None);
    assert!(c.submit(Source::Override, 90, None).is_err());
    assert!(c.submit(Source::Agent, 90, None).is_err());
    assert_eq!(c.tick(T0), None);
}

#[test]
fn no_request_gives_none() {
    assert_eq!(ctl(10, 3, 0, None).tick(T0), None);
}

#[test]
fn ramps_with_step_up_and_step_down() {
    let mut c = ctl(10, 3, 0, None);
    c.submit(Source::Profile, 50, None).unwrap();
    assert_eq!(c.tick(T0), Some(50)); // first tick: no history, go to target
    c.submit(Source::Profile, 80, None).unwrap();
    assert_eq!(c.tick(T0), Some(60));
    assert_eq!(c.tick(T0), Some(70));
    c.submit(Source::Profile, 65, None).unwrap();
    assert_eq!(c.tick(T0), Some(67));
    assert_eq!(c.tick(T0), Some(65));
}

#[test]
fn min_pwm_floor_applies_before_ramping_and_duty_clamps() {
    let mut c = ctl(100, 100, 30, None);
    c.submit(Source::Profile, 10, None).unwrap();
    assert_eq!(c.tick(T0), Some(30));
    c.submit(Source::Profile, 250, None).unwrap();
    assert_eq!(c.tick(T0), Some(100));
}

#[test]
fn zero_rpm_stops_at_or_below_stop_and_restarts_strictly_above_start() {
    let mut c = ctl(100, 100, 25, Some((40, 30)));
    c.submit(Source::Profile, 50, None).unwrap();
    assert_eq!(c.tick(T0), Some(50));
    c.submit(Source::Profile, 31, None).unwrap();
    assert_eq!(c.tick(T0), Some(31)); // above stop: keeps running
    c.submit(Source::Profile, 30, None).unwrap();
    assert_eq!(c.tick(T0), Some(0)); // at stop: stops
    c.submit(Source::Profile, 40, None).unwrap();
    assert_eq!(c.tick(T0), Some(0)); // at start: stays stopped
    c.submit(Source::Profile, 41, None).unwrap();
    assert_eq!(c.tick(T0), Some(41)); // strictly above start: restarts
}

#[test]
fn zero_rpm_stop_still_ramps_down() {
    let mut c = ctl(100, 10, 0, Some((40, 30)));
    c.submit(Source::Profile, 50, None).unwrap();
    c.tick(T0);
    c.submit(Source::Profile, 20, None).unwrap();
    assert_eq!(c.tick(T0), Some(40));
}

#[test]
fn without_zero_rpm_config_the_fan_never_stops() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Profile, 0, None).unwrap();
    assert_eq!(c.tick(T0), Some(0)); // curve asked for 0: that is the duty, no latch involved
}

#[test]
fn withdraw_falls_back_to_next_source() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Profile, 30, None).unwrap();
    c.submit(Source::AppRule, 80, None).unwrap();
    assert_eq!(c.tick(T0), Some(80));
    c.withdraw(Source::AppRule);
    assert_eq!(c.tick(T0), Some(30));
}

#[test]
fn withdrawing_every_source_gives_none_not_a_stale_duty() {
    let mut c = ctl(100, 100, 0, None);
    c.submit(Source::Profile, 30, None).unwrap();
    assert_eq!(c.tick(T0), Some(30));
    c.withdraw(Source::Profile);
    assert_eq!(c.tick(T0), None);
}

#[test]
fn seed_starts_ramping_from_the_real_fan() {
    let mut c = ctl(10, 3, 0, None);
    c.seed(20, true);
    c.submit(Source::Profile, 80, None).unwrap();
    assert_eq!(c.tick(T0), Some(30));
}

#[test]
fn seed_of_a_stopped_fan_without_zero_rpm_config_still_drives_it() {
    let mut c = ctl(100, 100, 0, None);
    c.seed(0, false);
    c.submit(Source::Profile, 50, None).unwrap();
    assert_eq!(c.tick(T0), Some(50));
}

#[test]
fn seed_of_a_stopped_fan_stays_stopped_until_above_start() {
    let mut c = ctl(10, 3, 0, Some((30, 20)));
    c.seed(0, false);
    c.submit(Source::Profile, 30, None).unwrap();
    assert_eq!(c.tick(T0), Some(0));
    c.submit(Source::Profile, 31, None).unwrap();
    assert_eq!(c.tick(T0), Some(31));
}

#[test]
fn restart_jumps_to_start_percent_then_ramps() {
    let mut c = ctl(10, 100, 0, Some((30, 20)));
    c.submit(Source::Profile, 50, None).unwrap();
    c.tick(T0);
    c.submit(Source::Profile, 10, None).unwrap();
    assert_eq!(c.tick(T0), Some(0));
    c.submit(Source::Profile, 60, None).unwrap();
    assert_eq!(c.tick(T0), Some(40)); // jump to 30, then +10: never 10 or 20
    assert_eq!(c.tick(T0), Some(50));
}
