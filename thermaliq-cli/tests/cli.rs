//! `tq` against a fake daemon on loopback (PORTING M14). Nothing here touches a real daemon,
//! a real data directory or a fan.

#![allow(clippy::unwrap_used)]

use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use thermaliq_cli::{run, Env};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    auth: Option<String>,
    body: Value,
}

type Route = (&'static str, &'static str, u16, Value);

struct Fake {
    port: u16,
    log: Arc<Mutex<Vec<Req>>>,
}

impl Fake {
    fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                serve(conn, &routes, &seen);
            }
        });
        Self { port, log }
    }

    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(mut conn: TcpStream, routes: &[Route], log: &Mutex<Vec<Req>>) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap().to_owned(),
        parts.next().unwrap().to_owned(),
    );
    let (mut auth, mut len) = (None, 0usize);
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "authorization" => auth = Some(v.trim().to_owned()),
                "content-length" => len = v.trim().parse().unwrap(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    log.lock().unwrap().push(Req {
        method: method.clone(),
        path: path.clone(),
        auth,
        body,
    });
    let (status, reply) = routes
        .iter()
        .find(|r| r.0 == method && r.1 == path)
        .map_or((404, json!({"error": "no such route"})), |r| {
            (r.2, r.3.clone())
        });
    let text = reply.to_string();
    let _ = write!(
        conn,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
}

struct Rig {
    dir: tempfile::TempDir,
    fake: Fake,
}

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn rig(routes: Vec<Route>) -> Rig {
    let fake = Fake::start(routes);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!("api_port = {}\n", fake.port),
    )
    .unwrap();
    std::fs::write(dir.path().join("token.txt"), format!("{TOKEN}\n")).unwrap();
    Rig { dir, fake }
}

fn env_of(r: &Rig) -> Env {
    Env {
        token: None,
        data_dir: Some(r.dir.path().into()),
        program_data: None,
        tty: false,
    }
}

fn tq_with(env: &Env, args: &[&str], stdin: &str) -> Out {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&args, env, &mut stdin.as_bytes(), &mut out, &mut err);
    Out {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

fn tq(r: &Rig, args: &[&str]) -> Out {
    tq_with(&env_of(r), args, "")
}

fn status_body(degraded: bool, latched: bool, running: bool) -> Value {
    json!({
        "engine_running": running, "latched": latched, "degraded": degraded,
        "degraded_reasons": if degraded { json!(["profile 'balanced' controls no fans"]) } else { json!([]) },
        "waiting_on": [], "unavailable_sources": [], "snapshot_age": 0.4,
        "active_profile": "balanced", "ticks": 120, "agent": "running",
    })
}

fn ok(method: &'static str, path: &'static str, body: Value) -> Route {
    (method, path, 200, body)
}

// ---- reads, output and exit codes ----

#[test]
fn status_shows_the_state_and_exits_0_when_all_is_well() {
    let r = rig(vec![ok("GET", "/status", status_body(false, false, true))]);
    let o = tq(&r, &["status"]);
    assert_eq!(o.code, 0, "{}", o.err);
    for want in ["balanced", "running", "120"] {
        assert!(o.out.contains(want), "{want}: {}", o.out);
    }
}

#[test]
fn status_exits_5_when_degraded_latched_or_the_engine_is_not_running() {
    for (d, l, e) in [
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        let r = rig(vec![ok("GET", "/status", status_body(d, l, e))]);
        let o = tq(&r, &["status"]);
        assert_eq!(o.code, 5, "degraded={d} latched={l} running={e}");
    }
    let r = rig(vec![ok("GET", "/status", status_body(true, false, true))]);
    let o = tq(&r, &["status"]);
    assert!(
        o.out.contains("controls no fans"),
        "reasons are shown: {}",
        o.out
    );
}

#[test]
fn json_prints_the_daemons_answer_unchanged() {
    let body = status_body(false, false, true);
    let r = rig(vec![ok("GET", "/status", body.clone())]);
    let o = tq(&r, &["status", "--json"]);
    assert_eq!(serde_json::from_str::<Value>(&o.out).unwrap(), body);
    let o = tq(&r, &["--json", "status"]);
    assert_eq!(
        serde_json::from_str::<Value>(&o.out).unwrap(),
        body,
        "flag position is free"
    );
}

#[test]
fn sensors_fans_metrics_and_profiles_print_tables() {
    let r = rig(vec![
        ok(
            "GET",
            "/sensors",
            json!([{"id": "s1", "label": "CPU Package", "kind": "cpu", "temp_celsius": 51.5}]),
        ),
        ok(
            "GET",
            "/fans",
            json!([{"id": "f1", "label": "CPU Fan", "rpm": 812, "duty_percent": 40, "kind": "fan",
            "curve_duty": 38, "agent_duty": null, "commanded_duty": 40}]),
        ),
        ok(
            "GET",
            "/metrics",
            json!([{"id": "m1", "kind": "cpu_load", "value": 12.5}]),
        ),
        ok(
            "GET",
            "/profiles",
            json!([
            {"name": "balanced", "description": "Everyday use", "active": true},
            {"name": "silent", "description": "Quiet", "active": false}]),
        ),
    ]);
    for (args, wants) in [
        (vec!["sensors"], vec!["CPU Package", "51.5", "TEMP"]),
        (vec!["fans"], vec!["CPU Fan", "812", "40", "-"]),
        (vec!["metrics"], vec!["cpu_load", "12.5"]),
        (vec!["profile", "list"], vec!["balanced", "silent", "*"]),
    ] {
        let o = tq(&r, &args);
        assert_eq!(o.code, 0, "{args:?}: {}", o.err);
        for w in wants {
            assert!(o.out.contains(w), "{args:?} {w}: {}", o.out);
        }
    }
    assert!(
        r.fake.requests().iter().all(|q| q.auth.is_none()),
        "reads send no token"
    );
}

#[test]
fn identity_and_config_show_print_what_the_daemon_says() {
    let r = rig(vec![
        ok(
            "GET",
            "/identity",
            json!({
            "board": "TEST BOARD", "problems": ["role 'cpu_temp' is not mapped (profiles: balanced)"],
            "roles": {"cpu_fan": "h0", "case_fans": ["h1", "h2"]}, "unmapped_roles": ["pump"],
            "inventory": {"new": [], "missing": ["gone"], "changed": [], "board_changed": null},
            "proposals": [{"role": "pump", "id": "p0", "why": "a pump"}], "fans": [] }),
        ),
        ok(
            "GET",
            "/config",
            json!({"api_port": 7272, "default_profile": "balanced"}),
        ),
    ]);
    let o = tq(&r, &["identity"]);
    assert_eq!(o.code, 0, "{}", o.err);
    for w in [
        "TEST BOARD",
        "role 'cpu_temp' is not mapped",
        "case_fans",
        "h1, h2",
        "pump",
        "gone",
    ] {
        assert!(o.out.contains(w), "{w}: {}", o.out);
    }
    let o = tq(&r, &["config", "show"]);
    assert!(
        o.out.contains("api_port") && o.out.contains("7272"),
        "{}",
        o.out
    );
}

#[test]
fn history_asks_for_the_sensor_and_keeps_only_the_last_minutes() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (old, recent) = (now - 3600, now - 30);
    let r = rig(vec![ok(
        "GET",
        "/history/sensors/s1",
        json!([
        {"t": old, "temp_celsius": 99.0}, {"t": recent, "temp_celsius": 42.5}]),
    )]);
    let o = tq(&r, &["history", "s1", "--mins", "5"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("42.5") && !o.out.contains("99"), "{}", o.out);
    let o = tq(&r, &["history", "s1", "--mins", "120", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&o.out)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        tq(&r, &["history", "s1", "--mins", "0"]).code,
        1,
        "minutes must be positive"
    );
}

#[test]
fn an_id_with_path_characters_cannot_change_the_path() {
    let r = rig(vec![]);
    tq(&r, &["history", "a/b?c d"]);
    assert_eq!(r.fake.requests()[0].path, "/history/sensors/a%2Fb%3Fc%20d");
}

// ---- failures ----

#[test]
fn an_unreachable_daemon_is_exit_2_and_says_so() {
    let r = rig(vec![]);
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = dead.local_addr().unwrap().port();
    drop(dead);
    std::fs::write(
        r.dir.path().join("config.toml"),
        format!("api_port = {port}\n"),
    )
    .unwrap();
    let o = tq(&r, &["status"]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("cannot reach the daemon"), "{}", o.err);
}

#[test]
fn the_daemons_own_words_come_back_and_4xx_is_4_and_5xx_is_1() {
    let r = rig(vec![
        (
            "POST",
            "/profiles/active",
            404,
            json!({"error": "no profile 'nope'"}),
        ),
        (
            "POST",
            "/agent/pause",
            409,
            json!({"error": "the safety latch holds every fan at 100%"}),
        ),
        (
            "POST",
            "/agent/resume",
            503,
            json!({"error": "engine is not running"}),
        ),
        ("GET", "/fans", 500, json!(null)),
    ]);
    let o = tq(&r, &["profile", "set", "nope"]);
    assert_eq!(o.code, 4);
    assert!(o.err.contains("no profile 'nope'"), "{}", o.err);
    let o = tq(&r, &["agent", "pause"]);
    assert_eq!(o.code, 4);
    assert!(o.err.contains("safety latch"), "{}", o.err);
    assert_eq!(tq(&r, &["agent", "resume"]).code, 1);
    let o = tq(&r, &["fans"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("500"), "{}", o.err);
}

#[test]
fn a_token_the_daemon_refuses_is_exit_3_not_4() {
    let r = rig(vec![(
        "POST",
        "/agent/pause",
        401,
        json!({"error": "missing or wrong bearer token"}),
    )]);
    let o = tq(&r, &["agent", "pause"]);
    assert_eq!(o.code, 3);
    assert!(o.err.contains("wrong bearer token"), "{}", o.err);
}

// ---- the token ----

#[test]
fn an_unreadable_token_is_exit_3_with_the_fix_and_nothing_is_sent() {
    let r = rig(vec![]);
    std::fs::remove_file(r.dir.path().join("token.txt")).unwrap();
    let o = tq(&r, &["agent", "pause"]);
    assert_eq!(o.code, 3);
    assert!(
        o.err.contains("elevated or as the installing user"),
        "{}",
        o.err
    );
    assert!(o.err.contains("token.txt"), "{}", o.err);
    assert!(r.fake.requests().is_empty());
}

#[test]
fn a_malformed_token_file_is_exit_3_too() {
    let r = rig(vec![]);
    std::fs::write(r.dir.path().join("token.txt"), "short").unwrap();
    assert_eq!(tq(&r, &["agent", "pause"]).code, 3);
    assert!(r.fake.requests().is_empty());
}

#[test]
fn the_env_token_wins_over_the_file_and_the_file_is_used_otherwise() {
    let r = rig(vec![ok("POST", "/agent/pause", json!({"agent": "paused"}))]);
    let env_token = "f".repeat(64);
    let mut env = env_of(&r);
    env.token = Some(env_token.clone());
    assert_eq!(tq_with(&env, &["agent", "pause"], "").code, 0);
    assert_eq!(tq(&r, &["agent", "pause"]).code, 0);
    let auths: Vec<_> = r
        .fake
        .requests()
        .into_iter()
        .map(|q| q.auth.unwrap())
        .collect();
    assert_eq!(
        auths,
        [format!("Bearer {env_token}"), format!("Bearer {TOKEN}")]
    );
}

#[test]
fn a_bad_env_token_is_exit_3_and_is_not_replaced_by_the_file() {
    let r = rig(vec![]);
    let mut env = env_of(&r);
    env.token = Some("nonsense".into());
    let o = tq_with(&env, &["agent", "pause"], "");
    assert_eq!(o.code, 3);
    assert!(o.err.contains("THERMALIQ_TOKEN"), "{}", o.err);
    assert!(r.fake.requests().is_empty());
}

#[test]
fn without_a_data_dir_override_it_looks_in_program_data() {
    let r = rig(vec![ok("POST", "/agent/pause", json!({"agent": "paused"}))]);
    let pd = tempfile::tempdir().unwrap();
    let dir = pd.path().join("ThermalIQ");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("token.txt"), TOKEN).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        format!("api_port = {}\n", r.fake.port),
    )
    .unwrap();
    let env = Env {
        token: None,
        data_dir: None,
        program_data: Some(OsString::from(pd.path())),
        tty: false,
    };
    assert_eq!(tq_with(&env, &["agent", "pause"], "").code, 0);
}

#[test]
fn reads_work_without_a_token_and_without_a_config_file() {
    let r = rig(vec![ok("GET", "/status", status_body(false, false, true))]);
    std::fs::remove_file(r.dir.path().join("token.txt")).unwrap();
    assert_eq!(tq(&r, &["status"]).code, 0);
}

// ---- writes ----

#[test]
fn profile_set_posts_the_name_with_the_token() {
    let r = rig(vec![ok(
        "POST",
        "/profiles/active",
        json!({"active_profile": "silent"}),
    )]);
    let o = tq(&r, &["profile", "set", "silent"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("silent"), "{}", o.out);
    let q = &r.fake.requests()[0];
    assert_eq!(q.body, json!({"name": "silent"}));
    assert_eq!(q.auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
}

#[test]
fn override_sends_duty_and_seconds_and_defaults_the_expiry() {
    let reply = json!({"fan": "f1", "duty": 70, "seconds": 30, "in_effect": true, "reason": null});
    let r = rig(vec![ok("POST", "/fans/f1/override", reply)]);
    let o = tq(&r, &["override", "f1", "70", "--secs", "30"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(
        r.fake.requests()[0].body,
        json!({"duty": 70, "seconds": 30})
    );
    tq(&r, &["override", "f1", "70"]);
    assert_eq!(
        r.fake.requests()[1].body,
        json!({"duty": 70, "seconds": 60})
    );
}

#[test]
fn override_that_is_not_in_effect_says_why_and_exits_4() {
    let reply = json!({"fan": "f1", "duty": 20, "seconds": 60, "in_effect": false, "reason": "a higher priority holds this fan"});
    let r = rig(vec![ok("POST", "/fans/f1/override", reply)]);
    let o = tq(&r, &["override", "f1", "20"]);
    assert_eq!(o.code, 4);
    assert!(
        o.out.contains("a higher priority holds this fan"),
        "{}{}",
        o.out,
        o.err
    );
}

#[test]
fn override_rejects_bad_input_before_sending_anything() {
    let r = rig(vec![]);
    for args in [
        vec!["override", "f1", "101"],
        vec!["override", "f1", "-1"],
        vec!["override", "f1", "abc"],
        vec!["override", "f1"],
        vec!["override", "f1", "50", "--secs", "0"],
        vec!["override", "f1", "50", "--secs", "x"],
        vec!["override", "f1", "50", "--cancel"],
    ] {
        assert_eq!(tq(&r, &args).code, 1, "{args:?}");
    }
    assert!(r.fake.requests().is_empty());
}

#[test]
fn override_cancel_deletes_it() {
    let r = rig(vec![ok(
        "DELETE",
        "/fans/f1/override",
        json!({"fan": "f1", "override": "cancelled"}),
    )]);
    let o = tq(&r, &["override", "f1", "--cancel"]);
    assert_eq!(o.code, 0, "{}", o.err);
    let q = &r.fake.requests()[0];
    assert_eq!(
        (q.method.as_str(), q.path.as_str()),
        ("DELETE", "/fans/f1/override")
    );
    assert!(q.auth.is_some());
}

#[test]
fn agent_pause_resume_and_decisions_use_the_token() {
    let r = rig(vec![
        ok("POST", "/agent/pause", json!({"agent": "paused"})),
        ok("POST", "/agent/resume", json!({"agent": "running"})),
        ok(
            "GET",
            "/agent/decisions",
            json!({
            "hosted_account": "openrouter", "hosted_error": null, "local": "not configured",
            "foreground": true, "crashed": false,
            "questions": [{"exe": "game.exe", "suggested_workload": "gaming", "suggested_by": "local", "confidence": 0.8}],
            "decisions": [{"at": 1, "exe": "game.exe", "workload": "gaming", "source": "built-in",
                           "confidence": null, "asked": false, "profile": "gaming", "note": null}] }),
        ),
    ]);
    assert!(tq(&r, &["agent", "pause"]).out.contains("paused"));
    assert!(tq(&r, &["agent", "resume"]).out.contains("running"));
    let o = tq(&r, &["agent", "decisions"]);
    assert_eq!(o.code, 0, "{}", o.err);
    for w in ["openrouter", "game.exe", "gaming", "built-in"] {
        assert!(o.out.contains(w), "{w}: {}", o.out);
    }
    assert!(r.fake.requests().iter().all(|q| q.auth.is_some()));
}

#[test]
fn agent_key_set_reads_the_key_from_stdin_and_never_prints_it() {
    let r = rig(vec![ok(
        "PUT",
        "/agent/key",
        json!({"hosted_account": "openrouter"}),
    )]);
    let o = tq_with(
        &env_of(&r),
        &["agent", "key", "set", "openrouter"],
        "sk-secret-123\n",
    );
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(
        r.fake.requests()[0].body,
        json!({"provider": "openrouter", "key": "sk-secret-123"})
    );
    assert!(!o.out.contains("sk-secret") && !o.err.contains("sk-secret"));
    for stdin in ["", "\n", "  \n"] {
        let o = tq_with(&env_of(&r), &["agent", "key", "set", "openrouter"], stdin);
        assert_eq!(o.code, 1, "{stdin:?}");
    }
    assert_eq!(r.fake.requests().len(), 1, "an empty key is never sent");
    assert_eq!(
        tq(&r, &["agent", "key", "set", "openrouter", "sk-on-argv"]).code,
        1
    );
}

#[test]
fn a_piped_key_prints_no_prompt_and_a_tty_flag_on_a_pipe_still_works() {
    let r = rig(vec![ok(
        "PUT",
        "/agent/key",
        json!({"hosted_account": "openrouter"}),
    )]);
    let args = ["agent", "key", "set", "openrouter"];
    let quiet = tq_with(&env_of(&r), &args, "sk-secret-123\n");
    assert_eq!((quiet.code, quiet.err.as_str()), (0, ""), "{}", quiet.err);
    // tty = true but stdin is not a console: echo cannot be turned off, nothing must break.
    let mut env = env_of(&r);
    env.tty = true;
    let asked = tq_with(&env, &args, "sk-secret-456\n");
    assert_eq!(asked.code, 0, "{}", asked.err);
    assert!(asked.err.contains("not shown"), "{}", asked.err);
    assert!(!asked.err.contains("sk-secret") && !asked.out.contains("sk-secret"));
    let sent: Vec<_> = r
        .fake
        .requests()
        .into_iter()
        .map(|q| q.body["key"].clone())
        .collect();
    assert_eq!(sent, [json!("sk-secret-123"), json!("sk-secret-456")]);
    // An empty line on that path is still an error and still sends nothing.
    assert_eq!(tq_with(&env, &args, "\n").code, 1);
    assert_eq!(r.fake.requests().len(), 2);
}

#[test]
fn agent_key_clear_and_workload() {
    let r = rig(vec![
        ok("DELETE", "/agent/key", json!({"hosted_account": null})),
        ok(
            "PUT",
            "/agent/workloads/game.exe",
            json!({"exe": "game.exe", "workload": "gaming", "profile": "gaming"}),
        ),
        (
            "PUT",
            "/agent/workloads/x.exe",
            400,
            json!({"error": "unknown workload"}),
        ),
    ]);
    assert_eq!(tq(&r, &["agent", "key", "clear"]).code, 0);
    let o = tq(&r, &["agent", "workload", "game.exe", "gaming"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("gaming"), "{}", o.out);
    assert_eq!(r.fake.requests()[1].body, json!({"workload": "gaming"}));
    let o = tq(&r, &["agent", "workload", "x.exe", "napping"]);
    assert_eq!(o.code, 4);
    assert!(o.err.contains("unknown workload"), "{}", o.err);
}

#[test]
fn inventory_accept_posts() {
    let r = rig(vec![ok(
        "POST",
        "/inventory/accept",
        json!({"inventory": "saved"}),
    )]);
    let o = tq(&r, &["inventory", "accept"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(r.fake.requests()[0].path, "/inventory/accept");
    assert!(r.fake.requests()[0].auth.is_some());
}

// ---- identify ----

fn identity() -> Value {
    json!({
        "board": "TEST BOARD", "problems": [], "roles": {}, "unmapped_roles": ["cpu_fan"],
        "inventory": {"new": [], "missing": [], "changed": [], "board_changed": null},
        "proposals": [{"role": "cpu_fan", "id": "h0", "why": "its label says CPU"}],
        "fans": [{"id": "h0", "label": "CPU Fan", "kind": "fan", "rpm": 800, "controllable": true}],
    })
}

#[test]
fn identify_saves_on_enter_through_the_api() {
    let r = rig(vec![
        ok("GET", "/identity", identity()),
        ok(
            "PUT",
            "/roles",
            json!({"roles": {}, "applies": "when the service restarts"}),
        ),
        ok("POST", "/inventory/accept", json!({"inventory": "saved"})),
    ]);
    let o = tq_with(&env_of(&r), &["identify"], "\n");
    assert_eq!(o.code, 0, "{}", o.err);
    let sent = r.fake.requests();
    let put = sent.iter().find(|q| q.method == "PUT").unwrap();
    assert_eq!(put.path, "/roles");
    assert_eq!(put.body, json!({"roles": {"cpu_fan": "h0"}}));
    assert!(put.auth.is_some());
    assert!(sent.iter().any(|q| q.path == "/inventory/accept"));
}

#[test]
fn identify_probe_runs_one_fan_and_saves_nothing() {
    let probe = json!({"rpm_before": [["h0", 500]], "rpm_after": [["h0", 1300]],
                       "pairing": {"result": "own"}, "landed": true, "duty_after": 100});
    let r = rig(vec![
        ok("GET", "/identity", identity()),
        ok("POST", "/identify/probe", probe),
    ]);
    let o = tq(&r, &["identify", "--probe", "h0"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(
        o.out.contains("its own tach rose (500 -> 1300 RPM)"),
        "{}",
        o.out
    );
    let sent = r.fake.requests();
    let p = sent.iter().find(|q| q.path == "/identify/probe").unwrap();
    assert_eq!(p.body, json!({"fan": "h0"}));
    assert!(sent.iter().all(|q| q.method != "PUT"));
}

#[test]
fn identify_checks_the_token_before_it_runs_any_fan() {
    let r = rig(vec![ok("GET", "/identity", identity())]);
    std::fs::remove_file(r.dir.path().join("token.txt")).unwrap();
    for args in [vec!["identify"], vec!["identify", "--probe", "h0"]] {
        let o = tq_with(&env_of(&r), &args, "\n");
        assert_eq!(o.code, 3, "{args:?}");
    }
    assert!(r.fake.requests().is_empty(), "not even the read");
}

#[test]
fn identify_has_no_json_mode() {
    let r = rig(vec![]);
    assert_eq!(tq(&r, &["identify", "--json"]).code, 1);
}

// ---- usage ----

#[test]
fn help_is_exit_0_and_lists_every_command() {
    let r = rig(vec![]);
    for args in [vec!["help"], vec!["--help"], vec!["-h"]] {
        let o = tq(&r, &args);
        assert_eq!(o.code, 0);
        for w in [
            "status",
            "sensors",
            "fans",
            "metrics",
            "profile",
            "override",
            "agent",
            "identity",
            "identify",
            "inventory",
            "history",
            "config",
            "--json",
        ] {
            assert!(o.out.contains(w), "{w}: {}", o.out);
        }
    }
}

#[test]
fn bad_usage_is_exit_1_with_the_usage_on_stderr_and_sends_nothing() {
    let r = rig(vec![]);
    for args in [
        vec![],
        vec!["frobnicate"],
        vec!["status", "extra"],
        vec!["status", "--nope"],
        vec!["profile"],
        vec!["profile", "set"],
        vec!["agent", "key", "set"],
        vec!["agent", "workload", "game.exe"],
        vec!["history"],
        vec!["history", "s1", "--mins"],
    ] {
        let o = tq(&r, &args);
        assert_eq!(o.code, 1, "{args:?}");
        assert!(o.err.contains("usage"), "{args:?}: {}", o.err);
    }
    assert!(r.fake.requests().is_empty());
}
