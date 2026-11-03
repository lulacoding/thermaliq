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

#[test]
fn inverted_curves_are_rejected() {
    let lin = "[[curves]]\nid=\"c\"\ntype=\"linear\"\nsource=\"s\"\nmin_temp=40\nmax_temp=80\nmin_pwm=100\nmax_pwm=0\n";
    assert!(err(lin).contains("min_pwm"));
    let tr = "[[curves]]\nid=\"t\"\ntype=\"trigger\"\nsource=\"s\"\nidle_temp=45\nidle_pwm=90\nload_temp=70\nload_pwm=10\n";
    assert!(err(tr).contains("idle_pwm"));
    let g = "[[curves]]\nid=\"g\"\ntype=\"graph\"\nsource=\"s\"\npoints=[{temp=30,pwm=80},{temp=60,pwm=20}]\n";
    assert!(err(g).contains("non-decreasing"));
    // equal duties are fine
    parse("[[curves]]\nid=\"g\"\ntype=\"graph\"\nsource=\"s\"\npoints=[{temp=30,pwm=50},{temp=60,pwm=50}]\n").unwrap();
}

#[test]
fn curve_temps_must_be_in_sensor_range() {
    let lin = |a: &str, b: &str| {
        format!("[[curves]]\nid=\"c\"\ntype=\"linear\"\nsource=\"s\"\nmin_temp={a}\nmax_temp={b}\nmin_pwm=0\nmax_pwm=100\n")
    };
    assert!(err(&lin("40", "1e30")).contains("max_temp"));
    assert!(err(&lin("-50", "80")).contains("min_temp"));
    parse(&lin("-20", "150")).unwrap();
    let g = "[[curves]]\nid=\"g\"\ntype=\"graph\"\nsource=\"s\"\npoints=[{temp=30,pwm=10},{temp=500,pwm=20}]\n";
    assert!(err(g).contains("temp"));
    let tr = "[[curves]]\nid=\"t\"\ntype=\"trigger\"\nsource=\"s\"\nidle_temp=45\nidle_pwm=10\nload_temp=900\nload_pwm=90\n";
    assert!(err(tr).contains("load_temp"));
}

#[test]
fn polling_interval_is_capped_so_failsafes_trip_quickly() {
    for ms in [249, 2001, 60_000] {
        assert!(
            DaemonConfig::from_toml(&format!("polling_interval_ms = {ms}")).is_err(),
            "{ms}"
        );
    }
    for ms in [250, 2000] {
        DaemonConfig::from_toml(&format!("polling_interval_ms = {ms}")).unwrap();
    }
}

#[test]
fn zero_rpm_hysteresis_is_both_or_neither_and_start_not_below_stop() {
    let ctl = |extra: &str| {
        format!("[[curves]]\nid=\"a\"\ntype=\"flat\"\npwm=5\n[[controls]]\nfan=\"f\"\ncurve=\"a\"\n{extra}")
    };
    let p = parse(&ctl("start_percent = 30\nstop_percent = 20\n")).unwrap();
    assert_eq!(
        (p.controls[0].start_percent, p.controls[0].stop_percent),
        (Some(30), Some(20))
    );
    assert_eq!(parse(&ctl("")).unwrap().controls[0].start_percent, None);
    parse(&ctl("start_percent = 20\nstop_percent = 20\n")).unwrap();
    assert!(err(&ctl("start_percent = 30\n")).contains("both"));
    assert!(err(&ctl("stop_percent = 30\n")).contains("both"));
    assert!(err(&ctl("start_percent = 10\nstop_percent = 20\n")).contains("start_percent"));
    assert!(err(&ctl("start_percent = 101\nstop_percent = 20\n")).contains("100"));
}

#[test]
fn sync_offset_is_limited_to_100() {
    let s = |o: i32| {
        format!("[[curves]]\nid=\"a\"\ntype=\"flat\"\npwm=5\n[[curves]]\nid=\"s\"\ntype=\"sync\"\ntarget=\"a\"\noffset={o}\n")
    };
    parse(&s(100)).unwrap();
    parse(&s(-100)).unwrap();
    assert!(err(&s(101)).contains("offset"));
    assert!(err(&s(-101)).contains("offset"));
}

#[test]
fn bundled_profiles_say_their_ids_are_roles() {
    for (name, toml) in bundled_profiles() {
        let head: String = toml
            .lines()
            .take_while(|l| l.starts_with('#'))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            head.contains("role"),
            "{name} lacks the role-ID header comment"
        );
    }
}

#[test]
fn file_sensor_is_out_of_scope() {
    // SPEC §2: `file` moved to v2.1
    err("[[sensors]]\nid = \"ext\"\ntype = \"file\"\npath = \"C:/temp.txt\"\n");
}

#[test]
fn time_average_window_is_capped_at_one_hour() {
    let s = |t: u32| {
        format!("[[sensors]]\nid=\"a\"\ntype=\"time_average\"\nsource=\"s\"\ntime_s={t}\n")
    };
    assert!(parse(&s(3600)).is_ok());
    assert!(err(&s(3601)).contains("time_s"));
}
