use crate::{invalid, Error, MAX_VALID_TEMP_C, MIN_VALID_TEMP_C};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub fn as_str(self) -> &'static str {
        DAYS[self as usize]
    }

    pub fn parse(s: &str) -> Option<Self> {
        use Weekday::*;
        let all = [Mon, Tue, Wed, Thu, Fri, Sat, Sun];
        DAYS.iter()
            .position(|d| d.eq_ignore_ascii_case(s))
            .map(|i| all[i])
    }
}

/// The wall clock the schedules read. Evaluated statelessly every tick, so a DST jump,
/// a manual clock change and a resume from sleep are all picked up on the next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub weekday: Weekday,
    pub minute_of_day: u16,
}

pub trait LocalClock: Send + Sync {
    fn now(&self) -> LocalTime;
}

/// A local-time profile window (`[[schedule]]`). Empty `days` means every day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEntry {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub days: Vec<String>,
    /// 24-hour range, for example `22:00-08:00` (midnight crossing is supported).
    pub time: String,
    pub profile: String,
}

fn yes() -> bool {
    true
}

impl ScheduleEntry {
    fn on_day(&self, day: &str) -> bool {
        self.days.is_empty() || self.days.iter().any(|d| d.eq_ignore_ascii_case(day))
    }

    /// Start included, end excluded. The after-midnight part of an overnight window
    /// belongs to the weekday the window started on.
    pub fn matches(&self, weekday: &str, minute: u16) -> bool {
        let Some((start, end)) = parse_time_range(&self.time) else {
            return false;
        };
        if start <= end {
            return minute >= start && minute < end && self.on_day(weekday);
        }
        if minute >= start {
            return self.on_day(weekday);
        }
        let Some(today) = DAYS.iter().position(|d| d.eq_ignore_ascii_case(weekday)) else {
            return false;
        };
        minute < end && self.on_day(DAYS[(today + 6) % 7])
    }
}

/// An enabled, matching schedule with the highest priority; configuration order breaks
/// ties. A target that is not `usable` is skipped and the next candidate is tried.
pub fn pick_schedule(
    list: &[ScheduleEntry],
    now: LocalTime,
    usable: impl Fn(&str) -> bool,
) -> Option<&ScheduleEntry> {
    best(
        list.iter().filter(|e| {
            e.enabled && e.matches(now.weekday.as_str(), now.minute_of_day) && usable(&e.profile)
        }),
        |e| e.priority,
    )
}

/// Foreground application rule (`[[app_rule]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppRuleEntry {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Executable name, matched whole and ASCII-case-insensitively.
    pub exe: String,
    pub profile: String,
    #[serde(default)]
    pub priority: i32,
}

pub fn pick_app_rule<'a>(
    list: &'a [AppRuleEntry],
    foreground_exe: &str,
    usable: impl Fn(&str) -> bool,
) -> Option<&'a AppRuleEntry> {
    best(
        list.iter().filter(|r| {
            r.enabled && r.exe.eq_ignore_ascii_case(foreground_exe) && usable(&r.profile)
        }),
        |r| r.priority,
    )
}

/// Highest priority; the earliest in configuration order wins a tie.
fn best<'a, T>(items: impl Iterator<Item = &'a T>, priority: impl Fn(&T) -> i32) -> Option<&'a T> {
    items.fold(None, |best: Option<&T>, x| match best {
        Some(b) if priority(b) >= priority(x) => Some(b),
        _ => Some(x),
    })
}

fn parse_time_range(s: &str) -> Option<(u16, u16)> {
    let (start, end) = s.split_once('-')?;
    let start = parse_hhmm(start, false)?;
    let end = parse_hhmm(end, true)?;
    (start != end).then_some((start, end))
}

fn parse_hhmm(s: &str, allow_24: bool) -> Option<u16> {
    let (hour, minute) = s.trim().split_once(':')?;
    let hour = hour.parse::<u16>().ok()?;
    let minute = minute.parse::<u16>().ok()?;
    if minute > 59 || hour > if allow_24 { 24 } else { 23 } || (hour == 24 && minute != 0) {
        return None;
    }
    Some(hour * 60 + minute)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warn,
    Crit,
}

/// SPEC §2: log, notify, apply-profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlertAction {
    Log,
    /// Recorded and shown by `tq alerts` and `/status`; the tray shows the toast (M18).
    Notify,
    ApplyProfile {
        profile: String,
    },
}

/// A global temperature alert (`[[alert]]`, SPEC §6). It is evaluated every tick whichever
/// profile is active, so its state survives profile switches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertEntry {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// A sensor role (`cpu_temp`) or a sensor id.
    pub sensor: String,
    pub threshold: f32,
    pub severity: Severity,
    pub action: AlertAction,
}

/// The alerts the bundled profiles carried before M16 moved them here.
fn default_alerts() -> Vec<AlertEntry> {
    let a = |id: &str, sensor: &str, threshold, severity, action| AlertEntry {
        id: id.into(),
        enabled: true,
        sensor: sensor.into(),
        threshold,
        severity,
        action,
    };
    vec![
        a("cpu-warm", "cpu_temp", 85.0, Severity::Warn, AlertAction::Notify),
        a("cpu-hot", "cpu_temp", 92.0, Severity::Crit, AlertAction::Log),
        a("gpu-hot", "gpu_temp", 85.0, Severity::Crit, AlertAction::Notify),
    ]
}

/// `config.toml`. No secrets: API keys live in the OS keychain (SPEC §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub api_port: u16,
    pub polling_interval_ms: u64,
    pub log_level: LogLevel,
    pub default_profile: String,
    /// The user's chosen fallback profile is `default_profile` (SPEC §4.1).
    #[serde(default)]
    pub app_rule: Vec<AppRuleEntry>,
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
    /// Without any `[[alert]]` in the file these are `default_alerts`; disable one with
    /// `enabled = false`.
    pub alert: Vec<AlertEntry>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            api_port: 7272,
            polling_interval_ms: 1000,
            log_level: LogLevel::Info,
            default_profile: "balanced".into(),
            app_rule: Vec::new(),
            schedule: Vec::new(),
            alert: default_alerts(),
        }
    }
}

impl DaemonConfig {
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let c: Self = toml::from_str(s)?;
        if c.api_port == 0 {
            return invalid("api_port must not be 0".into());
        }
        // S3/S4 trip after 3 ticks, so a slow tick delays the failsafe
        if !(250..=2000).contains(&c.polling_interval_ms) {
            return invalid(format!(
                "polling_interval_ms {} is outside 250..2000",
                c.polling_interval_ms
            ));
        }
        if c.default_profile.is_empty() {
            return invalid("default_profile must not be empty".into());
        }
        let mut ids = std::collections::HashSet::new();
        for r in &c.app_rule {
            if r.id.is_empty() || r.exe.is_empty() || r.profile.is_empty() {
                return invalid("app_rule needs a non-empty id, exe and profile".into());
            }
            if !ids.insert(r.id.as_str()) {
                return invalid(format!("duplicate app_rule id '{}'", r.id));
            }
        }
        let mut ids = std::collections::HashSet::new();
        for a in &c.alert {
            if a.id.is_empty() || a.sensor.is_empty() {
                return invalid("alert needs a non-empty id and sensor".into());
            }
            if !ids.insert(a.id.as_str()) {
                return invalid(format!("duplicate alert id '{}'", a.id));
            }
            if !(a.threshold >= MIN_VALID_TEMP_C && a.threshold <= MAX_VALID_TEMP_C) {
                return invalid(format!(
                    "alert '{}': threshold {} is outside {MIN_VALID_TEMP_C}..{MAX_VALID_TEMP_C}",
                    a.id, a.threshold
                ));
            }
            if matches!(&a.action, AlertAction::ApplyProfile { profile } if profile.is_empty()) {
                return invalid(format!("alert '{}': apply_profile needs a profile name", a.id));
            }
        }
        let mut ids = std::collections::HashSet::new();
        for entry in &c.schedule {
            if entry.id.is_empty() {
                return invalid("schedule id must not be empty".into());
            }
            if !ids.insert(entry.id.as_str()) {
                return invalid(format!("duplicate schedule id '{}'", entry.id));
            }
            if entry.profile.is_empty() {
                return invalid("schedule profile must not be empty".into());
            }
            if parse_time_range(&entry.time).is_none() {
                return invalid(format!(
                    "schedule time '{}' must be HH:MM-HH:MM",
                    entry.time
                ));
            }
            if entry.days.iter().any(|day| {
                !["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
                    .iter()
                    .any(|valid| day.eq_ignore_ascii_case(valid))
            }) {
                return invalid(format!("schedule '{}' has an invalid weekday", entry.time));
            }
        }
        Ok(c)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod alert_tests {
    use super::*;

    fn err(extra: &str) -> String {
        DaemonConfig::from_toml(extra).unwrap_err().to_string()
    }

    const HOT: &str =
        "[[alert]]\nid='hot'\nsensor='cpu_temp'\nthreshold=85\nseverity='warn'\naction={type='notify'}\n";

    #[test]
    fn alerts_are_config_tables_and_the_bundled_profiles_alerts_are_the_default() {
        let c = DaemonConfig::from_toml(HOT).unwrap();
        assert_eq!(c.alert.len(), 1);
        assert!(c.alert[0].enabled, "enabled defaults to true");
        assert_eq!(c.alert[0].action, AlertAction::Notify);

        // No `[[alert]]` in the file: the alerts the bundled profiles used to carry.
        let d = DaemonConfig::from_toml("").unwrap();
        let got: Vec<_> = d
            .alert
            .iter()
            .map(|a| (a.sensor.as_str(), a.threshold as u32, a.severity))
            .collect();
        assert_eq!(
            got,
            [
                ("cpu_temp", 85, Severity::Warn),
                ("cpu_temp", 92, Severity::Crit),
                ("gpu_temp", 85, Severity::Crit)
            ]
        );
        assert_eq!(DaemonConfig::default().alert, d.alert);
    }

    #[test]
    fn bad_alerts_are_refused_by_name() {
        let with = |s: &str| HOT.replace(s.split_once('|').unwrap().0, s.split_once('|').unwrap().1);
        assert!(err(&with("id='hot'|id=''")).contains("alert"));
        assert!(err(&format!("{HOT}{HOT}")).contains("duplicate alert id 'hot'"));
        assert!(err(&with("threshold=85|threshold=999")).contains("threshold"));
        assert!(err(&with("sensor='cpu_temp'|sensor=''")).contains("sensor"));
        assert!(err(&with("{type='notify'}|{type='apply_profile',profile=''}")).contains("profile"));
        assert!(err(&with("severity='warn'|severity='loud'")).contains("loud"));
        assert!(err(&HOT.replace("threshold", "limit")).contains("limit"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod schedule_tests {
    use super::*;

    fn entry(time: &str, days: &[&str]) -> ScheduleEntry {
        ScheduleEntry {
            id: "s".into(),
            enabled: true,
            priority: 0,
            days: days.iter().map(|d| (*d).into()).collect(),
            time: time.into(),
            profile: "quiet".into(),
        }
    }

    fn at(weekday: Weekday, h: u16, m: u16) -> LocalTime {
        LocalTime {
            weekday,
            minute_of_day: h * 60 + m,
        }
    }

    fn sched(id: &str, priority: i32, time: &str, profile: &str) -> ScheduleEntry {
        ScheduleEntry {
            id: id.into(),
            priority,
            profile: profile.into(),
            ..entry(time, &[])
        }
    }

    fn rule(id: &str, priority: i32, exe: &str, profile: &str) -> AppRuleEntry {
        AppRuleEntry {
            id: id.into(),
            enabled: true,
            exe: exe.into(),
            profile: profile.into(),
            priority,
        }
    }

    struct FakeClock(std::sync::Mutex<LocalTime>);
    impl LocalClock for FakeClock {
        fn now(&self) -> LocalTime {
            *self.0.lock().unwrap()
        }
    }

    #[test]
    fn schedule_start_is_included_and_end_is_excluded() {
        let e = entry("22:00-08:00", &[]);
        assert!(!e.matches("mon", 21 * 60 + 59));
        assert!(e.matches("mon", 22 * 60));
        assert!(e.matches("tue", 7 * 60 + 59));
        assert!(!e.matches("tue", 8 * 60));
    }

    #[test]
    fn schedule_priority_wins_then_configuration_order_and_disabled_are_ignored() {
        let mut off = sched("off", 9, "00:00-24:00", "disabled");
        off.enabled = false;
        let list = [
            sched("a", 0, "00:00-24:00", "first"),
            off,
            sched("b", 5, "00:00-24:00", "high"),
            sched("c", 5, "00:00-24:00", "tie"),
        ];
        let now = at(Weekday::Mon, 12, 0);
        assert_eq!(pick_schedule(&list, now, |_| true).unwrap().id, "b");
        // an unavailable target is skipped; the next candidate in the same source is tried
        let got = pick_schedule(&list, now, |p| p != "high").unwrap();
        assert_eq!(got.id, "c");
        assert!(pick_schedule(&list, now, |_| false).is_none());
    }

    #[test]
    fn app_rule_matches_the_whole_exe_name_ignoring_ascii_case_only() {
        let list = [rule("r", 0, "CS2.exe", "gaming")];
        for (exe, hit) in [
            ("cs2.EXE", true),
            ("xcs2.exe", false),
            ("cs2.exe.bak", false),
            ("cs2", false),
            ("", false),
        ] {
            assert_eq!(pick_app_rule(&list, exe, |_| true).is_some(), hit, "{exe}");
        }
    }

    #[test]
    fn app_rule_priority_then_order_disabled_and_unavailable() {
        let mut off = rule("off", 9, "a.exe", "x");
        off.enabled = false;
        let list = [
            rule("low", 0, "a.exe", "low"),
            off,
            rule("hi1", 3, "A.exe", "hi1"),
            rule("hi2", 3, "a.exe", "hi2"),
        ];
        assert_eq!(pick_app_rule(&list, "a.exe", |_| true).unwrap().id, "hi1");
        let got = pick_app_rule(&list, "a.exe", |p| p != "hi1").unwrap();
        assert_eq!(got.id, "hi2");
        let got = pick_app_rule(&list, "a.exe", |p| p == "low").unwrap();
        assert_eq!(got.id, "low");
    }

    #[test]
    fn config_parses_app_rules_and_schedules_with_defaults() {
        let c = DaemonConfig::from_toml(
            "[[app_rule]]\nid='cs2'\nexe='cs2.exe'\nprofile='gaming'\n\
             [[schedule]]\nid='night'\ndays=['fri']\ntime='22:00-08:00'\nprofile='silent'\npriority=2\nenabled=false",
        )
        .unwrap();
        let r = &c.app_rule[0];
        assert!(r.enabled && r.priority == 0 && r.exe == "cs2.exe");
        let s = &c.schedule[0];
        assert!(!s.enabled && s.priority == 2 && s.id == "night");
    }

    #[test]
    fn config_rejects_bad_ids_and_app_rule_fields() {
        for bad in [
            "[[app_rule]]\nid=''\nexe='a.exe'\nprofile='p'",
            "[[app_rule]]\nid='a'\nexe=''\nprofile='p'",
            "[[app_rule]]\nid='a'\nexe='a.exe'\nprofile=''",
            "[[app_rule]]\nid='a'\nexe='a.exe'\nprofile='p'\n[[app_rule]]\nid='a'\nexe='b.exe'\nprofile='p'",
            "[[schedule]]\nid=''\ntime='09:00-17:00'\nprofile='p'",
            "[[schedule]]\nid='a'\ntime='09:00-17:00'\nprofile='p'\n[[schedule]]\nid='a'\ntime='10:00-11:00'\nprofile='p'",
        ] {
            assert!(DaemonConfig::from_toml(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dst_and_clock_changes_are_stateless_wall_clock_checks() {
        let list = [sched("w", 0, "02:30-04:00", "quiet")];
        let clock = FakeClock(std::sync::Mutex::new(at(Weekday::Sun, 1, 59)));
        let pick = || pick_schedule(&list, clock.now(), |_| true).map(|e| e.id.clone());
        assert_eq!(pick(), None);
        // spring forward: 02:xx does not exist, the clock reads 03:00 and the window is active
        *clock.0.lock().unwrap() = at(Weekday::Sun, 3, 0);
        assert_eq!(pick().as_deref(), Some("w"));
        // clock set back by hand: re-evaluated on the next tick
        *clock.0.lock().unwrap() = at(Weekday::Sun, 2, 0);
        assert_eq!(pick(), None);
        // fall back: the wall clock repeats 03:30, so the same window is active twice
        for _ in 0..2 {
            *clock.0.lock().unwrap() = at(Weekday::Sun, 3, 30);
            assert_eq!(pick().as_deref(), Some("w"));
        }
    }

    #[test]
    fn reviewer_overnight_window_belongs_to_its_starting_weekday() {
        let friday = entry("22:00-08:00", &["fri"]);
        assert!(friday.matches("fri", 23 * 60));
        assert!(
            friday.matches("sat", 7 * 60),
            "Friday night must include Saturday 07:00"
        );
        assert!(
            !friday.matches("fri", 7 * 60),
            "Friday 07:00 belongs to Thursday night"
        );
    }
    #[test]
    fn schedule_matches_day_and_regular_or_midnight_window() {
        assert!(entry("09:00-17:00", &["mon"]).matches("Mon", 9 * 60));
        assert!(!entry("09:00-17:00", &["mon"]).matches("tue", 12 * 60));
        assert!(entry("22:00-08:00", &[]).matches("sun", 23 * 60));
        assert!(entry("22:00-08:00", &[]).matches("sun", 7 * 60 + 59));
        assert!(!entry("22:00-08:00", &[]).matches("sun", 12 * 60));
        assert!(entry("00:00-24:00", &[]).matches("sun", 23 * 60 + 59));
    }

    #[test]
    fn config_rejects_invalid_schedule_fields() {
        for schedule in [
            "[[schedule]]\nid='a'\ntime='25:00-26:00'\nprofile='quiet'",
            "[[schedule]]\nid='a'\ntime='09:00-09:00'\nprofile='quiet'",
            "[[schedule]]\nid='a'\ntime='09:00-17:00'\ndays=['funday']\nprofile='quiet'",
            "[[schedule]]\nid='a'\ntime='09:00-17:00'\nprofile=''",
        ] {
            let toml = format!("default_profile='balanced'\n{schedule}");
            assert!(DaemonConfig::from_toml(&toml).is_err(), "{schedule}");
        }
    }
}
