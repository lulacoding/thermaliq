//! The two decision providers on the wire (SPEC §7.2): hosted Jev with the user's key, and a
//! local model on loopback, started for one call and unloaded after it.

use crate::keychain::KeyStore;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::net::{SocketAddr, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use thermaliq_agent::decision::{Answer, AskError, DecisionProvider, Question};

/// SPEC §7.2: hosted is only used if it answers within this.
const HOSTED_TIMEOUT: Duration = Duration::from_secs(2);
/// A load took 37 s on one core in the spike; a call that takes this long is not coming back.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BODY: u64 = 1024 * 1024;
pub const LOCAL_CONFIG_FILE: &str = "local-model.toml";
/// Nothing the child needs, and no key should sit in its environment.
const ENV_REMOVED: [&str; 3] = [
    "OPENROUTER_API_KEY",
    "TYPESAFE_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
];

/// The request body. The state is an exe name and numbers, by construction (SPEC §7.2).
fn wire_body(q: &Question, model: Option<&str>) -> Value {
    let s = &q.state;
    let mut state = Map::new();
    state.insert("exe".into(), json!(s.exe));
    state.insert("seconds_observed".into(), json!(s.seconds_observed));
    let readings = [
        ("avg_cpu_load_pct", s.cpu_load_pct),
        ("avg_gpu_load_pct", s.gpu_load_pct),
        ("avg_cpu_power_w", s.cpu_power_w),
        ("avg_gpu_power_w", s.gpu_power_w),
        ("avg_cpu_temp_c", s.cpu_temp_c),
        ("avg_gpu_temp_c", s.gpu_temp_c),
    ];
    for (name, value) in readings {
        if let Some(v) = value {
            state.insert(name.into(), json!((v * 10.0).round() / 10.0));
        }
    }
    let criteria: Map<String, Value> = q
        .options
        .iter()
        .map(|(name, what)| (name.clone(), json!(what)))
        .collect();
    let mut questions = Map::new();
    questions.insert(
        q.id.into(),
        json!({"type": "choice", "instructions": q.instructions, "criteria": criteria}),
    );
    let mut body = json!({"state": Value::Object(state).to_string(), "questions": questions});
    if let Some(m) = model {
        body["model"] = json!(m);
    }
    body
}

/// Fails on anything but a choice and a numeric confidence for this question. The caller
/// checks the choice against the options.
fn parse_answer(question_id: &str, body: &str) -> Result<Answer, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the response is not JSON".to_owned())?;
    let a = &v["answers"][question_id];
    match (a["choice"].as_str(), a["confidence"].as_f64()) {
        (Some(choice), Some(confidence)) => Ok(Answer {
            choice: choice.into(),
            confidence,
        }),
        _ => Err("the response has no choice and confidence for this question".into()),
    }
}

/// Reads the key per call, so adding or removing one takes effect at once.
pub struct HostedProvider {
    keys: Arc<dyn KeyStore>,
    last_error: Arc<Mutex<Option<String>>>,
    #[cfg(test)]
    test_url: Option<String>,
}

impl HostedProvider {
    pub fn new(keys: Arc<dyn KeyStore>) -> Self {
        Self {
            keys,
            last_error: Arc::default(),
            #[cfg(test)]
            test_url: None,
        }
    }

    /// Where to keep why the last call failed, for the API (S8: a silent fallback hides a bad
    /// key). `None` after a success.
    #[must_use]
    pub fn reporting_to(mut self, last_error: Arc<Mutex<Option<String>>>) -> Self {
        self.last_error = last_error;
        self
    }

    #[cfg(test)]
    fn with_test_url(mut self, url: String) -> Self {
        self.test_url = Some(url);
        self
    }

    #[cfg(test)]
    fn url(&self, account: crate::keychain::Account) -> String {
        self.test_url
            .clone()
            .unwrap_or_else(|| account.url().into())
    }

    #[cfg(not(test))]
    fn url(&self, account: crate::keychain::Account) -> String {
        account.url().into()
    }
}

impl DecisionProvider for HostedProvider {
    fn ask(&mut self, q: &Question, _: &dyn Fn() -> bool) -> Result<Answer, AskError> {
        let result = self.call(q);
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = match &result {
            Err(AskError::Unavailable(why)) => Some(why.clone()),
            _ => None,
        };
        result
    }
}

impl HostedProvider {
    fn call(&self, q: &Question) -> Result<Answer, AskError> {
        let unavailable = AskError::Unavailable;
        let key = self
            .keys
            .get()
            .map_err(|_| unavailable("the keychain could not be read".into()))?
            .ok_or_else(|| unavailable("no key".into()))?;
        // No redirects: the key must only ever reach the account's own URL.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(HOSTED_TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent
            .post(self.url(key.account))
            .header("Authorization", format!("Bearer {}", key.secret()))
            .header("Content-Type", "application/json")
            .send(wire_body(q, Some(key.account.model())).to_string())
            // The transport's reason names the host at most: headers aren't part of it.
            .map_err(|e| unavailable(format!("the request failed: {e}")))?;
        let status = response.status().as_u16();
        if status != 200 {
            let text = response
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_string()
                .unwrap_or_default();
            // Only its error message, short: it is the provider's text, and may echo what we sent.
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                .map(|m| m.replace(key.secret(), "[key]"))
                .map(|m| m.chars().take(200).collect::<String>());
            return Err(unavailable(match message {
                Some(m) => format!("HTTP {status}: {m}"),
                None => format!("HTTP {status}"),
            }));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(|_| unavailable("the response could not be read".into()))?;
        parse_answer(q.id, &body).map_err(unavailable)
    }
}

/// `local-model.toml` in the data directory. It is not settable through the API: the service
/// runs this exe as SYSTEM, so only someone who can write that folder may name it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    pub exe: String,
    #[serde(default = "default_args")]
    pub args: Vec<String>,
    pub model: String,
    /// Loopback only: there is no way to point it at another machine.
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_args() -> Vec<String> {
    vec!["serve".into()]
}

const fn default_port() -> u16 {
    11435
}

impl LocalConfig {
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        let text = match std::fs::read_to_string(dir.join(LOCAL_CONFIG_FILE)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{LOCAL_CONFIG_FILE}: {e}")),
        };
        let c: Self = toml::from_str(&text).map_err(|e| format!("{LOCAL_CONFIG_FILE}: {e}"))?;
        if c.exe.is_empty() || c.model.is_empty() || c.port == 0 {
            return Err(format!(
                "{LOCAL_CONFIG_FILE}: exe and model are required, and port must not be 0"
            ));
        }
        Ok(Some(c))
    }

    /// A service must not run something a standard user could replace.
    pub fn check(&self, service: bool, program_files: Option<&Path>) -> Result<(), String> {
        if !service {
            return Ok(());
        }
        let exe = PathBuf::from(self.exe.to_lowercase());
        let root = program_files
            .ok_or("%ProgramFiles% is not set")?
            .to_string_lossy()
            .to_lowercase();
        let climbs = exe.components().any(|c| c == Component::ParentDir);
        if !exe.is_absolute() || climbs || !exe.starts_with(&root) {
            return Err(format!(
                "the exe must be under {root} when run by the service"
            ));
        }
        Ok(())
    }

    /// `check`, and for a service, that nobody but an administrator could replace the exe or the
    /// DLLs beside it. Some installers leave a Program Files folder writable, and whoever swaps
    /// the exe then runs as SYSTEM. M21 should also drop the rights the model runs with.
    pub fn verify(&self, service: bool, program_files: Option<&Path>) -> Result<(), String> {
        self.check(service, program_files)?;
        #[cfg(windows)]
        if let (true, Some(root)) = (service, program_files) {
            // Lower case on both sides, as in `check`: these paths are case-insensitive.
            let exe = PathBuf::from(self.exe.to_lowercase());
            let root = PathBuf::from(root.to_string_lossy().to_lowercase());
            let problems = crate::acl::replaceable_by_others(&exe, &root)
                .map_err(|e| format!("cannot check who could replace the exe: {e}"))?;
            if !problems.is_empty() {
                return Err(format!(
                    "others could replace the exe: {}",
                    problems.join("; ")
                ));
            }
        }
        Ok(())
    }
}

/// SPEC §7.2: when idle, up to half the cores. Counted as logical processors, the unit these
/// engines' thread settings use.
fn child_env(logical: usize, data_dir: &Path, port: u16) -> Vec<(&'static str, String)> {
    let threads = (logical / 2).max(1).to_string();
    vec![
        ("OLLAYA_THREADS", threads.clone()),
        ("OMP_NUM_THREADS", threads),
        ("OLLAYA_DEVICE", "cpu".into()),
        ("OLLAYA_HOST", format!("127.0.0.1:{port}")),
        (
            "OLLAYA_MODELS",
            data_dir.join("models").display().to_string(),
        ),
    ]
}

/// The process and everything it started. Dropping it unloads the model.
struct Running {
    child: Child,
    #[cfg(windows)]
    _job: Option<crate::win::Job>,
}

impl Drop for Running {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            self._job = None; // closing the job kills the whole tree
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct LocalProvider {
    config: LocalConfig,
    data_dir: PathBuf,
    /// Set for the service: `%ProgramFiles%`, to check the exe again before every start.
    service_root: Option<PathBuf>,
    /// Only tests read it: to see the process is gone.
    pub last_pid: Option<u32>,
}

impl LocalProvider {
    pub const fn new(config: LocalConfig, data_dir: PathBuf) -> Self {
        Self {
            config,
            data_dir,
            service_root: None,
            last_pid: None,
        }
    }

    /// The ACLs may have changed since startup (an update, a reinstall), and this runs as SYSTEM.
    #[must_use]
    pub fn for_service(mut self, program_files: PathBuf) -> Self {
        self.service_root = Some(program_files);
        self
    }

    fn start(&self) -> std::io::Result<Running> {
        let c = &self.config;
        let logical = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
        let mut cmd = Command::new(&c.exe);
        cmd.args(&c.args)
            .envs(child_env(logical, &self.data_dir, c.port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for name in ENV_REMOVED {
            cmd.env_remove(name);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW; children inherit the priority.
            cmd.creation_flags(0x0000_4000 | 0x0800_0000);
        }
        let child = cmd.spawn()?;
        #[cfg(windows)]
        {
            let _job = crate::win::Job::containing(&child).ok();
            Ok(Running { child, _job })
        }
        #[cfg(not(windows))]
        Ok(Running { child })
    }
}

fn port_in_use(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// Posts until the server answers, while it starts up. Ends when `stop` is set or at the deadline.
fn post_until_answered(url: &str, body: &str, stop: &AtomicBool) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(LOCAL_TIMEOUT))
        .max_redirects(0)
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into();
    let deadline = Instant::now() + LOCAL_TIMEOUT;
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        let sent = agent
            .post(url)
            .header("Content-Type", "application/json")
            .send(body);
        match sent {
            Ok(mut r) if r.status() == 200 => {
                return r
                    .body_mut()
                    .with_config()
                    .limit(MAX_BODY)
                    .read_to_string()
                    .map_err(|_| "the response could not be read".to_owned());
            }
            // Not up yet: nothing listening, or 503 while the model loads.
            Ok(r) if r.status() != 503 => return Err(format!("HTTP {}", r.status())),
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    Err("no answer in time".into())
}

impl DecisionProvider for LocalProvider {
    fn ask(&mut self, q: &Question, cancel: &dyn Fn() -> bool) -> Result<Answer, AskError> {
        if cancel() {
            return Err(AskError::Cancelled);
        }
        if let Some(root) = &self.service_root {
            self.config
                .verify(true, Some(root))
                .map_err(AskError::Unavailable)?;
        }
        // The local server has no authentication on 127.0.0.1, so whatever already listens here
        // could be anyone's: a squatter must not be able to feed us answers. Refuse it, and only
        // talk to a port that was free before we started our own process.
        if port_in_use(self.config.port) {
            return Err(AskError::Unavailable(format!(
                "something already listens on port {}",
                self.config.port
            )));
        }
        let mut running = self
            .start()
            .map_err(|e| AskError::Unavailable(format!("could not start: {e}")))?;
        self.last_pid = Some(running.child.id());
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let url = format!("http://127.0.0.1:{}/v1/systemone", self.config.port);
        let body = wire_body(q, Some(&self.config.model)).to_string();
        let worker_stop = stop.clone();
        std::thread::spawn(move || {
            let _ = tx.send(post_until_answered(&url, &body, &worker_stop));
        });
        let result = loop {
            if cancel() {
                break Err(AskError::Cancelled);
            }
            if !matches!(running.child.try_wait(), Ok(None)) {
                break Err(AskError::Unavailable("the server exited".into()));
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(text)) => break parse_answer(q.id, &text).map_err(AskError::Unavailable),
                Ok(Err(e)) => break Err(AskError::Unavailable(e)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(AskError::Unavailable("the request stopped".into()));
                }
            }
        };
        stop.store(true, Ordering::SeqCst);
        drop(running); // unload, and a blocked request fails with it
        result
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::keychain::{Account, Key, MemoryKeys};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use thermaliq_agent::decision::{workload_question, ExeState};

    const GOOD: &str = r#"{"model":"m","answers":{"workload":{"type":"choice","choice":"gaming","confidence":0.91,"probabilities":{}}},"usage":{}}"#;

    fn state() -> ExeState {
        ExeState {
            exe: "cs2.exe".into(),
            seconds_observed: 12,
            cpu_load_pct: Some(40.5),
            gpu_load_pct: None,
            cpu_power_w: None,
            gpu_power_w: Some(90.0),
            cpu_temp_c: Some(60.0),
            gpu_temp_c: None,
        }
    }

    /// Answers each request with `reply`, keeping the raw requests. `after` delays binding,
    /// so the port is free when a local server would still be starting.
    fn server_on(port: u16, reply: String, after: Duration) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
            for s in l.incoming() {
                let mut s = s.unwrap();
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let (mut raw, mut buf) = (Vec::new(), [0u8; 4096]);
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&raw);
                    if let Some(h) = t.find("\r\n\r\n") {
                        let len = t[..h]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if raw.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).into_owned());
                let _ = s.write_all(reply.as_bytes());
            }
        });
        seen
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn hosted_to(port: u16, keys: Arc<MemoryKeys>) -> HostedProvider {
        HostedProvider::new(keys).with_test_url(format!("http://127.0.0.1:{port}/v1/systemone"))
    }

    fn keys() -> Arc<MemoryKeys> {
        let k = Arc::new(MemoryKeys::default());
        k.set(&Key::new(Account::OpenRouter, "sk-or-secret").unwrap())
            .unwrap();
        k
    }

    fn never() -> bool {
        false
    }

    // ---- the wire format ----

    #[test]
    fn the_body_holds_numbers_and_the_exe_name_and_nothing_else() {
        let q = workload_question(state());
        let body = wire_body(&q, Some("typesafe/jev-1.14"));
        assert_eq!(body["model"], "typesafe/jev-1.14");
        let st: serde_json::Value = serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
        let obj = st.as_object().unwrap();
        let mut keys: Vec<_> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "avg_cpu_load_pct",
                "avg_cpu_temp_c",
                "avg_gpu_power_w",
                "exe",
                "seconds_observed"
            ],
            "missing readings are left out, not sent as 0"
        );
        for (k, v) in obj {
            assert!(k == "exe" || v.is_number(), "{k}: {v}");
        }
        assert_eq!(obj["exe"], "cs2.exe");
        let question = &body["questions"]["workload"];
        assert_eq!(question["type"], "choice");
        assert_eq!(question["criteria"].as_object().unwrap().len(), 6);
    }

    #[test]
    fn a_response_is_read_strictly() {
        assert_eq!(
            parse_answer("workload", GOOD).unwrap(),
            Answer {
                choice: "gaming".into(),
                confidence: 0.91
            }
        );
        for bad in [
            "not json",
            "{}",
            r#"{"answers":{"workload":{"choice":"gaming"}}}"#,
            r#"{"answers":{"workload":{"confidence":0.5}}}"#,
            r#"{"answers":{"other":{"choice":"gaming","confidence":0.5}}}"#,
        ] {
            assert!(parse_answer("workload", bad).is_err(), "{bad}");
        }
    }

    // ---- hosted ----

    #[test]
    fn hosted_sends_the_key_and_pinned_model_and_reads_the_answer() {
        let port = free_port();
        let seen = server_on(port, http("200 OK", GOOD), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(100));
        let mut p = hosted_to(port, keys());
        let a = p.ask(&workload_question(state()), &never).unwrap();
        assert_eq!(a.choice, "gaming");
        let raw = seen.lock().unwrap()[0].clone();
        assert!(raw.contains("Bearer sk-or-secret"), "{raw}");
        assert!(raw.contains("typesafe/jev-1.14"), "{raw}");
    }

    #[test]
    fn no_key_means_no_request() {
        let port = free_port();
        let seen = server_on(port, http("200 OK", GOOD), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(100));
        let mut p = hosted_to(port, Arc::new(MemoryKeys::default()));
        assert!(matches!(
            p.ask(&workload_question(state()), &never),
            Err(AskError::Unavailable(_))
        ));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rejected_key_reports_the_status_and_never_the_key_or_the_body() {
        let port = free_port();
        server_on(
            port,
            http("401 Unauthorized", r#"{"error":"bad key sk-or-secret"}"#),
            Duration::ZERO,
        );
        std::thread::sleep(Duration::from_millis(100));
        let mut p = hosted_to(port, keys());
        let Err(AskError::Unavailable(why)) = p.ask(&workload_question(state()), &never) else {
            panic!("expected a failure");
        };
        assert!(why.contains("401"), "{why}");
        assert!(!why.contains("sk-or-secret"), "{why}");
    }

    #[test]
    fn a_rejection_carries_the_providers_own_message_with_the_key_hidden() {
        let port = free_port();
        server_on(
            port,
            http(
                "401 Unauthorized",
                r#"{"error":{"message":"bad key sk-or-secret","code":401}}"#,
            ),
            Duration::ZERO,
        );
        std::thread::sleep(Duration::from_millis(100));
        let Err(AskError::Unavailable(why)) =
            hosted_to(port, keys()).ask(&workload_question(state()), &never)
        else {
            panic!("expected a failure");
        };
        assert_eq!(why, "HTTP 401: bad key [key]");
    }

    #[test]
    fn a_huge_provider_message_is_cut_short() {
        let msg = "x".repeat(5000);
        let port = free_port();
        server_on(
            port,
            http("500 Oops", &format!(r#"{{"error":{{"message":"{msg}"}}}}"#)),
            Duration::ZERO,
        );
        std::thread::sleep(Duration::from_millis(100));
        let Err(AskError::Unavailable(why)) =
            hosted_to(port, keys()).ask(&workload_question(state()), &never)
        else {
            panic!("expected a failure");
        };
        assert!(why.len() < 300, "{}", why.len());
    }

    #[test]
    fn the_last_failure_is_reported_without_the_key_and_a_success_clears_it() {
        let status: Arc<Mutex<Option<String>>> = Arc::default();
        let read = || status.lock().unwrap().clone();
        let q = workload_question(state());

        let mut none =
            hosted_to(free_port(), Arc::new(MemoryKeys::default())).reporting_to(status.clone());
        assert!(none.ask(&q, &never).is_err());
        assert_eq!(read().as_deref(), Some("no key"));

        let port = free_port();
        server_on(
            port,
            http("401 Unauthorized", r#"{"error":"bad key sk-or-secret"}"#),
            Duration::ZERO,
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(hosted_to(port, keys())
            .reporting_to(status.clone())
            .ask(&q, &never)
            .is_err());
        assert_eq!(read().as_deref(), Some("HTTP 401"));

        // nothing listening: the transport's own reason, which must not carry the key
        assert!(hosted_to(free_port(), keys())
            .reporting_to(status.clone())
            .ask(&q, &never)
            .is_err());
        let why = read().unwrap();
        assert!(why.starts_with("the request failed: "), "{why}");
        assert!(!why.contains("sk-or-secret"), "{why}");

        let port = free_port();
        server_on(port, http("200 OK", GOOD), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(100));
        assert!(hosted_to(port, keys())
            .reporting_to(status.clone())
            .ask(&q, &never)
            .is_ok());
        assert_eq!(read(), None);
    }

    #[test]
    fn a_redirect_is_not_followed_so_the_key_goes_nowhere_else() {
        let (first, second) = (free_port(), free_port());
        let elsewhere = server_on(second, http("200 OK", GOOD), Duration::ZERO);
        let redirect = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{second}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        server_on(first, redirect, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(100));
        let mut p = hosted_to(first, keys());
        assert!(p.ask(&workload_question(state()), &never).is_err());
        std::thread::sleep(Duration::from_millis(100));
        assert!(elsewhere.lock().unwrap().is_empty());
    }

    #[test]
    fn hosted_gives_up_after_two_seconds() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap(); // accepts in the backlog, never answers
        let port = l.local_addr().unwrap().port();
        let mut p = hosted_to(port, keys());
        let t0 = Instant::now();
        assert!(p.ask(&workload_question(state()), &never).is_err());
        let took = t0.elapsed();
        assert!(took >= Duration::from_millis(1800), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    // ---- local: configuration ----

    #[cfg(windows)]
    fn config(exe: &str) -> LocalConfig {
        LocalConfig {
            exe: exe.into(),
            args: vec!["serve".into()],
            model: "strands-decider:2b".into(),
            port: 11435,
        }
    }

    #[test]
    fn the_config_file_is_optional_and_strict() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(LocalConfig::load(d.path()).unwrap(), None);
        let f = d.path().join(LOCAL_CONFIG_FILE);
        std::fs::write(&f, "exe = 'x'\nmodel = 'm'\n").unwrap();
        let c = LocalConfig::load(d.path()).unwrap().unwrap();
        assert_eq!((c.port, c.args.len()), (11435, 1));
        std::fs::write(&f, "exe = 'x'\nmodel = 'm'\nurl = 'http://evil'\n").unwrap();
        assert!(LocalConfig::load(d.path()).is_err(), "no unknown fields");
        std::fs::write(&f, "exe = 'x'\nmodel = 'm'\nport = 0\n").unwrap();
        assert!(LocalConfig::load(d.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_service_only_runs_a_local_model_from_a_protected_folder() {
        let pf = std::path::Path::new("C:\\Program Files");
        let ok = |exe: &str, service| config(exe).check(service, Some(pf)).is_ok();
        assert!(ok("C:\\Program Files\\Ollaya\\ollaya.exe", true));
        assert!(ok("c:\\program files\\Ollaya\\ollaya.exe", true));
        assert!(!ok("C:\\Users\\sam\\AppData\\Local\\ollaya.exe", true));
        assert!(!ok("C:\\Program Files\\..\\Users\\sam\\ollaya.exe", true));
        assert!(!ok("ollaya.exe", true), "must be absolute");
        assert!(!ok("C:\\Program Files Evil\\ollaya.exe", true));
        assert!(
            config("C:\\Program Files\\o.exe")
                .check(true, None)
                .is_err(),
            "unknown Program Files: refuse"
        );
        assert!(
            ok("C:\\Users\\sam\\ollaya.exe", false),
            "dev runs as the user"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_service_refuses_an_exe_others_can_replace_and_starts_nothing() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("App");
        std::fs::create_dir(&app).unwrap();
        let exe = app.join("model.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let cfg = config(&exe.to_string_lossy());
        // a temp folder is writable by whoever runs this, who is no Administrators-only owner
        let why = cfg.verify(true, Some(t.path())).unwrap_err();
        assert!(why.contains("replace"), "{why}");
        assert!(
            cfg.verify(false, Some(t.path())).is_ok(),
            "dev runs as the user"
        );
        let d = tempfile::tempdir().unwrap();
        let mut p = local(cfg, &d).for_service(t.path().to_owned());
        assert!(matches!(
            p.ask(&workload_question(state()), &never),
            Err(AskError::Unavailable(_))
        ));
        assert_eq!(p.last_pid, None, "checked again before every start");
    }

    #[test]
    fn the_child_gets_half_the_threads_the_cpu_and_no_key() {
        let env = child_env(12, std::path::Path::new("data"), 11435);
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(get("OLLAYA_THREADS"), "6");
        assert_eq!(get("OMP_NUM_THREADS"), "6");
        assert_eq!(get("OLLAYA_DEVICE"), "cpu");
        assert_eq!(get("OLLAYA_HOST"), "127.0.0.1:11435");
        assert!(get("OLLAYA_MODELS").contains("data"));
        assert_eq!(
            child_env(1, std::path::Path::new("d"), 1)[0].1,
            "1",
            "at least one"
        );
        assert!(ENV_REMOVED.contains(&"OPENROUTER_API_KEY"));
        assert!(ENV_REMOVED.contains(&"TYPESAFE_API_KEY"));
    }

    // ---- local: the process ----

    /// A child that just waits, standing in for the server (the fake server is the test's).
    fn sleeper() -> LocalConfig {
        if cfg!(windows) {
            LocalConfig {
                exe: "ping".into(),
                args: ["-n", "120", "127.0.0.1"].map(Into::into).to_vec(),
                model: "m".into(),
                port: free_port(),
            }
        } else {
            LocalConfig {
                exe: "sleep".into(),
                args: vec!["120".into()],
                model: "m".into(),
                port: free_port(),
            }
        }
    }

    fn alive(pid: u32) -> bool {
        let out = if cfg!(windows) {
            std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output()
        } else {
            std::process::Command::new("ps")
                .args(["-p", &pid.to_string()])
                .output()
        }
        .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }

    fn local(cfg: LocalConfig, d: &tempfile::TempDir) -> LocalProvider {
        LocalProvider::new(cfg, d.path().to_owned())
    }

    #[test]
    fn local_starts_answers_and_unloads() {
        let d = tempfile::tempdir().unwrap();
        let cfg = sleeper();
        let seen = server_on(cfg.port, http("200 OK", GOOD), Duration::from_millis(400));
        let mut p = local(cfg, &d);
        let a = p.ask(&workload_question(state()), &never).unwrap();
        assert_eq!(a.choice, "gaming");
        assert!(!alive(p.last_pid.unwrap()), "unloaded after use");
        assert!(seen.lock().unwrap()[0].contains("\"model\":\"m\""));
    }

    #[test]
    fn a_game_arriving_mid_call_stops_the_call_and_unloads() {
        let d = tempfile::tempdir().unwrap();
        let cfg = sleeper();
        // the model is still loading: nothing listens, so the call keeps retrying
        let mut p = local(cfg, &d);
        let game = Arc::new(AtomicBool::new(false));
        let flag = game.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::SeqCst);
        });
        let t0 = Instant::now();
        let cancel = || game.load(Ordering::SeqCst);
        assert_eq!(
            p.ask(&workload_question(state()), &cancel),
            Err(AskError::Cancelled)
        );
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert!(!alive(p.last_pid.unwrap()));
    }

    #[test]
    fn a_game_already_there_never_starts_the_process() {
        let d = tempfile::tempdir().unwrap();
        let mut p = local(sleeper(), &d);
        assert_eq!(
            p.ask(&workload_question(state()), &|| true),
            Err(AskError::Cancelled)
        );
        assert_eq!(p.last_pid, None);
    }

    #[test]
    fn a_server_that_was_already_there_is_not_trusted() {
        let d = tempfile::tempdir().unwrap();
        let cfg = sleeper();
        let seen = server_on(cfg.port, http("200 OK", GOOD), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(100));
        let mut p = local(cfg, &d);
        assert!(matches!(
            p.ask(&workload_question(state()), &never),
            Err(AskError::Unavailable(_))
        ));
        assert_eq!(p.last_pid, None);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            seen.lock().unwrap().iter().all(String::is_empty),
            "the port check only connects: nothing may be sent to that server"
        );
    }

    #[test]
    fn a_server_that_dies_is_reported_at_once() {
        let d = tempfile::tempdir().unwrap();
        let cfg = if cfg!(windows) {
            LocalConfig {
                exe: "cmd".into(),
                args: ["/c", "exit", "3"].map(Into::into).to_vec(),
                model: "m".into(),
                port: free_port(),
            }
        } else {
            LocalConfig {
                exe: "false".into(),
                args: vec![],
                model: "m".into(),
                port: free_port(),
            }
        };
        let mut p = local(cfg, &d);
        let t0 = Instant::now();
        assert!(matches!(
            p.ask(&workload_question(state()), &never),
            Err(AskError::Unavailable(_))
        ));
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    }

    #[test]
    fn a_missing_exe_is_reported_not_panicked() {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = sleeper();
        cfg.exe = "this-program-does-not-exist-xyz".into();
        let mut p = local(cfg, &d);
        assert!(matches!(
            p.ask(&workload_question(state()), &never),
            Err(AskError::Unavailable(_))
        ));
    }
}
