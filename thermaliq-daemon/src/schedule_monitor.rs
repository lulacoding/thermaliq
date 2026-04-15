//! Time-based profile schedule monitor (Phase 3.3).
//!
//! Checks the schedule every 60 s and activates the first matching entry.
//! When no entry matches, reverts to the base profile (same as app_monitor).
//!
//! Schedule entries live in `DaemonConfig::schedule` (loaded from config.toml).
//! They can also be updated at runtime via `AppState::schedule`.
//!
//! Priority: app rules (app_monitor) > schedule > base profile.
//! The schedule monitor only switches when no app-rule override is active; it
//! intentionally does not fight the app_monitor.

use chrono::{Datelike, Local, Timelike, Weekday};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::api::auth::AppState;
use thermaliq_common::config::{load_profile, profiles_dir, ScheduleEntry};

/// How often to re-evaluate the schedule.
const SCHEDULE_INTERVAL_SECS: u64 = 60;

pub fn start_schedule_monitor(
    state: AppState,
    base_profile_name: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(SCHEDULE_INTERVAL_SECS));
        let mut active_schedule_profile: Option<String> = None;

        loop {
            ticker.tick().await;

            let entries: Vec<ScheduleEntry> = state.schedule.read().await.clone();
            if entries.is_empty() {
                debug!("schedule monitor: no entries configured");
                continue;
            }

            let now = Local::now();
            let current_weekday = now.weekday();
            let current_minutes = now.hour() * 60 + now.minute();

            let matched = entries.iter().find(|entry| {
                day_matches(entry, current_weekday) && time_matches(entry, current_minutes)
            });

            match matched {
                Some(entry) => {
                    if active_schedule_profile.as_deref() != Some(entry.profile.as_str()) {
                        info!(
                            profile = %entry.profile,
                            note = %entry.note,
                            "schedule matched — switching profile"
                        );
                        if try_switch_profile(&state, &entry.profile).await {
                            active_schedule_profile = Some(entry.profile.clone());
                            let _ = state.agent_event_tx.send(
                                serde_json::json!({
                                    "type": "schedule_profile_switch",
                                    "profile": entry.profile,
                                    "note": entry.note,
                                })
                                .to_string(),
                            );
                        }
                    }
                }
                None => {
                    if active_schedule_profile.is_some() {
                        info!(
                            base = %base_profile_name,
                            "schedule window ended — restoring base profile"
                        );
                        if try_switch_profile(&state, &base_profile_name).await {
                            active_schedule_profile = None;
                            let _ = state.agent_event_tx.send(
                                serde_json::json!({
                                    "type": "schedule_profile_restore",
                                    "profile": base_profile_name,
                                })
                                .to_string(),
                            );
                        }
                    }
                }
            }

            debug!(
                schedule_active = ?active_schedule_profile,
                entries = entries.len(),
                "schedule monitor tick"
            );
        }
    })
}

/// Returns true if the current weekday matches the entry's `days` filter.
/// An empty `days` list means "every day".
fn day_matches(entry: &ScheduleEntry, weekday: Weekday) -> bool {
    if entry.days.is_empty() {
        return true;
    }
    let day_str = weekday_to_str(weekday);
    entry.days.iter().any(|d| d.to_lowercase() == day_str)
}

fn weekday_to_str(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

/// Returns true if `current_minutes` falls within the entry's time range.
///
/// Handles midnight-spanning ranges (e.g. "22:00-08:00"):
///  - "09:00-17:00" → normal range: 09:00 ≤ now < 17:00
///  - "22:00-08:00" → wraps midnight: now ≥ 22:00 OR now < 08:00
fn time_matches(entry: &ScheduleEntry, current_minutes: u32) -> bool {
    let (start, end) = match parse_time_range(&entry.time) {
        Some(v) => v,
        None => {
            warn!(time = %entry.time, "schedule: unparseable time range — skipping entry");
            return false;
        }
    };

    if start <= end {
        // Normal range (does not cross midnight)
        current_minutes >= start && current_minutes < end
    } else {
        // Midnight-spanning range
        current_minutes >= start || current_minutes < end
    }
}

/// Parse "HH:MM-HH:MM" → (start_minutes, end_minutes). Returns None on error.
fn parse_time_range(s: &str) -> Option<(u32, u32)> {
    let (start_str, end_str) = s.split_once('-')?;
    let start = parse_hhmm(start_str)?;
    let end = parse_hhmm(end_str)?;
    Some((start, end))
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let hours: u32 = h.parse().ok()?;
    let minutes: u32 = m.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}

/// Load and activate a named profile. Returns `true` on success.
async fn try_switch_profile(state: &AppState, profile_name: &str) -> bool {
    let path = profiles_dir().join(format!("{profile_name}.toml"));
    match load_profile(&path) {
        Ok(profile) => {
            *state.active_profile.write().await = Some(profile);
            true
        }
        Err(e) => {
            warn!(profile = %profile_name, error = %e, "schedule monitor: profile not found");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_range_in_window() {
        // 09:00-17:00, current = 12:30 → match
        assert!(time_matches(&make_entry("09:00-17:00"), 12 * 60 + 30));
    }

    #[test]
    fn normal_range_before_window() {
        assert!(!time_matches(&make_entry("09:00-17:00"), 8 * 60 + 59));
    }

    #[test]
    fn normal_range_after_window() {
        assert!(!time_matches(&make_entry("09:00-17:00"), 17 * 60));
    }

    #[test]
    fn midnight_range_before_midnight() {
        // 22:00-08:00, current = 23:30 → match
        assert!(time_matches(&make_entry("22:00-08:00"), 23 * 60 + 30));
    }

    #[test]
    fn midnight_range_after_midnight() {
        // 22:00-08:00, current = 06:00 → match
        assert!(time_matches(&make_entry("22:00-08:00"), 6 * 60));
    }

    #[test]
    fn midnight_range_outside_window() {
        // 22:00-08:00, current = 12:00 → no match
        assert!(!time_matches(&make_entry("22:00-08:00"), 12 * 60));
    }

    #[test]
    fn day_match_empty_days() {
        let entry = make_entry("00:00-24:00");
        assert!(day_matches(&entry, Weekday::Mon));
        assert!(day_matches(&entry, Weekday::Sun));
    }

    #[test]
    fn day_match_specific_days() {
        let mut entry = make_entry("09:00-17:00");
        entry.days = vec!["mon".into(), "fri".into()];
        assert!(day_matches(&entry, Weekday::Mon));
        assert!(day_matches(&entry, Weekday::Fri));
        assert!(!day_matches(&entry, Weekday::Wed));
        assert!(!day_matches(&entry, Weekday::Sun));
    }

    fn make_entry(time: &str) -> ScheduleEntry {
        ScheduleEntry {
            days: vec![],
            time: time.to_string(),
            profile: "test".to_string(),
            note: String::new(),
        }
    }
}
