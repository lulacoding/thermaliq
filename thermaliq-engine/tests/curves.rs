#![allow(clippy::unwrap_used)]
use std::time::Duration;
use thermaliq_common::Profile;
use thermaliq_engine::{CurveEngine, Temps};

fn engine(curves: &str) -> CurveEngine {
    CurveEngine::new(&Profile::from_toml(&format!("name = \"t\"\n{curves}")).unwrap())
}

fn temps(v: &[(&str, f32)]) -> Temps {
    v.iter().map(|(k, t)| (k.to_string(), *t)).collect()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

const CPU_CURVES: &str = r#"
[[curves]]
id = "lin"
type = "linear"
source = "cpu"
min_temp = 40
max_temp = 80
min_pwm = 20
max_pwm = 100
[[curves]]
id = "gr"
type = "graph"
source = "cpu"
points = [{ temp = 30, pwm = 20 }, { temp = 60, pwm = 50 }, { temp = 90, pwm = 100 }]
[[curves]]
id = "flat"
type = "flat"
pwm = 55
"#;

#[test]
fn linear_clamps_and_interpolates() {
    let mut e = engine(CPU_CURVES);
    let mut d = |t| e.duty("lin", &temps(&[("cpu", t)]), ms(0)).unwrap();
    assert_eq!(d(10.0), 20);
    assert_eq!(d(40.0), 20);
    assert_eq!(d(60.0), 60);
    assert_eq!(d(80.0), 100);
    assert_eq!(d(120.0), 100);
}

#[test]
fn graph_clamps_ends_and_interpolates() {
    let mut e = engine(CPU_CURVES);
    let mut d = |t| e.duty("gr", &temps(&[("cpu", t)]), ms(0)).unwrap();
    assert_eq!(d(0.0), 20);
    assert_eq!(d(45.0), 35);
    assert_eq!(d(60.0), 50);
    assert_eq!(d(75.0), 75);
    assert_eq!(d(100.0), 100);
}

#[test]
fn flat_ignores_temps() {
    let mut e = engine(CPU_CURVES);
    assert_eq!(e.duty("flat", &temps(&[]), ms(0)), Some(55));
}

#[test]
fn missing_or_invalid_source_or_unknown_curve_gives_none() {
    let mut e = engine(CPU_CURVES);
    assert_eq!(e.duty("lin", &temps(&[]), ms(0)), None);
    for bad in [f32::NAN, -50.0, 200.0] {
        assert_eq!(e.duty("lin", &temps(&[("cpu", bad)]), ms(0)), None);
    }
    assert_eq!(e.duty("nope", &temps(&[("cpu", 50.0)]), ms(0)), None);
}

const MIXES: &str = r#"
[[curves]]
id = "cpu_curve"
type = "linear"
source = "cpu"
min_temp = 40
max_temp = 80
min_pwm = 20
max_pwm = 100
[[curves]]
id = "gpu_curve"
type = "linear"
source = "gpu"
min_temp = 40
max_temp = 80
min_pwm = 30
max_pwm = 90
[[curves]]
id = "mix_one"
type = "mix"
function = "max"
curves = ["cpu_curve"]
[[curves]]
id = "mix_max"
type = "mix"
function = "max"
curves = ["cpu_curve", "gpu_curve"]
[[curves]]
id = "mix_min"
type = "mix"
function = "min"
curves = ["cpu_curve", "gpu_curve"]
[[curves]]
id = "mix_avg"
type = "mix"
function = "average"
curves = ["cpu_curve", "gpu_curve"]
[[curves]]
id = "mix_sum"
type = "mix"
function = "sum"
curves = ["cpu_curve", "gpu_curve"]
[[curves]]
id = "sync_up"
type = "sync"
target = "cpu_curve"
offset = 10
[[curves]]
id = "sync_down"
type = "sync"
target = "cpu_curve"
offset = -50
"#;

#[test]
fn mix_of_one_cpu_curve_at_75c_equals_that_curve() {
    let mut e = engine(MIXES);
    let t = temps(&[("cpu", 75.0), ("gpu", 20.0)]);
    let direct = e.duty("cpu_curve", &t, ms(0)).unwrap();
    assert_eq!(direct, 90);
    assert_eq!(e.duty("mix_one", &t, ms(0)), Some(direct));
}

#[test]
fn mix_functions_use_each_childs_own_source() {
    let mut e = engine(MIXES);
    let t = temps(&[("cpu", 60.0), ("gpu", 60.0)]); // cpu 60, gpu 60
    assert_eq!(e.duty("mix_max", &t, ms(0)), Some(60));
    assert_eq!(e.duty("mix_min", &t, ms(0)), Some(60));
    let t = temps(&[("cpu", 80.0), ("gpu", 40.0)]); // cpu 100, gpu 30
    assert_eq!(e.duty("mix_max", &t, ms(0)), Some(100));
    assert_eq!(e.duty("mix_min", &t, ms(0)), Some(30));
    assert_eq!(e.duty("mix_avg", &t, ms(0)), Some(65));
    assert_eq!(e.duty("mix_sum", &t, ms(0)), Some(100)); // 130 capped
}

#[test]
fn mix_average_rounds_not_floors() {
    let mut e = engine(MIXES);
    let t = temps(&[("cpu", 40.0), ("gpu", 45.0)]); // 20 and 37.5 -> 38; (20+38)/2 = 29
    assert_eq!(e.duty("mix_avg", &t, ms(0)), Some(29));
    let t = temps(&[("cpu", 40.0), ("gpu", 40.0)]); // 20 and 30
    assert_eq!(e.duty("mix_avg", &t, ms(0)), Some(25));
    let t = temps(&[("cpu", 41.0), ("gpu", 40.0)]); // 22 and 30 -> 26
    assert_eq!(e.duty("mix_avg", &t, ms(0)), Some(26));
}

#[test]
fn mix_with_a_missing_child_is_none() {
    let mut e = engine(MIXES);
    assert_eq!(e.duty("mix_max", &temps(&[("cpu", 90.0)]), ms(0)), None);
}

#[test]
fn sync_follows_target_at_targets_own_temp_with_clamped_offset() {
    let mut e = engine(MIXES);
    let t = temps(&[("cpu", 60.0)]); // target = 60
    assert_eq!(e.duty("sync_up", &t, ms(0)), Some(70));
    assert_eq!(e.duty("sync_down", &t, ms(0)), Some(10));
    let t = temps(&[("cpu", 80.0)]);
    assert_eq!(e.duty("sync_up", &t, ms(0)), Some(100));
    let t = temps(&[("cpu", 40.0)]);
    assert_eq!(e.duty("sync_down", &t, ms(0)), Some(0));
}

const TRIGGER: &str = r#"
[[curves]]
id = "trg"
type = "trigger"
source = "cpu"
idle_temp = 50
idle_pwm = 30
load_temp = 70
load_pwm = 90
response_ms = 2000
"#;

fn trg(e: &mut CurveEngine, t: f32, at: u64) -> u8 {
    e.duty("trg", &temps(&[("cpu", t)]), ms(at)).unwrap()
}

#[test]
fn trigger_waits_response_ms_before_switching_to_load() {
    let mut e = engine(TRIGGER);
    assert_eq!(trg(&mut e, 40.0, 0), 30);
    assert_eq!(trg(&mut e, 75.0, 1000), 30); // want load, timer starts
    assert_eq!(trg(&mut e, 75.0, 2999), 30); // 1999 ms held
    assert_eq!(trg(&mut e, 75.0, 3000), 90); // 2000 ms held
}

#[test]
fn trigger_timer_resets_if_the_condition_breaks() {
    let mut e = engine(TRIGGER);
    trg(&mut e, 75.0, 0);
    trg(&mut e, 60.0, 1500); // dead band: back to holding idle
    assert_eq!(trg(&mut e, 75.0, 2000), 30); // timer restarted
    assert_eq!(trg(&mut e, 75.0, 3500), 30);
    assert_eq!(trg(&mut e, 75.0, 4000), 90);
}

#[test]
fn trigger_holds_in_the_dead_band_both_ways() {
    let mut e = engine(TRIGGER);
    trg(&mut e, 75.0, 0);
    assert_eq!(trg(&mut e, 75.0, 2000), 90);
    assert_eq!(trg(&mut e, 60.0, 10_000), 90); // between idle and load: stays load
    assert_eq!(trg(&mut e, 50.0, 11_000), 90); // idle wanted, waiting
    assert_eq!(trg(&mut e, 50.0, 13_000), 30);
    assert_eq!(trg(&mut e, 60.0, 20_000), 30); // dead band: stays idle
}

#[test]
fn trigger_inside_a_mix_gives_the_same_answer_within_one_tick() {
    let mut e = engine(&format!(
        "{TRIGGER}[[curves]]\nid = \"m\"\ntype = \"mix\"\nfunction = \"max\"\ncurves = [\"trg\"]\n"
    ));
    let t = temps(&[("cpu", 75.0)]);
    assert_eq!(e.duty("m", &t, ms(0)), Some(30));
    assert_eq!(e.duty("trg", &t, ms(0)), Some(30)); // same tick, same answer
    assert_eq!(e.duty("m", &t, ms(2000)), Some(90));
}
