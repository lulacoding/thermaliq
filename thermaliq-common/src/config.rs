use crate::{invalid, Error};
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

/// One local-time profile window. Empty `days` means every day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEntry {
    #[serde(default)]
    pub days: Vec<String>,
    /// 24-hour range, for example `22:00-08:00` (midnight crossing is supported).
    pub time: String,
    pub profile: String,
}

impl ScheduleEntry {
    pub fn matches(&self, weekday: &str, minute: u16) -> bool {
        if !self.days.is_empty()
            && !self
                .days
                .iter()
                .any(|day| day.eq_ignore_ascii_case(weekday))
        {
            return false;
        }
        let Some((start, end)) = parse_time_range(&self.time) else {
            return false;
        };
        if start <= end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        }
    }
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

/// `config.toml`. No secrets: API keys live in the OS keychain (SPEC §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub api_port: u16,
    pub polling_interval_ms: u64,
    pub log_level: LogLevel,
    pub default_profile: String,
    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            api_port: 7272,
            polling_interval_ms: 1000,
            log_level: LogLevel::Info,
            default_profile: "balanced".into(),
            schedule: Vec::new(),
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
        for entry in &c.schedule {
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
mod schedule_tests {
    use super::*;

    fn entry(time: &str, days: &[&str]) -> ScheduleEntry {
        ScheduleEntry {
            days: days.iter().map(|d| (*d).into()).collect(),
            time: time.into(),
            profile: "quiet".into(),
        }
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
            "[[schedule]]\ntime='25:00-26:00'\nprofile='quiet'",
            "[[schedule]]\ntime='09:00-09:00'\nprofile='quiet'",
            "[[schedule]]\ntime='09:00-17:00'\ndays=['funday']\nprofile='quiet'",
            "[[schedule]]\ntime='09:00-17:00'\nprofile=''",
        ] {
            let toml = format!("default_profile='balanced'\n{schedule}");
            assert!(DaemonConfig::from_toml(&toml).is_err(), "{schedule}");
        }
    }
}
