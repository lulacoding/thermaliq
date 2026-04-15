//! Per-application fan curve monitor (Phase 3.1).
//!
//! Scans running processes every 5 s and switches the active profile when a
//! matching `app_rules` entry is found. On app exit, restores the base profile.
//!
//! Priority: app rules > base profile; safety enforcer always overrides both.

use sysinfo::System;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

use crate::api::auth::AppState;
use thermaliq_common::config::{load_profile, profiles_dir};

/// How often to re-scan the process list.
const SCAN_INTERVAL_MS: u64 = 5_000;

/// Ramp duration when switching profiles due to an app rule match.
#[allow(dead_code)]
pub const APP_SWITCH_RAMP_MS: u64 = 10_000;

pub fn start_app_monitor(
    state: AppState,
    base_profile_name: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_millis(SCAN_INTERVAL_MS));
        let mut sys = System::new();
        // Track which app-rule profile is currently active (None = base profile).
        let mut active_override: Option<String> = None;

        loop {
            ticker.tick().await;

            // Refresh process list (names only — avoids full stat overhead).
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All);

            // Collect running process names (lowercase for case-insensitive match).
            let process_names: Vec<String> = sys
                .processes()
                .values()
                .map(|p| p.name().to_string_lossy().to_lowercase())
                .collect();

            // Read app_rules from whichever profile is currently active.
            let rules = {
                let guard = state.active_profile.read().await;
                guard
                    .as_ref()
                    .map(|p| p.app_rules.clone())
                    .unwrap_or_default()
            };

            // Find the first matching rule.
            let matched_rule = rules.iter().find(|rule| {
                let needle = rule.process.to_lowercase();
                process_names
                    .iter()
                    .any(|name| name.contains(needle.as_str()))
            });

            match matched_rule {
                Some(rule) => {
                    // Switch only if the override changed.
                    if active_override.as_deref() != Some(rule.profile.as_str()) {
                        info!(
                            process = %rule.process,
                            profile = %rule.profile,
                            note = %rule.note,
                            "app rule matched — switching profile"
                        );
                        if try_switch_profile(&state, &rule.profile).await {
                            active_override = Some(rule.profile.clone());
                            let _ = state.agent_event_tx.send(
                                serde_json::json!({
                                    "type": "app_profile_switch",
                                    "process": rule.process,
                                    "profile": rule.profile,
                                })
                                .to_string(),
                            );
                        }
                    }
                }
                None => {
                    // No rule matches — revert to base profile if we had an override.
                    if active_override.is_some() {
                        info!(
                            base = %base_profile_name,
                            "app rule no longer matches — restoring base profile"
                        );
                        if try_switch_profile(&state, &base_profile_name).await {
                            active_override = None;
                            let _ = state.agent_event_tx.send(
                                serde_json::json!({
                                    "type": "app_profile_restore",
                                    "profile": base_profile_name,
                                })
                                .to_string(),
                            );
                        }
                    }
                }
            }

            debug!(
                processes = process_names.len(),
                override_active = ?active_override,
                "app monitor tick"
            );
        }
    })
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
            tracing::warn!(profile = %profile_name, error = %e, "app monitor: profile not found");
            false
        }
    }
}
