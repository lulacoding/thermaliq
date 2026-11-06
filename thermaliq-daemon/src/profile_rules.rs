//! Priority arbitration for Agent, app rules, schedule and the user's base profile (M15).

use thermaliq_common::{Profile, ScheduleEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Agent,
    AppRule,
    Schedule,
    User,
}

/// Selects the highest-priority active profile request.
pub fn arbitrate<'a>(
    agent: Option<&'a str>,
    app_rule: Option<&'a str>,
    schedule: Option<&'a str>,
    user: &'a str,
) -> (&'a str, ProfileSource) {
    if let Some(profile) = agent {
        (profile, ProfileSource::Agent)
    } else if let Some(profile) = app_rule {
        (profile, ProfileSource::AppRule)
    } else if let Some(profile) = schedule {
        (profile, ProfileSource::Schedule)
    } else {
        (user, ProfileSource::User)
    }
}

/// App rules always come from the configured base profile, which stays stable while an
/// app-rule, schedule, or Agent-selected profile is active.
pub fn app_rule_profile<'a>(base: &'a Profile, process_names: &[String]) -> Option<&'a str> {
    base.app_rules
        .iter()
        .find(|rule| {
            process_names
                .iter()
                .any(|exe| exe.to_lowercase().contains(&rule.process.to_lowercase()))
        })
        .map(|rule| rule.profile.as_str())
}

pub fn scheduled_profile<'a>(
    schedule: &'a [ScheduleEntry],
    weekday: &str,
    minute: u16,
) -> Option<&'a str> {
    schedule
        .iter()
        .find(|entry| entry.matches(weekday, minute))
        .map(|entry| entry.profile.as_str())
}

#[cfg(windows)]
pub fn local_time() -> (&'static str, u16) {
    crate::win::local_time()
}

/// Other CI targets use UTC. The shipped v2 service is Windows-only and uses local time.
#[cfg(not(windows))]
pub fn local_time() -> (&'static str, u16) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day = seconds / 86_400;
    let weekday = match (day + 4) % 7 {
        0 => "sun",
        1 => "mon",
        2 => "tue",
        3 => "wed",
        4 => "thu",
        5 => "fri",
        _ => "sat",
    };
    (weekday, ((seconds % 86_400) / 60) as u16)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::ScheduleEntry;

    #[test]
    fn priority_is_agent_then_app_rule_then_schedule_then_user() {
        assert_eq!(
            arbitrate(Some("agent"), Some("app"), Some("scheduled"), "user"),
            ("agent", ProfileSource::Agent)
        );
        assert_eq!(
            arbitrate(None, Some("app"), Some("scheduled"), "user"),
            ("app", ProfileSource::AppRule)
        );
        assert_eq!(
            arbitrate(None, None, Some("scheduled"), "user"),
            ("scheduled", ProfileSource::Schedule)
        );
        assert_eq!(
            arbitrate(None, None, None, "user"),
            ("user", ProfileSource::User)
        );
    }

    #[test]
    fn app_rules_are_read_from_the_base_profile_and_match_case_insensitively() {
        let base =
            Profile::from_toml("name='base'\n[[app_rules]]\nprocess='game.exe'\nprofile='gaming'")
                .unwrap();
        let active = Profile::from_toml("name='gaming'").unwrap();
        assert_eq!(
            app_rule_profile(&base, &["other.exe".into(), "GAME.EXE".into()]),
            Some("gaming")
        );
        assert_eq!(app_rule_profile(&active, &["GAME.EXE".into()]), None);
        assert_eq!(app_rule_profile(&base, &[]), None);
    }

    #[test]
    fn schedule_selects_first_matching_entry() {
        let entries = vec![
            ScheduleEntry {
                days: vec!["mon".into()],
                time: "09:00-17:00".into(),
                profile: "work".into(),
            },
            ScheduleEntry {
                days: vec![],
                time: "00:00-24:00".into(),
                profile: "everyday".into(),
            },
        ];
        assert_eq!(scheduled_profile(&entries, "mon", 10 * 60), Some("work"));
        assert_eq!(
            scheduled_profile(&entries, "tue", 10 * 60),
            Some("everyday")
        );
    }
}
