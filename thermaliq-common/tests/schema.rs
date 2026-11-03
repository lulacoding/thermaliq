#![allow(clippy::unwrap_used)]
use thermaliq_common::{bundled_profiles, DaemonConfig, Profile};

const HEAD: &str = "name = \"t\"\n";

fn parse(body: &str) -> Result<Profile, thermaliq_common::Error> {
    Profile::from_toml(&format!("{HEAD}{body}"))
}

fn err(body: &str) -> String {
    match parse(body) {
        Ok(_) => panic!("expected an error for:\n{body}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn every_bundled_profile_parses_and_matches_its_name() {
    let all = bundled_profiles();
    assert!(all.len() >= 5);
    for (name, toml) in all {
        let p = Profile::from_toml(toml).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.name, *name);
    }
}

#[test]
fn full_profile_with_every_in_scope_feature_parses() {
    let p = parse(
        r#"
[[sensors]]
id = "cpu_avg"
type = "time_average"
source = "cpu_package"
time_s = 10
[[sensors]]
id = "hot"
type = "mix"
function = "max"
sources = ["cpu_avg", "gpu_edge"]
[[sensors]]
id = "ext"
type = "file"
path = "C:/temp.txt"
[[sensors]]
id = "cpu_off"
type = "offset"
source = "cpu_package"
offset = -5.0

[[curves]]
id = "lin"
type = "linear"
source = "hot"
min_temp = 40
max_temp = 80
min_pwm = 30
max_pwm = 100
[[curves]]
id = "flat"
type = "flat"
pwm = 50
[[curves]]
id = "gr"
type = "graph"
source = "hot"
points = [{ temp = 30, pwm = 20 }, { temp = 60, pwm = 50 }, { temp = 90, pwm = 100 }]
[[curves]]
id = "mx"
type = "mix"
function = "average"
curves = ["lin", "gr"]
[[curves]]
id = "tr"
type = "trigger"
source = "hot"
idle_temp = 45
idle_pwm = 30
load_temp = 70
load_pwm = 90
[[curves]]
id = "sy"
type = "sync"
target = "lin"
offset = 5

[[controls]]
fan = "chassis1"
curve = "mx"
step_up_pct = 15
min_pwm = 20
[[controls]]
fan = "gpu1"
curve = "sy"

[[alerts]]
sensor = "cpu_package"
threshold = 85
severity = "warn"
action = { type = "notify" }
[[alerts]]
sensor = "cpu_package"
threshold = 92
severity = "crit"
action = { type = "apply_profile", profile = "performance" }

[[app_rules]]
process = "game.exe"
profile = "gaming"
"#,
    )
    .unwrap();
    assert_eq!(p.curves.len(), 6);
    assert_eq!(p.controls[1].step_up_pct, 10); // default
}

#[test]
fn out_of_scope_curve_types_are_rejected_by_name() {
    for t in ["auto", "ai", "predictive", "learning"] {
        let e = err(&format!("[[curves]]\nid = \"x\"\ntype = \"{t}\"\n"));
        assert!(e.contains(t), "{t}: {e}");
    }
}

#[test]
fn unknown_fields_are_rejected() {
    assert!(err("trust_level = 3\n").contains("trust_level"));
    assert!(
        err("[[curves]]\nid=\"f\"\ntype=\"flat\"\npwm=5\nlookahead_secs=3\n")
            .contains("lookahead_secs")
    );
}

#[test]
fn bad_values_give_readable_errors() {
    let lin = |extra: &str| {
        format!("[[curves]]\nid=\"c1\"\ntype=\"linear\"\nsource=\"s\"\nmin_temp=40\nmax_temp=80\nmin_pwm=0\nmax_pwm=100\n{extra}")
    };
    // equal temps would divide by zero
    let e = err(&lin("").replace("max_temp=80", "max_temp=40"));
    assert!(e.contains("c1") && e.contains("min_temp"), "{e}");
    // duty above 100 (S7)
    let e = err(&lin("").replace("max_pwm=100", "max_pwm=101"));
    assert!(e.contains("c1") && e.contains("100"), "{e}");
    let e = err(&lin("").replace("min_temp=40", "min_temp=nan"));
    assert!(e.contains("c1") && e.contains("finite"), "{e}");
    assert!(err("[[curves]]\nid=\"f\"\ntype=\"flat\"\npwm=200\n").contains("100"));
    assert!(err("[[curves]]\nid=\"\"\ntype=\"flat\"\npwm=5\n").contains("id"));
}

#[test]
fn graph_points_must_be_sorted_and_at_least_two() {
    let g =
        |pts: &str| format!("[[curves]]\nid=\"g\"\ntype=\"graph\"\nsource=\"s\"\npoints={pts}\n");
    assert!(err(&g("[{temp=30,pwm=10}]")).contains("two"));
    assert!(err(&g("[{temp=60,pwm=10},{temp=30,pwm=20}]")).contains("increasing"));
    assert!(err(&g("[{temp=30,pwm=10},{temp=30,pwm=20}]")).contains("increasing"));
    assert!(err(&g("[{temp=30,pwm=10},{temp=60,pwm=120}]")).contains("100"));
}

#[test]
fn references_must_resolve_and_not_cycle() {
    let flat = "[[curves]]\nid=\"a\"\ntype=\"flat\"\npwm=5\n";
    assert!(err(&format!(
        "{flat}[[curves]]\nid=\"a\"\ntype=\"flat\"\npwm=5\n"
    ))
    .contains("duplicate"));
    assert!(err(&format!("{flat}[[controls]]\nfan=\"f\"\ncurve=\"nope\"\n")).contains("nope"));
    assert!(err(&format!(
        "{flat}[[controls]]\nfan=\"f\"\ncurve=\"a\"\n[[controls]]\nfan=\"f\"\ncurve=\"a\"\n"
    ))
    .contains("fan"));
    assert!(
        err("[[curves]]\nid=\"m\"\ntype=\"mix\"\nfunction=\"max\"\ncurves=[\"zz\"]\n")
            .contains("zz")
    );
    assert!(err("[[curves]]\nid=\"m\"\ntype=\"mix\"\nfunction=\"max\"\ncurves=[]\n").contains("m"));
    // direct self reference and a two-step loop
    assert!(err("[[curves]]\nid=\"s\"\ntype=\"sync\"\ntarget=\"s\"\n").contains("cycle"));
    let two = "[[curves]]\nid=\"m\"\ntype=\"mix\"\nfunction=\"max\"\ncurves=[\"s\"]\n[[curves]]\nid=\"s\"\ntype=\"sync\"\ntarget=\"m\"\n";
    assert!(err(two).contains("cycle"));
}

#[test]
fn sensors_alerts_and_rules_are_validated() {
    assert!(
        err("[[sensors]]\nid=\"a\"\ntype=\"time_average\"\nsource=\"s\"\ntime_s=0\n")
            .contains("time_s")
    );
    assert!(
        err("[[sensors]]\nid=\"a\"\ntype=\"mix\"\nfunction=\"max\"\nsources=[]\n")
            .contains("sources")
    );
    assert!(err(
        "[[alerts]]\nsensor=\"s\"\nthreshold=999\nseverity=\"warn\"\naction={type=\"log\"}\n"
    )
    .contains("threshold"));
    assert!(err("[[alerts]]\nsensor=\"s\"\nthreshold=80\nseverity=\"warn\"\naction={type=\"apply_profile\",profile=\"\"}\n").contains("profile"));
    assert!(err("[[app_rules]]\nprocess=\"\"\nprofile=\"x\"\n").contains("process"));
}

#[test]
fn trigger_needs_load_above_idle() {
    let e = err("[[curves]]\nid=\"t\"\ntype=\"trigger\"\nsource=\"s\"\nidle_temp=70\nidle_pwm=20\nload_temp=60\nload_pwm=90\n");
    assert!(e.contains("load_temp"), "{e}");
}

#[test]
fn empty_name_and_syntax_errors_are_readable() {
    assert!(Profile::from_toml("name = \"\"")
        .unwrap_err()
        .to_string()
        .contains("name"));
    assert!(Profile::from_toml("name = ").is_err());
    assert!(Profile::from_toml("")
        .unwrap_err()
        .to_string()
        .contains("name"));
}

#[test]
fn config_defaults_and_validation() {
    let c = DaemonConfig::from_toml("").unwrap();
    assert_eq!((c.api_port, c.polling_interval_ms), (7272, 1000));
    assert_eq!(c.default_profile, "balanced");
    let c2 = DaemonConfig::from_toml(&toml::to_string(&c).unwrap()).unwrap();
    assert_eq!(c, c2);
    assert!(DaemonConfig::from_toml("api_port = 0").is_err());
    assert!(DaemonConfig::from_toml("polling_interval_ms = 10").is_err());
    assert!(DaemonConfig::from_toml("log_level = \"loud\"").is_err());
    assert!(DaemonConfig::from_toml("default_profile = \"\"").is_err());
    // keys never live in TOML; unknown sections are errors, not ignored
    assert!(DaemonConfig::from_toml("[ai]\napi_key = \"sk-x\"").is_err());
}
