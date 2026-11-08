//! `tq`: the command-line client for the daemon (SPEC §6.1). It only talks to the HTTP API: it
//! never touches hardware, and a fan duty can only be asked for through the engine.

mod client;
mod echo;
pub mod identify;
mod render;

use client::enc;
pub use client::Client;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use thermaliq_common::datadir::{data_dir_from, parse_token, DATA_DIR_ENV};
use thermaliq_common::DaemonConfig;

pub const USAGE: &str = "\
usage: tq [--json] <command>

  status                          engine state (exit 5 if degraded, latched or stopped)
  sensors | fans | metrics        live readings
  profile list | set <name>       the profiles, or switch to one
  override <fan> <duty> [--secs N]   hold a fan at 0-100% for N s (default 60, daemon caps at 300)
  override <fan> --cancel         end an override
  agent pause | resume            pause or resume the agent
  agent decisions                 what the agent decided and what it asks
  agent key set <provider>        store your key (read from stdin, never from the command line)
  agent key clear                 remove the key
  agent workload <exe> <kind>     tell the agent what an app is
  identity                        board, roles, inventory and proposed mapping
  identify [--probe <fan>]        first-run fan mapping; --probe runs one fan at 100% for 5 s
  inventory accept                accept the current fans as the saved inventory
  history <sensor> [--mins N]     a sensor's last N minutes (default 10)
  config show                     the daemon's config.toml

--json prints the daemon's answer unchanged (not for identify).
Writes need the token: THERMALIQ_TOKEN, else token.txt in THERMALIQ_DATA_DIR or
%ProgramData%\\ThermalIQ (run elevated or as the user who ran install-service).
Exit codes: 0 ok, 1 error, 2 daemon unreachable, 3 token, 4 daemon refused it, 5 status unhealthy.";

const DEFAULT_OVERRIDE_SECS: u64 = 60;
const DEFAULT_HISTORY_MINS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Success = 0,
    Error = 1,
    Unreachable = 2,
    Token = 3,
    Rejected = 4,
    Unhealthy = 5,
}

#[derive(Clone, Debug)]
pub struct CliError {
    pub code: Exit,
    pub msg: String,
}

impl CliError {
    pub fn new(code: Exit, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }
}

impl From<String> for CliError {
    fn from(msg: String) -> Self {
        Self::new(Exit::Error, msg)
    }
}

impl From<&str> for CliError {
    fn from(msg: &str) -> Self {
        Self::new(Exit::Error, msg)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

type Res<T> = Result<T, CliError>;

/// Everything `tq` reads from the process, so tests can set it.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `THERMALIQ_TOKEN`.
    pub token: Option<String>,
    /// `THERMALIQ_DATA_DIR`.
    pub data_dir: Option<OsString>,
    /// `%ProgramData%`.
    pub program_data: Option<OsString>,
    /// Whether stdin is a terminal (only changes a prompt).
    pub tty: bool,
}

impl Env {
    pub fn from_process() -> Self {
        Self {
            token: std::env::var("THERMALIQ_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
            data_dir: std::env::var_os(DATA_DIR_ENV),
            program_data: std::env::var_os("ProgramData"),
            tty: std::io::stdin().is_terminal(),
        }
    }

    fn dir(&self) -> std::io::Result<std::path::PathBuf> {
        data_dir_from(self.data_dir.clone(), self.program_data.clone())
    }

    /// `api_port` from `config.toml`. The port is not a secret, so a missing or unreadable file
    /// (a standard user may not read the data directory) means the default; a file that does
    /// not parse is an error.
    fn port(&self) -> Res<u16> {
        let text = self
            .dir()
            .ok()
            .and_then(|d| fs::read_to_string(d.join("config.toml")).ok());
        match text {
            Some(t) => DaemonConfig::from_toml(&t)
                .map(|c| c.api_port)
                .map_err(|e| CliError::from(format!("config.toml: {e}"))),
            None => Ok(DaemonConfig::default().api_port),
        }
    }

    /// `THERMALIQ_TOKEN` wins; else `token.txt`. Never taken from the command line, where the
    /// process list would show it.
    fn token(&self) -> Res<String> {
        let fix = "run tq elevated or as the installing user, or set THERMALIQ_TOKEN";
        let bad = |m: String| CliError::new(Exit::Token, m);
        if let Some(t) = &self.token {
            return parse_token(t)
                .ok_or_else(|| bad("THERMALIQ_TOKEN is not a 64-digit hex token".into()));
        }
        let path = self
            .dir()
            .map_err(|e| bad(format!("{e}: {fix}")))?
            .join("token.txt");
        let raw = fs::read_to_string(&path)
            .map_err(|e| bad(format!("cannot read {} ({e}): {fix}", path.display())))?;
        parse_token(&raw)
            .ok_or_else(|| bad(format!("{} holds no valid token: {fix}", path.display())))
    }

    fn client(&self) -> Res<Client> {
        let env = self.clone();
        Ok(Client::new(self.port()?, move || env.token()))
    }
}

/// Runs one `tq` command. Returns the process exit code.
pub fn run(
    args: &[String],
    env: &Env,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    match dispatch(args, env, input, out, err) {
        Ok(code) => code as i32,
        Err(e) => {
            let _ = writeln!(err, "tq: {}", e.msg);
            e.code as i32
        }
    }
}

fn usage<T>(why: &str) -> Res<T> {
    Err(CliError::new(Exit::Error, format!("{why}\n{USAGE}")))
}

struct Parsed<'a> {
    words: Vec<&'a str>,
    json: bool,
    flags: Vec<(&'a str, &'a str)>,
}

impl Parsed<'_> {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|f| f.0 == name).map(|f| f.1)
    }
}

/// Each command allows only its own options, so `tq status --cancel` is an error, not ignored.
fn parse(args: &[String]) -> Res<Parsed<'_>> {
    let first = args
        .iter()
        .map(String::as_str)
        .find(|a| !a.starts_with("--"));
    let allowed: &[&str] = match first {
        Some("history") => &["--mins"],
        Some("override") => &["--secs", "--cancel"],
        Some("identify") => &["--probe"],
        _ => &[],
    };
    let mut p = Parsed {
        words: Vec::new(),
        json: false,
        flags: Vec::new(),
    };
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "--json" => p.json = true,
            f if f.starts_with("--") => {
                if !allowed.contains(&f) {
                    return usage(&format!("unknown option {f}"));
                }
                if f == "--cancel" {
                    p.flags.push((f, ""));
                } else if let Some(v) = it.next() {
                    p.flags.push((f, v));
                } else {
                    return usage(&format!("{f} needs a value"));
                }
            }
            w => p.words.push(w),
        }
    }
    Ok(p)
}

fn say(out: &mut dyn Write, text: &str) {
    let _ = write!(out, "{text}");
}

/// The daemon's answer as JSON when asked, else the readable form.
fn show(out: &mut dyn Write, json: bool, v: &Value, text: &str) {
    if json {
        say(
            out,
            &format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_default()),
        );
    } else {
        say(out, text);
    }
}

fn read(
    out: &mut dyn Write,
    json: bool,
    v: Res<Value>,
    render: impl Fn(&Value) -> String,
) -> Res<Exit> {
    let v = v?;
    show(out, json, &v, &render(&v));
    Ok(Exit::Success)
}

fn done(out: &mut dyn Write, json: bool, v: &Value, line: &str) -> Res<Exit> {
    show(out, json, v, &format!("{line}\n"));
    Ok(Exit::Success)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn positive(flag: &str, text: &str) -> Res<u64> {
    text.parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{flag} must be a whole number above 0").into())
}

fn dispatch(
    args: &[String],
    env: &Env,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Res<Exit> {
    let first = args.iter().find(|a| !a.starts_with("--"));
    if first.is_some_and(|a| a == "help")
        || args.iter().any(|a| matches!(a.as_str(), "--help" | "-h"))
    {
        say(out, &format!("{USAGE}\n"));
        return Ok(Exit::Success);
    }
    let p = parse(args)?;
    let json = p.json;
    match p.words.as_slice() {
        ["status"] => {
            let v = env.client()?.get("/status")?;
            show(out, json, &v, &render::status(&v));
            let healthy =
                v["engine_running"] == true && v["degraded"] != true && v["latched"] != true;
            Ok(if healthy {
                Exit::Success
            } else {
                Exit::Unhealthy
            })
        }
        ["sensors"] => read(out, json, env.client()?.get("/sensors"), |v| {
            render::array_table(
                v,
                &["ID", "LABEL", "KIND", "TEMP_C"],
                &["id", "label", "kind", "temp_celsius"],
            )
        }),
        ["fans"] => read(out, json, env.client()?.get("/fans"), |v| {
            render::array_table(
                v,
                &[
                    "ID", "LABEL", "KIND", "RPM", "DUTY%", "CURVE%", "AGENT%", "SENT%",
                ],
                &[
                    "id",
                    "label",
                    "kind",
                    "rpm",
                    "duty_percent",
                    "curve_duty",
                    "agent_duty",
                    "commanded_duty",
                ],
            )
        }),
        ["metrics"] => read(out, json, env.client()?.get("/metrics"), |v| {
            render::array_table(v, &["ID", "KIND", "VALUE"], &["id", "kind", "value"])
        }),
        ["identity"] => read(out, json, env.client()?.get("/identity"), render::identity),
        ["config", "show"] => read(out, json, env.client()?.get("/config"), render::config),
        ["profile", "list"] => read(out, json, env.client()?.get("/profiles"), render::profiles),
        ["profile", "set", name] => {
            let v = env
                .client()?
                .post("/profiles/active", Some(json!({ "name": name })))?;
            done(
                out,
                json,
                &v,
                &format!(
                    "chosen: {}\neffective: {}",
                    render::cell(&v["chosen"]),
                    render::winner(&v["effective"], &v["source"], &v["rule_id"])
                ),
            )
        }
        ["history", sensor] => {
            let mins = p
                .flag("--mins")
                .map_or(Ok(DEFAULT_HISTORY_MINS), |m| positive("--mins", m))?;
            let v = env
                .client()?
                .get(&format!("/history/sensors/{}", enc(sensor)))?;
            let now = now();
            let since =
                now.saturating_sub(i64::try_from(mins.saturating_mul(60)).unwrap_or(i64::MAX));
            let rows: Vec<Value> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["t"].as_i64().is_some_and(|t| t >= since))
                .cloned()
                .collect();
            let table = render::table(
                &["WHEN", "TEMP_C"],
                &rows
                    .iter()
                    .map(|r| {
                        vec![
                            r["t"]
                                .as_i64()
                                .map_or_else(|| "-".into(), |t| render::ago(now, t)),
                            render::cell(&r["temp_celsius"]),
                        ]
                    })
                    .collect::<Vec<_>>(),
            );
            show(out, json, &Value::Array(rows), &table);
            Ok(Exit::Success)
        }
        ["override", fan] if p.flag("--cancel").is_some() && p.flag("--secs").is_none() => {
            let v = env
                .client()?
                .delete(&format!("/fans/{}/override", enc(fan)))?;
            done(out, json, &v, &format!("{fan}: override cancelled"))
        }
        ["override", fan, duty] if p.flag("--cancel").is_none() => {
            let duty = duty
                .parse::<u8>()
                .ok()
                .filter(|d| *d <= 100)
                .ok_or_else(|| CliError::from("duty must be a whole number from 0 to 100"))?;
            let secs = p
                .flag("--secs")
                .map_or(Ok(DEFAULT_OVERRIDE_SECS), |s| positive("--secs", s))?;
            let v = env.client()?.post(
                &format!("/fans/{}/override", enc(fan)),
                Some(json!({ "duty": duty, "seconds": secs })),
            )?;
            if v["in_effect"] == true {
                done(
                    out,
                    json,
                    &v,
                    &format!(
                        "{fan}: {}% for {} s",
                        render::cell(&v["duty"]),
                        render::cell(&v["seconds"])
                    ),
                )
            } else {
                show(
                    out,
                    json,
                    &v,
                    &format!("{fan}: not in effect: {}\n", render::cell(&v["reason"])),
                );
                Ok(Exit::Rejected)
            }
        }
        ["agent", "pause"] => {
            let v = env.client()?.post("/agent/pause", None)?;
            done(
                out,
                json,
                &v,
                &format!("agent: {}", render::cell(&v["agent"])),
            )
        }
        ["agent", "resume"] => {
            let v = env.client()?.post("/agent/resume", None)?;
            done(
                out,
                json,
                &v,
                &format!("agent: {}", render::cell(&v["agent"])),
            )
        }
        ["agent", "decisions"] => read(
            out,
            json,
            env.client()?.get_authed("/agent/decisions"),
            |v| render::decisions(v, now()),
        ),
        ["agent", "key", "set", provider] => {
            let c = env.client()?;
            c.token()?;
            let mut line = String::new();
            {
                // Echo is off only while the line is read, and comes back on every path out.
                let _quiet = env.tty.then(echo::off).flatten();
                if env.tty {
                    let _ = write!(err, "Paste the key and press Enter (not shown): ");
                }
                let read = input.read_line(&mut line);
                if env.tty {
                    let _ = writeln!(err);
                }
                read.map_err(|e| CliError::from(format!("cannot read the key: {e}")))?;
            }
            let key = line.trim();
            if key.is_empty() {
                return Err("no key on stdin: pipe it in or type it and press Enter".into());
            }
            let v = c.put("/agent/key", json!({ "provider": provider, "key": key }))?;
            done(
                out,
                json,
                &v,
                &format!(
                    "hosted key stored for {}",
                    render::cell(&v["hosted_account"])
                ),
            )
        }
        ["agent", "key", "clear"] => {
            let v = env.client()?.delete("/agent/key")?;
            done(out, json, &v, "hosted key removed")
        }
        ["agent", "workload", exe, kind] => {
            let v = env.client()?.put(
                &format!("/agent/workloads/{}", enc(exe)),
                json!({ "workload": kind }),
            )?;
            done(
                out,
                json,
                &v,
                &format!(
                    "{}: {} (profile {})",
                    render::cell(&v["exe"]),
                    render::cell(&v["workload"]),
                    render::cell(&v["profile"])
                ),
            )
        }
        ["inventory", "accept"] => {
            let v = env.client()?.post("/inventory/accept", None)?;
            done(out, json, &v, "inventory saved")
        }
        ["identify"] => {
            if json {
                return usage("identify has no --json mode");
            }
            let c = env.client()?;
            // Before any fan runs: a token that fails at the save would waste the whole walk.
            c.token()?;
            match p.flag("--probe") {
                Some(fan) => identify::run_probe(&c, fan, out)?,
                None => identify::run(&c, input, out)?,
            }
            Ok(Exit::Success)
        }
        _ => usage("unknown or incomplete command"),
    }
}
