//! Phase 3.2 — Natural Language Profile Creation via Claude API.
//!
//! Converts a plain-English description into a validated ThermalIQ profile
//! TOML by calling the Anthropic Messages API, then saves it to profiles_dir.
//!
//! Requires `ANTHROPIC_API_KEY` in the environment.

use anyhow::{anyhow, Context, Result};
use thermaliq_common::config::{profiles_dir, Profile};
use thermaliq_common::HardwareSnapshot;

// TODO(post-v1.0): profile_gen bypasses the configured AI provider and calls Anthropic directly.
// Multi-provider support for profile/curve generation endpoints is tracked for post-v1.0.
// See docs/AI_AGENT.md — "Profile generation requires ANTHROPIC_API_KEY" note.
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL: &str = "claude-haiku-4-5-20251001";

const SYSTEM_PROMPT: &str = r#"You are a ThermalIQ fan profile generator. Output ONLY raw TOML — no markdown fences, no explanation, nothing else.

Profile schema:
  name = "snake_case_name"        # required
  description = "..."             # required

  # Curves (at least one required)
  [[curves]]
  type = "linear"                 # types: linear, flat, graph, auto, trigger, mix, sync
  id = "cpu_curve"
  source = "cpu_tctl"             # sensor id
  min_temp = 40.0
  max_temp = 80.0
  min_pwm = 20                    # 0-100
  max_pwm = 100

  [[curves]]
  type = "flat"
  id = "silent_curve"
  pwm = 20                        # fixed duty %

  [[curves]]
  type = "graph"
  id = "custom_curve"
  source = "cpu_tctl"
  [[curves.points]]
  temp = 40.0
  pwm = 20
  [[curves.points]]
  temp = 80.0
  pwm = 100

  [[curves]]
  type = "auto"
  id = "auto_curve"
  source = "cpu_tctl"
  idle_temp = 45.0
  load_temp = 75.0
  min_pwm = 15
  max_pwm = 100

  # Controls — maps fans to curves (at least one required)
  [[controls]]
  fan = "cpu_fan"                 # fan id from hardware
  curve = "cpu_curve"
  min_pwm = 10                    # floor %  (optional, default 0)
  step_up_pct = 10                # max ramp up per tick (optional, default 10)
  step_down_pct = 3               # max ramp down per tick (optional, default 3)
  # stop_percent = 15             # duty% below which fan stops (0-RPM hysteresis)
  # start_percent = 25            # duty% above which stopped fan restarts

  # Alerts (optional)
  [[alerts]]
  sensor = "cpu_tctl"
  threshold = 90.0
  severity = "warn"               # "warn" or "crit"
  action = "notify"               # "log", "notify"

Rules:
- Never set min_pwm below 10 (safety).
- "quiet/silent" → use flat curve with pwm = 20–30, step_down_pct = 1.
- "balanced" → linear curve, min_pwm=20, max_pwm=90.
- "performance/gaming" → auto or linear curve, min_pwm=30, max_pwm=100, step_up_pct=15.
- Use real sensor/fan IDs from the hardware context when provided.
- Profile name must be snake_case with no spaces.
"#;

#[allow(dead_code)]
pub struct GeneratedProfile {
    pub name: String,
    pub toml: String,
    pub profile: Profile,
}

/// Generate a validated ThermalIQ profile from a natural language prompt.
pub async fn generate_profile(
    prompt: &str,
    snapshot: Option<&HardwareSnapshot>,
) -> Result<GeneratedProfile> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY not set — export it to enable profile generation")?;

    let hw_context = build_hw_context(snapshot);
    let user_message = format!("{hw_context}\n\nGenerate a profile: {prompt}");

    let client = reqwest::Client::new();
    let request_body = serde_json::json!({
        "model": MODEL,
        "max_tokens": 1024,
        "system": SYSTEM_PROMPT,
        "messages": [{"role": "user", "content": user_message}]
    });

    let resp = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .await
        .context("calling Anthropic API")?;

    let status = resp.status();
    let resp_json: serde_json::Value = resp.json().await.context("parsing Anthropic response")?;

    if !status.is_success() {
        return Err(anyhow!("Anthropic API error {}: {}", status, resp_json));
    }

    let raw = resp_json["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .ok_or_else(|| anyhow!("unexpected response shape from Anthropic API"))?
        .trim()
        .to_string();

    let toml_text = strip_code_fences(&raw);

    let profile: Profile = toml::from_str(&toml_text)
        .with_context(|| format!("generated TOML failed validation:\n{toml_text}"))?;

    if profile.name.is_empty() {
        return Err(anyhow!("generated profile has no `name` field"));
    }
    if profile.curves.is_empty() {
        return Err(anyhow!("generated profile has no `[[curves]]`"));
    }
    if profile.controls.is_empty() {
        return Err(anyhow!("generated profile has no `[[controls]]`"));
    }

    Ok(GeneratedProfile {
        name: profile.name.clone(),
        toml: toml_text,
        profile,
    })
}

/// Persist a generated profile to the profiles directory.
pub fn save_profile(gen: &GeneratedProfile) -> Result<std::path::PathBuf> {
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).context("creating profiles directory")?;
    let path = dir.join(format!("{}.toml", gen.name));
    std::fs::write(&path, &gen.toml)
        .with_context(|| format!("writing profile to {}", path.display()))?;
    tracing::info!(name = %gen.name, path = %path.display(), "profile generated and saved");
    Ok(path)
}

fn build_hw_context(snapshot: Option<&HardwareSnapshot>) -> String {
    match snapshot {
        Some(snap) => {
            let sensors: Vec<String> = snap
                .sensors
                .iter()
                .map(|s| format!("  {} ({}): {:.1}°C", s.id, s.label, s.temp_celsius))
                .collect();
            let fans: Vec<String> = snap
                .fans
                .iter()
                .map(|f| {
                    format!(
                        "  {} ({}): {} RPM, {}% duty",
                        f.id, f.label, f.rpm, f.duty_percent
                    )
                })
                .collect();
            format!(
                "Hardware context:\nSensors:\n{}\nFans:\n{}",
                sensors.join("\n"),
                fans.join("\n")
            )
        }
        None => {
            "Hardware: unknown — use generic IDs like 'cpu_tctl', 'cpu_fan', 'gpu_fan'.".to_string()
        }
    }
}

/// Strip markdown code fences that Claude may add despite instructions.
fn strip_code_fences(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("```") {
        let after = s.trim_start_matches('`');
        // Remove optional language tag ("toml\n")
        let after = after
            .strip_prefix("toml")
            .unwrap_or(after)
            .trim_start_matches('\n');
        if let Some(end) = after.rfind("```") {
            after[..end].trim().to_string()
        } else {
            after.trim().to_string()
        }
    } else {
        s.to_string()
    }
}
