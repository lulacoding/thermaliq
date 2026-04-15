use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:7272";

#[derive(Parser)]
#[command(name = "tq", about = "ThermalIQ CLI — talk to the local daemon", version)]
struct Cli {
    /// Daemon base URL
    #[arg(long, env = "THERMALIQ_URL", default_value = DEFAULT_BASE_URL)]
    url: String,

    /// Bearer token (auto-loaded from config if omitted)
    #[arg(long, env = "THERMALIQ_TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List all temperature sensors
    Sensors,
    /// List all fans
    Fans,
    /// Show daemon status (health + version)
    Status,
    /// Profile management
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Set a fan duty cycle manually
    Fan {
        /// Fan ID
        id: String,
        /// Duty cycle 0–100
        #[arg(long)]
        pwm: u8,
    },
    /// Agent control and monitoring
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Historical sensor/fan data
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
    /// Alert management
    Alert {
        #[command(subcommand)]
        action: AlertAction,
    },
    /// Live-stream sensor and fan data to the terminal (Ctrl+C to stop)
    Watch {
        /// Poll interval in milliseconds
        #[arg(long, default_value = "1000")]
        interval_ms: u64,
    },
    /// Stop the daemon
    Stop,
}

#[derive(Subcommand)]
enum ProfileAction {
    /// List available profiles
    List,
    /// Show active profile
    Active,
    /// Set active profile by name
    Set {
        /// Profile name
        name: String,
    },
    /// Show details of a profile
    Show {
        /// Profile name
        name: String,
    },
}

#[derive(Subcommand)]
enum AgentAction {
    /// Show agent status (mode + pending interventions)
    Status,
    /// Set agent mode: passive | approval | autonomous
    Mode {
        /// Agent mode
        mode: String,
    },
    /// List pending interventions
    Interventions,
    /// Approve a pending intervention
    Approve {
        /// Intervention ID
        id: String,
    },
    /// Reject a pending intervention
    Reject {
        /// Intervention ID
        id: String,
    },
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Sensor temperature history
    Sensors {
        /// Sensor ID
        sensor_id: String,
        /// Number of records
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Fan history
    Fans {
        /// Fan ID
        fan_id: String,
        /// Number of records
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum AlertAction {
    /// List alerts in the active profile
    List,
}

fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq")
        .join("api.token")
}

fn load_token() -> Option<String> {
    fs::read_to_string(token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn get(client: &Client, url: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body: Value = resp.json().with_context(|| format!("parse response from {url}"))?;
    if !status.is_success() {
        anyhow::bail!("HTTP {status}: {body}");
    }
    Ok(body)
}

fn post_authed(client: &Client, url: &str, token: &str, body: &Value) -> Result<Value> {
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(body)
        .send()
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let resp_body: Value = resp.json().with_context(|| format!("parse response from {url}"))?;
    if !status.is_success() {
        anyhow::bail!("HTTP {status}: {resp_body}");
    }
    Ok(resp_body)
}

fn require_token(cli_token: &Option<String>) -> Result<String> {
    cli_token
        .clone()
        .or_else(load_token)
        .context("no API token found — run the daemon first or set THERMALIQ_TOKEN")
}

fn print_json(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new();
    let base = cli.url.trim_end_matches('/');

    match &cli.command {
        Command::Sensors => {
            print_json(&get(&client, &format!("{base}/sensors"))?)?;
        }

        Command::Fans => {
            print_json(&get(&client, &format!("{base}/fans"))?)?;
        }

        Command::Status => {
            let health = get(&client, &format!("{base}/health"))?;
            let version = get(&client, &format!("{base}/api/version"))?;
            let agent = get(&client, &format!("{base}/api/agent/status"))?;
            println!("health:  {}", health["status"].as_str().unwrap_or("?"));
            println!("version: {}", version["version"].as_str().unwrap_or("?"));
            println!("agent:   {} (pending: {})", agent["mode"].as_str().unwrap_or("?"), agent["pending_count"].as_u64().unwrap_or(0));
        }

        Command::Profile { action } => match action {
            ProfileAction::List => {
                let names = thermaliq_common::config::list_profiles().unwrap_or_default();
                if names.is_empty() {
                    println!("No profiles found in {}", thermaliq_common::config::profiles_dir().display());
                } else {
                    for name in &names {
                        println!("{name}");
                    }
                }
            }
            ProfileAction::Active => {
                let data = get(&client, &format!("{base}/api/profiles/active"))?;
                match data["name"].as_str() {
                    Some(n) => println!("active: {n}"),
                    None => println!("no profile active"),
                }
            }
            ProfileAction::Set { name } => {
                let token = require_token(&cli.token)?;
                let resp = post_authed(
                    &client,
                    &format!("{base}/api/profiles/active"),
                    &token,
                    &serde_json::json!({ "name": name }),
                )?;
                print_json(&resp)?;
            }
            ProfileAction::Show { name } => {
                let path = thermaliq_common::config::profiles_dir().join(format!("{name}.toml"));
                let profile = thermaliq_common::config::load_profile(&path)
                    .with_context(|| format!("load profile {name}"))?;
                print_json(&serde_json::to_value(&profile)?)?;
            }
        },

        Command::Fan { id, pwm } => {
            let token = require_token(&cli.token)?;
            let body = serde_json::json!({ "duty_percent": pwm });
            let resp = post_authed(&client, &format!("{base}/fans/{id}/pwm"), &token, &body)?;
            print_json(&resp)?;
        }

        Command::Agent { action } => match action {
            AgentAction::Status => {
                let data = get(&client, &format!("{base}/api/agent/status"))?;
                println!("mode:    {}", data["mode"].as_str().unwrap_or("?"));
                println!("pending: {}", data["pending_count"].as_u64().unwrap_or(0));
            }
            AgentAction::Mode { mode } => {
                let token = require_token(&cli.token)?;
                let resp = post_authed(
                    &client,
                    &format!("{base}/api/agent/mode"),
                    &token,
                    &serde_json::json!({ "mode": mode }),
                )?;
                print_json(&resp)?;
            }
            AgentAction::Interventions => {
                let data = get(&client, &format!("{base}/api/agent/interventions"))?;
                let empty = vec![];
                let list = data["interventions"].as_array().unwrap_or(&empty);
                if list.is_empty() {
                    println!("no pending interventions");
                } else {
                    for item in list {
                        println!(
                            "[{}] {} — {}",
                            item["id"].as_str().unwrap_or("?"),
                            item["proposed_action"].as_str().unwrap_or("?"),
                            item["description"].as_str().unwrap_or("?"),
                        );
                    }
                }
            }
            AgentAction::Approve { id } => {
                let token = require_token(&cli.token)?;
                let resp = post_authed(
                    &client,
                    &format!("{base}/api/agent/interventions/{id}/approve"),
                    &token,
                    &serde_json::json!({}),
                )?;
                print_json(&resp)?;
            }
            AgentAction::Reject { id } => {
                let token = require_token(&cli.token)?;
                let resp = post_authed(
                    &client,
                    &format!("{base}/api/agent/interventions/{id}/reject"),
                    &token,
                    &serde_json::json!({}),
                )?;
                print_json(&resp)?;
            }
        },

        Command::History { action } => match action {
            HistoryAction::Sensors { sensor_id, limit } => {
                let data = get(
                    &client,
                    &format!("{base}/api/history/sensors/{sensor_id}?limit={limit}"),
                )?;
                print_json(&data)?;
            }
            HistoryAction::Fans { fan_id, limit } => {
                let data = get(
                    &client,
                    &format!("{base}/api/history/fans/{fan_id}?limit={limit}"),
                )?;
                print_json(&data)?;
            }
        },

        Command::Alert { action } => match action {
            AlertAction::List => {
                let data = get(&client, &format!("{base}/api/profiles/active"))?;
                match data["name"].as_str() {
                    Some(name) => {
                        let path = thermaliq_common::config::profiles_dir()
                            .join(format!("{name}.toml"));
                        match thermaliq_common::config::load_profile(&path) {
                            Ok(profile) => {
                                if profile.alerts.is_empty() {
                                    println!("no alerts configured");
                                } else {
                                    for a in &profile.alerts {
                                        println!(
                                            "[{:?}] {} >= {:.1}°C → {:?}",
                                            a.severity, a.sensor, a.threshold, a.action
                                        );
                                    }
                                }
                            }
                            Err(e) => anyhow::bail!("could not load profile: {e}"),
                        }
                    }
                    None => println!("no active profile"),
                }
            }
        },

        Command::Watch { interval_ms } => {
            use std::thread;
            use std::time::Duration;

            println!("Watching sensors — Ctrl+C to stop");
            loop {
                let sensors = get(&client, &format!("{base}/sensors"));
                let fans = get(&client, &format!("{base}/fans"));

                // Clear screen (ANSI escape)
                print!("\x1B[2J\x1B[1;1H");
                println!("ThermalIQ live  ({}ms interval)\n", interval_ms);

                match sensors {
                    Ok(data) => {
                        println!("SENSORS");
                        if let Some(arr) = data["sensors"].as_array() {
                            for s in arr {
                                let _id = s["id"].as_str().unwrap_or("?");
                                let label = s["label"].as_str().unwrap_or("?");
                                let temp = s["temp_celsius"].as_f64().unwrap_or(0.0);
                                let bar = temp_bar(temp);
                                println!("  {:20} {:5.1}°C  {}", label, temp, bar);
                            }
                        }
                    }
                    Err(e) => println!("  error: {e}"),
                }

                println!();

                match fans {
                    Ok(data) => {
                        println!("FANS");
                        if let Some(arr) = data["fans"].as_array() {
                            for f in arr {
                                let label = f["label"].as_str().unwrap_or("?");
                                let rpm = f["rpm"].as_u64().unwrap_or(0);
                                let duty = f["duty_percent"].as_u64().unwrap_or(0);
                                let bar = duty_bar(duty);
                                println!("  {:20} {:4} RPM  {:3}%  {}", label, rpm, duty, bar);
                            }
                        }
                    }
                    Err(e) => println!("  error: {e}"),
                }

                thread::sleep(Duration::from_millis(*interval_ms));
            }
        }

        Command::Stop => {
            let token = require_token(&cli.token)?;
            let resp = post_authed(
                &client,
                &format!("{base}/api/shutdown"),
                &token,
                &serde_json::json!({}),
            )?;
            print_json(&resp)?;
        }
    }

    Ok(())
}

fn temp_bar(temp: f64) -> String {
    let filled = ((temp / 100.0) * 20.0).round() as usize;
    let filled = filled.min(20);
    let empty = 20 - filled;
    let color = if temp >= 85.0 { "\x1B[31m" } else if temp >= 70.0 { "\x1B[33m" } else { "\x1B[32m" };
    format!("{}[{}{}]\x1B[0m", color, "#".repeat(filled), ".".repeat(empty))
}

fn duty_bar(duty: u64) -> String {
    let filled = ((duty as f64 / 100.0) * 20.0).round() as usize;
    let filled = filled.min(20);
    let empty = 20 - filled;
    format!("[{}{}]", "=".repeat(filled), ".".repeat(empty))
}
