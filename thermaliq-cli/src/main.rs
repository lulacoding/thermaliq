use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:7272";

#[derive(Parser)]
#[command(name = "tq", about = "ThermalIQ CLI — talk to the local daemon")]
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
    /// Agent control
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Stop the daemon
    Stop,
}

#[derive(Subcommand)]
enum ProfileAction {
    /// List available profiles
    List,
    /// Show details of a profile
    Show {
        /// Profile name
        name: String,
    },
}

#[derive(Subcommand)]
enum AgentAction {
    /// Start the agent
    Start,
    /// Approve a pending intervention
    Approve {
        /// Intervention ID
        id: String,
    },
}

fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq")
        .join("api.token")
}

fn load_token() -> Option<String> {
    let path = token_path();
    fs::read_to_string(&path)
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new();
    let base = cli.url.trim_end_matches('/');

    match &cli.command {
        Command::Sensors => {
            let data = get(&client, &format!("{base}/sensors"))?;
            println!("{}", serde_json::to_string_pretty(&data)?);
        }

        Command::Fans => {
            let data = get(&client, &format!("{base}/fans"))?;
            println!("{}", serde_json::to_string_pretty(&data)?);
        }

        Command::Status => {
            let health = get(&client, &format!("{base}/health"))?;
            let version = get(&client, &format!("{base}/api/version"))?;
            println!("health:  {}", health["status"].as_str().unwrap_or("?"));
            println!("version: {}", version["version"].as_str().unwrap_or("?"));
        }

        Command::Profile { action } => match action {
            ProfileAction::List => {
                let names = thermaliq_common::config::list_profiles()
                    .unwrap_or_default();
                if names.is_empty() {
                    println!("No profiles found in {}", thermaliq_common::config::profiles_dir().display());
                } else {
                    for name in &names {
                        println!("{name}");
                    }
                }
            }
            ProfileAction::Show { name } => {
                let path = thermaliq_common::config::profiles_dir().join(format!("{name}.toml"));
                let profile = thermaliq_common::config::load_profile(&path)
                    .with_context(|| format!("load profile {name}"))?;
                println!("{}", serde_json::to_string_pretty(&profile)?);
            }
        },

        Command::Fan { id, pwm } => {
            let token = cli
                .token
                .clone()
                .or_else(load_token)
                .context("no API token found — run the daemon first or set THERMALIQ_TOKEN")?;
            let body = serde_json::json!({ "duty_percent": pwm });
            let resp = post_authed(&client, &format!("{base}/fans/{id}/pwm"), &token, &body)?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }

        Command::Agent { action } => {
            let token = cli
                .token
                .clone()
                .or_else(load_token)
                .context("no API token found — run the daemon first or set THERMALIQ_TOKEN")?;
            match action {
                AgentAction::Start => {
                    let resp = post_authed(
                        &client,
                        &format!("{base}/agent/start"),
                        &token,
                        &serde_json::json!({}),
                    )?;
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                AgentAction::Approve { id } => {
                    let resp = post_authed(
                        &client,
                        &format!("{base}/agent/approve/{id}"),
                        &token,
                        &serde_json::json!({}),
                    )?;
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
            }
        }

        Command::Stop => {
            let token = cli
                .token
                .clone()
                .or_else(load_token)
                .context("no API token found — run the daemon first or set THERMALIQ_TOKEN")?;
            let resp = post_authed(
                &client,
                &format!("{base}/daemon/stop"),
                &token,
                &serde_json::json!({}),
            )?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
    }

    Ok(())
}
