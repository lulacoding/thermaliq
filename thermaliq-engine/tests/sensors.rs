#![allow(clippy::unwrap_used)]
use std::time::Duration;
use thermaliq_common::Profile;
use thermaliq_engine::{SensorEngine, Temps};

fn engine(sensors: &str) -> SensorEngine {
    SensorEngine::new(&Profile::from_toml(&format!("name = \"t\"\n{sensors}")).unwrap())
}

fn temps(v: &[(&str, f32)]) -> Temps {
    v.iter().map(|(k, t)| (k.to_string(), *t)).collect()
}

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

const SENSORS: &str = r#"
[[sensors]]
id = "avg"
type = "time_average"
source = "cpu"
time_s = 10
[[sensors]]
id = "off"
type = "offset"
source = "cpu"
offset = -5.0
[[sensors]]
id = "hot"
type = "mix"
function = "max"
sources = ["off", "gpu"]
[[sensors]]
id = "cool"
type = "mix"
function = "min"
sources = ["cpu", "gpu"]
"#;

#[test]
fn offset_and_mix_chain_in_definition_order_and_keep_real_sensors() {
    let mut e = engine(SENSORS);
    let out = e.evaluate(&temps(&[("cpu", 70.0), ("gpu", 60.0)]), s(0));
    assert_eq!(out["off"], 65.0);
    assert_eq!(out["hot"], 65.0);
    assert_eq!(out["cool"], 60.0);
    assert_eq!(out["cpu"], 70.0);
}

#[test]
fn time_average_covers_only_the_window() {
    let mut e = engine(SENSORS);
    e.evaluate(&temps(&[("cpu", 100.0)]), s(0));
    let o = e.evaluate(&temps(&[("cpu", 50.0)]), s(5));
    assert_eq!(o["avg"], 75.0);
    let o = e.evaluate(&temps(&[("cpu", 50.0)]), s(11)); // t=0 sample is 11 s old
    assert_eq!(o["avg"], 50.0);
}

#[test]
fn missing_or_invalid_source_means_no_sensor_not_a_guess() {
    let mut e = engine(SENSORS);
    let o = e.evaluate(&temps(&[("cpu", 70.0)]), s(0)); // gpu missing
    assert!(!o.contains_key("hot") && !o.contains_key("cool"));
    assert!(o.contains_key("off"));
    let o = e.evaluate(&temps(&[("cpu", f32::NAN), ("gpu", 60.0)]), s(1));
    assert!(!o.contains_key("off") && !o.contains_key("avg") && !o.contains_key("cool"));
}

#[test]
fn offset_result_out_of_range_is_dropped() {
    let mut e =
        engine("[[sensors]]\nid = \"o\"\ntype = \"offset\"\nsource = \"cpu\"\noffset = 100.0\n");
    let o = e.evaluate(&temps(&[("cpu", 90.0)]), s(0)); // 190 > 150
    assert!(!o.contains_key("o"));
}

#[test]
fn nan_samples_do_not_poison_the_average() {
    let mut e = engine(SENSORS);
    e.evaluate(&temps(&[("cpu", f32::NAN)]), s(0));
    let o = e.evaluate(&temps(&[("cpu", 60.0)]), s(1));
    assert_eq!(o["avg"], 60.0);
}

#[test]
fn custom_sensor_never_overwrites_a_real_reading() {
    for offset in ["40.0", "-40.0"] {
        let mut e = engine(&format!(
            "[[sensors]]\nid = \"cpu\"\ntype = \"offset\"\nsource = \"cpu\"\noffset = {offset}\n"
        ));
        let o = e.evaluate(&temps(&[("cpu", 60.0)]), s(0));
        assert_eq!(o["cpu"], 60.0, "offset {offset}");
    }
}
