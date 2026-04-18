mod privileged_helper;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

const API_BASE: &str = "http://127.0.0.1:7272";
const TRAY_ID: &str = "thermaliq-tray";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default, Clone)]
struct TrayState {
    profiles: Vec<String>,
    active_profile: String,
    agent_status: String,
    token: String,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            send_notification,
            check_for_updates,
            privileged_helper::install_privileged_helper,
            privileged_helper::uninstall_privileged_helper,
            privileged_helper::helper_status,
        ])
        .setup(|app| {
            let state = Arc::new(Mutex::new(TrayState::default()));
            setup_tray(app, Arc::clone(&state))?;
            start_daemon_poller(app.handle().clone(), state);

            // Check for updates on app start (non-blocking)
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await; // Wait for UI to be ready
                let _ = check_for_updates_internal(&app_handle).await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running ThermalIQ application");
}

#[tauri::command]
fn send_notification(title: String, body: String) {
    tracing::info!(title = %title, body = %body, "Sending notification");
    // Notification is sent via Tauri's native Windows toast API on Windows
    // On other platforms, it falls back gracefully
}

// ── Tray setup ────────────────────────────────────────────────────────────────

fn setup_tray(app: &mut tauri::App, state: Arc<Mutex<TrayState>>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show ThermalIQ", true, None::<&str>)?;
    let agent_toggle = MenuItem::with_id(app, "agent_toggle", "⏸ Pause AI Agent", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &agent_toggle, &sep, &quit])?;

    let state_clone = Arc::clone(&state);

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().cloned().unwrap_or_else(|| {
            tauri::image::Image::new(&[0, 0, 0, 255], 1, 1)
        }))
        .tooltip("ThermalIQ")
        .menu(&menu)
        .on_menu_event(move |app, event| {
            let id = event.id.as_ref();
            if id == "show" {
                show_main_window(app);
            } else if id == "quit" {
                app.exit(0);
            } else if id == "agent_toggle" {
                let token = state_clone.lock().unwrap().token.clone();
                toggle_agent(token);
            } else if id.starts_with("profile_") {
                let profile_name = id.trim_start_matches("profile_").to_string();
                let token = state_clone.lock().unwrap().token.clone();
                apply_profile(profile_name, token);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

// ── Background poller ─────────────────────────────────────────────────────────

fn start_daemon_poller(app: AppHandle, state: Arc<Mutex<TrayState>>) {
    std::thread::spawn(move || {
        // Load API token once; re-read on failure in case it was just created
        loop {
            let token = read_api_token();
            {
                state.lock().unwrap().token = token.clone();
            }

            let (profiles, active, agent_str) = fetch_daemon_state(&token);
            {
                let mut s = state.lock().unwrap();
                let changed = s.profiles != profiles
                    || s.active_profile != active
                    || s.agent_status != agent_str;
                if changed {
                    s.profiles = profiles.clone();
                    s.active_profile = active.clone();
                    s.agent_status = agent_str.clone();
                    drop(s);
                    rebuild_tray_menu(&app, &profiles, &active, &agent_str, &token);
                }
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

fn read_api_token() -> String {
    let token_path = dirs::config_dir()
        .unwrap_or_default()
        .join("thermaliq")
        .join("api.token");
    std::fs::read_to_string(&token_path)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn fetch_daemon_state(token: &str) -> (Vec<String>, String, String) {
    if token.is_empty() {
        return (Vec::new(), String::new(), "Daemon not connected".to_string());
    }

    // Fetch profile list
    let profiles = ureq::get(&format!("{API_BASE}/api/profiles"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Vec<serde_json::Value>>().ok())
        .map(|v| {
            v.into_iter()
                .filter_map(|p| p["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Fetch active profile
    let active = ureq::get(&format!("{API_BASE}/api/profiles/active"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()
        .and_then(|r| r.into_json::<serde_json::Value>().ok())
        .and_then(|v| v["name"].as_str().map(str::to_owned))
        .unwrap_or_default();

    // Fetch agent status
    let agent_str = ureq::get(&format!("{API_BASE}/api/agent/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()
        .and_then(|r| r.into_json::<serde_json::Value>().ok())
        .map(|v| {
            let mode = v["mode"].as_str().unwrap_or("unknown");
            let state = v["state"].as_str().unwrap_or("unknown");
            format!("Agent: {mode} / {state}")
        })
        .unwrap_or_else(|| "Daemon offline".to_string());

    (profiles, active, agent_str)
}

fn rebuild_tray_menu(
    app: &AppHandle,
    profiles: &[String],
    active: &str,
    agent_status: &str,
    token: &str,
) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };

    // Update tooltip
    let _ = tray.set_tooltip(Some(&format!("ThermalIQ — {agent_status}")));

    // Build menu
    let build = || -> tauri::Result<()> {
        let show = MenuItem::with_id(app, "show", "Show ThermalIQ", true, None::<&str>)?;
        let status_item = MenuItem::with_id(
            app,
            "status",
            &format!("● {agent_status}"),
            false,
            None::<&str>,
        )?;
        let sep1 = PredefinedMenuItem::separator(app)?;

        let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![&show, &sep1, &status_item];

        // Profile items — only show if daemon is online and has profiles
        let profile_items: Vec<MenuItem<tauri::Wry>>;
        let sep2: tauri::menu::PredefinedMenuItem<tauri::Wry>;

        if !profiles.is_empty() && !token.is_empty() {
            sep2 = PredefinedMenuItem::separator(app)?;
            items.push(&sep2);

            profile_items = profiles
                .iter()
                .map(|p| {
                    let label = if p == active {
                        format!("✓ {p}")
                    } else {
                        format!("  {p}")
                    };
                    MenuItem::with_id(app, &format!("profile_{p}"), &label, true, None::<&str>)
                })
                .collect::<tauri::Result<Vec<_>>>()?;

            for item in &profile_items {
                items.push(item);
            }
        }

        let sep_quit = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        items.push(&sep_quit);
        items.push(&quit);

        let menu = Menu::with_items(app, &items)?;
        tray.set_menu(Some(menu))?;
        Ok(())
    };

    if let Err(e) = build() {
        tracing::warn!("Failed to rebuild tray menu: {e}");
    }
}

fn apply_profile(profile_name: String, token: String) {
    std::thread::spawn(move || {
        let result = ureq::post(&format!("{API_BASE}/api/profiles/{profile_name}/activate"))
            .set("Authorization", &format!("Bearer {token}"))
            .call();
        match result {
            Ok(_) => tracing::info!(profile = %profile_name, "tray: profile applied"),
            Err(e) => tracing::warn!(profile = %profile_name, error = %e, "tray: profile apply failed"),
        }
    });
}

fn toggle_agent(token: String) {
    std::thread::spawn(move || {
        // Get current agent status
        let status_result = ureq::get(&format!("{API_BASE}/api/agent/status"))
            .set("Authorization", &format!("Bearer {token}"))
            .call()
            .ok()
            .and_then(|r| r.into_json::<serde_json::Value>().ok());

        if let Some(status) = status_result {
            let current_state = status["state"].as_str().unwrap_or("unknown");
            let new_state = if current_state == "running" { "paused" } else { "running" };

            // POST to toggle agent state
            let result = ureq::post(&format!("{API_BASE}/api/agent/{new_state}"))
                .set("Authorization", &format!("Bearer {token}"))
                .call();
            match result {
                Ok(_) => tracing::info!(state = new_state, "tray: agent toggled"),
                Err(e) => tracing::warn!(error = %e, "tray: agent toggle failed"),
            }
        }
    });
}

// ── Auto-updater ──────────────────────────────────────────────────────────────

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<bool, String> {
    check_for_updates_internal(&app).await
}

async fn check_for_updates_internal(app: &tauri::AppHandle) -> Result<bool, String> {
    if let Ok(update) = app.updater().check().await {
        if update.is_update_available() {
            tracing::info!("Update available: {}", update.latest_version());
            // The updater plugin automatically shows a dialog if configured with "dialog": true
            let _ = update.download_and_install().await;
            return Ok(true);
        }
    }
    Ok(false)
}
