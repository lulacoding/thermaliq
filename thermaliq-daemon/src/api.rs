//! HTTP API and WebSocket (SPEC §6). Runs on its own tokio runtime, apart from the engine
//! thread. Reads come from the snapshot cache; every mutation needs the Bearer token and
//! only answers 200 once the engine thread (or the disk) has actually done it.

use crate::history::HistoryDb;
use crate::runtime::{clamp_override, Cache, CommandError, Handle};
use crate::store::{load_config, save_config};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use thermaliq_common::{DaemonConfig, Profile};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// The only origins a browser may call us from (the Tauri webview on Mac/Linux and Windows).
const ALLOWED_ORIGINS: [&str; 2] = ["tauri://localhost", "http://tauri.localhost"];
/// How long a shutdown waits for open connections (e.g. a WebSocket) before dropping them.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ApiState {
    handle: Handle,
    profiles: Arc<[Profile]>,
    history: Arc<Mutex<HistoryDb>>,
    token: Arc<str>,
    dir: Arc<PathBuf>,
    /// `save_config` writes a fixed temp name, so saves go one at a time.
    config_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ApiState {
    pub fn new(
        handle: Handle,
        profiles: Vec<Profile>,
        history: Arc<Mutex<HistoryDb>>,
        token: String,
        dir: PathBuf,
    ) -> Self {
        Self {
            handle,
            profiles: profiles.into(),
            history,
            token: token.into(),
            dir: Arc::new(dir),
            config_lock: Arc::default(),
        }
    }
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<CommandError> for ApiError {
    fn from(e: CommandError) -> Self {
        match e {
            CommandError::Unavailable => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "engine is not running".into(),
            ),
            CommandError::NotFound(m) => Self(StatusCode::NOT_FOUND, m),
            CommandError::Rejected(m) => Self(StatusCode::CONFLICT, m),
        }
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub fn router(state: ApiState) -> Router {
    let protected = Router::new()
        .route("/profiles/active", post(set_active_profile))
        .route(
            "/fans/{id}/override",
            post(set_override).delete(cancel_override),
        )
        .route("/config", put(put_config))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(
            ALLOWED_ORIGINS.map(HeaderValue::from_static),
        ))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    Router::new()
        .route("/status", get(status))
        .route("/sensors", get(sensors))
        .route("/fans", get(fans))
        .route("/profiles", get(profiles))
        .route("/config", get(get_config))
        .route("/history/sensors/{id}", get(history_sensor))
        .route("/ws", get(ws))
        .merge(protected)
        .layer(cors)
        .with_state(state)
}

// ---- auth ----

async fn require_bearer(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    match bearer(req.headers()) {
        Some(t) if constant_time_eq(t.as_bytes(), state.token.as_bytes()) => next.run(req).await,
        _ => ApiError(
            StatusCode::UNAUTHORIZED,
            "missing or wrong bearer token".into(),
        )
        .into_response(),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Compares every byte, so the time taken does not say how much of a guess was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---- reads (all from the cache) ----

fn status_json(c: &Cache) -> Value {
    json!({
        "engine_running": c.status.engine_running,
        "latched": c.status.latched,
        "degraded": c.status.degraded,
        "waiting_on": c.status.waiting_on,
        "snapshot_age": c.snapshot_age,
        "active_profile": c.active_profile,
        "ticks": c.ticks,
    })
}

async fn status(State(s): State<ApiState>) -> Json<Value> {
    Json(status_json(&s.handle.cache()))
}

async fn sensors(State(s): State<ApiState>) -> Json<Value> {
    Json(json!(s.handle.cache().snapshot.sensors))
}

async fn fans(State(s): State<ApiState>) -> Json<Value> {
    Json(json!(s.handle.cache().snapshot.fans))
}

async fn profiles(State(s): State<ApiState>) -> Json<Value> {
    let active = s.handle.cache().active_profile;
    Json(
        s.profiles
            .iter()
            .map(|p| json!({"name": p.name, "description": p.description, "active": p.name == active}))
            .collect(),
    )
}

async fn history_sensor(State(s): State<ApiState>, Path(id): Path<String>) -> ApiResult {
    let rows = tokio::task::spawn_blocking(move || {
        s.history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sensor_rows(&id)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|(t, temp)| json!({"t": t, "temp_celsius": temp}))
            .collect(),
    ))
}

// ---- mutations ----

#[derive(Deserialize)]
struct ProfileName {
    name: String,
}

async fn set_active_profile(State(s): State<ApiState>, Json(b): Json<ProfileName>) -> ApiResult {
    let profile = s
        .profiles
        .iter()
        .find(|p| p.name == b.name)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("no profile '{}'", b.name)))?;
    s.handle.set_profile(profile.clone()).await?;
    Ok(Json(json!({"active_profile": b.name})))
}

#[derive(Deserialize)]
struct OverrideBody {
    duty: i64,
    seconds: u64,
}

async fn set_override(
    State(s): State<ApiState>,
    Path(fan): Path<String>,
    Json(b): Json<OverrideBody>,
) -> ApiResult {
    let (duty, dur) = clamp_override(b.duty, b.seconds)
        .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "seconds must be > 0".into()))?;
    s.handle.set_override(&fan, b.duty, b.seconds).await?;
    Ok(Json(
        json!({"fan": fan, "duty": duty, "seconds": dur.as_secs()}),
    ))
}

async fn cancel_override(State(s): State<ApiState>, Path(fan): Path<String>) -> ApiResult {
    s.handle.cancel_override(&fan).await?;
    Ok(Json(json!({"fan": fan, "override": "cancelled"})))
}

async fn get_config(State(s): State<ApiState>) -> ApiResult {
    let dir = s.dir.clone();
    let cfg = tokio::task::spawn_blocking(move || load_config(&dir))
        .await
        .map_err(internal)?
        .map_err(internal)?;
    Ok(Json(json!(cfg)))
}

/// Saves `config.toml`. The running daemon keeps its current settings until it restarts.
async fn put_config(State(s): State<ApiState>, Json(cfg): Json<DaemonConfig>) -> ApiResult {
    let _one_at_a_time = s.config_lock.lock().await;
    let dir = s.dir.clone();
    let saved = tokio::task::spawn_blocking(move || {
        save_config(&dir, &cfg).and_then(|()| load_config(&dir))
    })
    .await
    .map_err(internal)?;
    match saved {
        Ok(cfg) => Ok(Json(json!({"config": cfg, "restart_required": true}))),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            Err(ApiError(StatusCode::BAD_REQUEST, e.to_string()))
        }
        Err(e) => Err(internal(e)),
    }
}

// ---- websocket ----

async fn ws(upgrade: WebSocketUpgrade, State(s): State<ApiState>) -> Response {
    upgrade.on_upgrade(move |socket| stream(socket, s.handle))
}

fn frame(c: &Cache) -> Message {
    let mut v = status_json(c);
    v["status"] = json!({
        "engine_running": c.status.engine_running,
        "latched": c.status.latched,
        "degraded": c.status.degraded,
        "waiting_on": c.status.waiting_on,
    });
    v["sensors"] = json!(c.snapshot.sensors);
    v["fans"] = json!(c.snapshot.fans);
    Message::Text(v.to_string().into())
}

/// Sends the cache now and after every tick, until the client leaves or the engine ends.
async fn stream(mut socket: WebSocket, handle: Handle) {
    let mut ticks = handle.ticks();
    loop {
        // Mark the current tick seen before reading, so a tick is never sent twice or missed.
        ticks.borrow_and_update();
        if socket.send(frame(&handle.cache())).await.is_err() {
            return;
        }
        tokio::select! {
            changed = ticks.changed() => if changed.is_err() {
                // The engine ended: say so once, then close.
                let _ = socket.send(frame(&handle.cache())).await;
                return;
            },
            msg = socket.recv() => if !matches!(msg, Some(Ok(_))) { return },
        }
    }
}

// ---- server ----

/// The API on its own runtime, bound to 127.0.0.1 only.
pub struct ApiServer {
    rt: Option<Runtime>,
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl ApiServer {
    /// `port` 0 picks a free one (tests).
    pub fn start(state: ApiState, port: u16) -> io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("thermaliq-api")
            .enable_all()
            .build()?;
        let listener = rt.block_on(TcpListener::bind((Ipv4Addr::LOCALHOST, port)))?;
        let addr = listener.local_addr()?;
        let (stop, stopped) = oneshot::channel::<()>();
        let task = rt.spawn(async move {
            let _ = axum::serve(listener, router(state))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Ok(Self {
            rt: Some(rt),
            addr,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shutdown(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(rt) = self.rt.take() {
            if let Some(task) = self.task.take() {
                // Open connections (a WebSocket) are dropped when the grace runs out.
                rt.block_on(async {
                    let _ = tokio::time::timeout(SHUTDOWN_GRACE, task).await;
                });
            }
            rt.shutdown_timeout(SHUTDOWN_GRACE);
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop_now();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime::{Daemon, Options};
    use crate::store::{load_config, save_config};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures_util::StreamExt;
    use serde_json::{json, Value};
    use std::net::Ipv4Addr;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use thermaliq_common::{DaemonConfig, Profile, SensorKind};
    use thermaliq_hal::{MockHardwareLayer, Tick};
    use tower::ServiceExt;

    const FLAT: &str = "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Rig {
        daemon: Daemon,
        hal: Arc<MockHardwareLayer>,
        db: Arc<Mutex<HistoryDb>>,
        dir: tempfile::TempDir,
        state: ApiState,
    }

    fn at(cpu: f32) -> Tick {
        Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, cpu)])
    }

    fn rig(script: Vec<Tick>) -> Rig {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], script));
        let flat = Profile::from_toml(FLAT).unwrap();
        let sixty = FLAT
            .replace("pwm = 30", "pwm = 60")
            .replace("name = \"flat\"", "name = \"sixty\"");
        let sixty = Profile::from_toml(&sixty).unwrap();
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        let opts = Options {
            tick: Duration::from_millis(5),
            prune_every: Duration::from_secs(3600),
        };
        let daemon = Daemon::start(hal.clone(), &flat, vec![sixty.clone()], db.clone(), opts);
        let dir = tempfile::tempdir().unwrap();
        let state = ApiState::new(
            daemon.handle(),
            vec![flat, sixty],
            db.clone(),
            TOKEN.into(),
            dir.path().into(),
        );
        Rig {
            daemon,
            hal,
            db,
            dir,
            state,
        }
    }

    async fn until(mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    async fn ticked(r: &Rig) {
        let h = r.daemon.handle();
        until(|| h.cache().ticks > 0).await;
    }

    async fn call(
        r: &Rig,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = router(r.state.clone())
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(r: &Rig, uri: &str) -> (StatusCode, Value) {
        call(r, "GET", uri, None, None).await
    }

    fn last_write(r: &Rig) -> Option<(String, u8)> {
        r.hal.writes().last().cloned()
    }

    // ---- reads ----

    #[tokio::test]
    async fn status_reports_the_engine_state_and_active_profile() {
        let r = rig(vec![at(40.0)]);
        ticked(&r).await;
        let (code, v) = get(&r, "/status").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["engine_running"], true);
        assert_eq!(v["latched"], false);
        assert_eq!(v["degraded"], false);
        assert_eq!(v["waiting_on"], json!([]));
        assert_eq!(v["snapshot_age"], 0);
        assert_eq!(v["active_profile"], "flat");
    }

    #[tokio::test]
    async fn status_reports_latched_and_what_it_waits_on() {
        let r = rig(vec![at(97.0)]);
        until(|| r.daemon.cache().status.latched).await;
        let (_, v) = get(&r, "/status").await;
        assert_eq!(v["latched"], true);
        assert_eq!(v["waiting_on"], json!(["cpu"]));
    }

    #[tokio::test]
    async fn status_says_when_the_engine_is_not_running() {
        let r = rig(vec![at(40.0)]);
        ticked(&r).await;
        let Rig { daemon, state, .. } = r;
        daemon.shutdown().unwrap();
        let res = router(state)
            .oneshot(Request::get("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["engine_running"], false);
    }

    #[tokio::test]
    async fn status_reports_a_stale_snapshot_and_sensors_keep_the_last_values() {
        let r = rig(vec![at(40.0), Tick::Fail]);
        until(|| r.daemon.cache().snapshot_age >= 2).await;
        let (_, v) = get(&r, "/status").await;
        assert!(v["snapshot_age"].as_u64().unwrap() >= 2);
        let (_, s) = get(&r, "/sensors").await;
        assert_eq!(s[0]["temp_celsius"], 40.0);
    }

    #[tokio::test]
    async fn sensors_and_fans_come_from_the_cache() {
        let r = rig(vec![at(40.0)]);
        ticked(&r).await;
        let (code, s) = get(&r, "/sensors").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(s[0]["id"], "cpu");
        assert_eq!(s[0]["temp_celsius"], 40.0);
        let (code, f) = get(&r, "/fans").await;
        assert_eq!(code, StatusCode::OK);
        let ids: Vec<_> = f
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].clone())
            .collect();
        assert_eq!(ids, [json!("f1"), json!("f2")]);
    }

    #[tokio::test]
    async fn profiles_lists_every_profile_and_marks_the_active_one() {
        let r = rig(vec![at(40.0)]);
        let (code, v) = get(&r, "/profiles").await;
        assert_eq!(code, StatusCode::OK);
        let v = v.as_array().unwrap();
        assert_eq!(v.len(), 2);
        let active: Vec<_> = v.iter().filter(|p| p["active"] == true).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0]["name"], "flat");
    }

    #[tokio::test]
    async fn history_returns_recorded_rows_and_nothing_for_an_unknown_sensor() {
        let r = rig(vec![at(40.0)]);
        until(|| !r.db.lock().unwrap().sensor_rows("cpu").unwrap().is_empty()).await;
        let (code, v) = get(&r, "/history/sensors/cpu").await;
        assert_eq!(code, StatusCode::OK);
        let first = &v.as_array().unwrap()[0];
        assert_eq!(first["temp_celsius"], 40.0);
        assert!(first["t"].as_i64().unwrap() > 0);
        let (code, v) = get(&r, "/history/sensors/nope").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v, json!([]));
    }

    // ---- profile switch ----

    #[tokio::test]
    async fn switching_profile_changes_what_the_engine_writes_and_what_status_says() {
        let r = rig(vec![at(40.0)]);
        let (code, _) = call(
            &r,
            "POST",
            "/profiles/active",
            Some(TOKEN),
            Some(json!({"name": "sixty"})),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        until(|| last_write(&r) == Some(("f1".into(), 60))).await;
        assert_eq!(get(&r, "/status").await.1["active_profile"], "sixty");
        let (_, p) = get(&r, "/profiles").await;
        assert!(p
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "sixty" && p["active"] == true));
    }

    #[tokio::test]
    async fn switching_to_an_unknown_profile_is_404_and_changes_nothing() {
        let r = rig(vec![at(40.0)]);
        let (code, _) = call(
            &r,
            "POST",
            "/profiles/active",
            Some(TOKEN),
            Some(json!({"name": "nope"})),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(get(&r, "/status").await.1["active_profile"], "flat");
    }

    // ---- overrides ----

    #[tokio::test]
    async fn override_reaches_the_fan_and_the_reply_shows_the_clamped_values() {
        let r = rig(vec![at(40.0)]);
        let (code, v) = call(
            &r,
            "POST",
            "/fans/f1/override",
            Some(TOKEN),
            Some(json!({"duty": 250, "seconds": 9999})),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v, json!({"fan": "f1", "duty": 100, "seconds": 300}));
        until(|| last_write(&r) == Some(("f1".into(), 100))).await;
    }

    #[tokio::test]
    async fn override_with_negative_duty_is_clamped_to_zero() {
        let r = rig(vec![at(40.0)]);
        let (_, v) = call(
            &r,
            "POST",
            "/fans/f1/override",
            Some(TOKEN),
            Some(json!({"duty": -20, "seconds": 5})),
        )
        .await;
        assert_eq!(v["duty"], 0);
        until(|| last_write(&r) == Some(("f1".into(), 0))).await;
    }

    #[tokio::test]
    async fn bad_overrides_are_refused_and_write_nothing() {
        let r = rig(vec![at(40.0)]);
        let post = |uri: &'static str, b: Value| call(&r, "POST", uri, Some(TOKEN), Some(b));
        let (code, _) = post("/fans/f1/override", json!({"duty": 50, "seconds": 0})).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        let (code, _) = post("/fans/f2/override", json!({"duty": 50, "seconds": 5})).await;
        assert_eq!(
            code,
            StatusCode::CONFLICT,
            "fan has no control in the profile"
        );
        let (code, _) = post("/fans/nope/override", json!({"duty": 50, "seconds": 5})).await;
        assert_eq!(code, StatusCode::CONFLICT);
        let (code, _) = post("/fans/f1/override", json!({"duty": "high"})).await;
        assert!(code.is_client_error());
        assert!(
            r.hal.writes().iter().all(|(_, d)| *d == 30),
            "only the profile wrote"
        );
    }

    #[tokio::test]
    async fn cancelling_an_override_returns_the_fan_and_a_second_cancel_is_404() {
        let r = rig(vec![at(40.0)]);
        call(
            &r,
            "POST",
            "/fans/f1/override",
            Some(TOKEN),
            Some(json!({"duty": 90, "seconds": 60})),
        )
        .await;
        until(|| last_write(&r) == Some(("f1".into(), 90))).await;
        let (code, _) = call(&r, "DELETE", "/fans/f1/override", Some(TOKEN), None).await;
        assert_eq!(code, StatusCode::OK);
        until(|| last_write(&r) == Some(("f1".into(), 30))).await;
        let (code, _) = call(&r, "DELETE", "/fans/f1/override", Some(TOKEN), None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        let (code, _) = call(&r, "DELETE", "/fans/nope/override", Some(TOKEN), None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn commands_to_a_stopped_engine_are_503_not_success() {
        let r = rig(vec![at(40.0)]);
        let Rig { daemon, state, .. } = r;
        daemon.shutdown().unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let mut req = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"));
            let body = body.map_or_else(Body::empty, |b| {
                req = std::mem::take(&mut req).header("content-type", "application/json");
                Body::from(b.to_string())
            });
            router(state.clone()).oneshot(req.body(body).unwrap())
        };
        let ov = json!({"duty": 50, "seconds": 5});
        for (m, u, b) in [
            ("POST", "/fans/f1/override", Some(ov)),
            ("DELETE", "/fans/f1/override", None),
            ("POST", "/profiles/active", Some(json!({"name": "sixty"}))),
        ] {
            assert_eq!(
                send(m, u, b).await.unwrap().status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }

    // ---- config ----

    #[tokio::test]
    async fn get_config_returns_the_saved_config_or_defaults() {
        let r = rig(vec![at(40.0)]);
        let (code, v) = get(&r, "/config").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["api_port"], 7272);
        let cfg = DaemonConfig {
            api_port: 9000,
            ..DaemonConfig::default()
        };
        save_config(r.dir.path(), &cfg).unwrap();
        assert_eq!(get(&r, "/config").await.1["api_port"], 9000);
    }

    #[tokio::test]
    async fn put_config_saves_it_to_disk_and_says_a_restart_is_needed() {
        let r = rig(vec![at(40.0)]);
        let body = json!({"api_port": 9100, "polling_interval_ms": 500, "log_level": "debug", "default_profile": "silent"});
        let (code, v) = call(&r, "PUT", "/config", Some(TOKEN), Some(body)).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["restart_required"], true);
        assert_eq!(v["config"]["api_port"], 9100);
        let saved = load_config(r.dir.path()).unwrap();
        assert_eq!(saved.api_port, 9100);
        assert_eq!(saved.polling_interval_ms, 500);
        assert_eq!(saved.default_profile, "silent");
    }

    #[tokio::test]
    async fn an_invalid_config_is_400_and_the_file_is_untouched() {
        let r = rig(vec![at(40.0)]);
        save_config(r.dir.path(), &DaemonConfig::default()).unwrap();
        let before = std::fs::read(r.dir.path().join("config.toml")).unwrap();
        for bad in [
            json!({"polling_interval_ms": 60000}),
            json!({"api_port": 0}),
            json!({"default_profile": ""}),
        ] {
            let (code, _) = call(&r, "PUT", "/config", Some(TOKEN), Some(bad)).await;
            assert_eq!(code, StatusCode::BAD_REQUEST);
        }
        let (code, _) = call(&r, "PUT", "/config", Some(TOKEN), Some(json!({"nope": 1}))).await;
        assert!(code.is_client_error(), "unknown fields are refused");
        assert_eq!(
            std::fs::read(r.dir.path().join("config.toml")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn concurrent_config_saves_are_serialized_and_all_succeed() {
        let r = rig(vec![at(40.0)]);
        let puts: Vec<_> = (0..20)
            .map(|i| {
                let state = r.state.clone();
                tokio::spawn(async move {
                    let req = Request::put("/config")
                        .header("authorization", format!("Bearer {TOKEN}"))
                        .header("content-type", "application/json")
                        .body(Body::from(json!({"api_port": 9000 + i}).to_string()))
                        .unwrap();
                    router(state).oneshot(req).await.unwrap().status()
                })
            })
            .collect();
        for p in puts {
            assert_eq!(p.await.unwrap(), StatusCode::OK);
        }
        assert!(load_config(r.dir.path()).unwrap().api_port >= 9000);
        let files: Vec<_> = std::fs::read_dir(r.dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1, "no temp file left behind");
    }

    // ---- auth ----

    #[tokio::test]
    async fn every_mutation_without_a_token_is_401_and_does_nothing() {
        let r = rig(vec![at(40.0)]);
        ticked(&r).await;
        let writes = r.hal.writes().len();
        let cases = [
            ("POST", "/profiles/active", Some(json!({"name": "sixty"}))),
            (
                "POST",
                "/fans/f1/override",
                Some(json!({"duty": 90, "seconds": 60})),
            ),
            ("DELETE", "/fans/f1/override", None),
            ("PUT", "/config", Some(json!({"api_port": 9999}))),
        ];
        for (m, u, b) in cases {
            let (code, _) = call(&r, m, u, None, b.clone()).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{m} {u} without a token");
            let (code, _) = call(&r, m, u, Some("wrong"), b.clone()).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{m} {u} with a wrong token");
            let almost = format!("{}0", &TOKEN[..63]);
            let (code, _) = call(&r, m, u, Some(&almost), b).await;
            assert_eq!(
                code,
                StatusCode::UNAUTHORIZED,
                "{m} {u} with a near-miss token"
            );
        }
        assert_eq!(get(&r, "/status").await.1["active_profile"], "flat");
        assert!(r.hal.writes().iter().all(|(_, d)| *d == 30));
        assert!(r.hal.writes().len() >= writes);
        assert!(!r.dir.path().join("config.toml").exists());
    }

    #[tokio::test]
    async fn only_a_bearer_scheme_is_accepted() {
        let r = rig(vec![at(40.0)]);
        for header in [
            format!("Basic {TOKEN}"),
            TOKEN.to_string(),
            "Bearer".to_string(),
        ] {
            let req = Request::delete("/fans/f1/override")
                .header("authorization", header)
                .body(Body::empty())
                .unwrap();
            let res = router(r.state.clone()).oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[test]
    fn constant_time_compare_matches_only_equal_bytes() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    // ---- CORS ----

    async fn preflight(r: &Rig, origin: &str) -> Option<String> {
        let req = Request::options("/config")
            .header("origin", origin)
            .header("access-control-request-method", "PUT")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap();
        let res = router(r.state.clone()).oneshot(req).await.unwrap();
        res.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn cors_allows_only_the_two_tauri_origins() {
        let r = rig(vec![at(40.0)]);
        for ok in ["tauri://localhost", "http://tauri.localhost"] {
            assert_eq!(preflight(&r, ok).await.as_deref(), Some(ok));
        }
        for bad in [
            "http://evil.example",
            "https://tauri.localhost",
            "http://localhost:1420",
            "null",
        ] {
            assert_eq!(preflight(&r, bad).await, None, "{bad}");
        }
        let req = Request::get("/status")
            .header("origin", "http://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = router(r.state.clone()).oneshot(req).await.unwrap();
        assert!(res.headers().get("access-control-allow-origin").is_none());
    }

    // ---- real server: bind address, websocket ----

    #[test]
    fn the_server_binds_127_0_0_1_only_and_releases_the_port_on_shutdown() {
        let r = rig(vec![at(40.0)]);
        let server = ApiServer::start(r.state.clone(), 0).unwrap();
        let addr = server.local_addr();
        assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(addr.port(), 0);
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        use std::io::{Read, Write};
        c.write_all(b"GET /status HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(out.contains("\"engine_running\""));
        // Not listening on any other address of this machine.
        for ip in local_non_loopback_ips() {
            let other = std::net::SocketAddr::new(ip, addr.port());
            assert!(
                std::net::TcpStream::connect_timeout(&other, Duration::from_millis(500)).is_err(),
                "reachable on {other}"
            );
        }
        server.shutdown();
        assert!(std::net::TcpStream::connect(addr).is_err(), "port released");
    }

    /// The address this machine would use to reach the internet, if it has one.
    fn local_non_loopback_ips() -> Vec<std::net::IpAddr> {
        let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") else {
            return vec![];
        };
        if s.connect("192.0.2.1:9").is_err() {
            return vec![];
        }
        s.local_addr()
            .map(|a| a.ip())
            .into_iter()
            .filter(|ip| !ip.is_loopback())
            .collect()
    }

    #[test]
    fn websocket_streams_the_cache_every_tick() {
        let r = rig(vec![at(40.0)]);
        let server = ApiServer::start(r.state.clone(), 0).unwrap();
        let url = format!("ws://{}/ws", server.local_addr());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ticks: Vec<u64> = rt.block_on(async {
            let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            let mut ticks = vec![];
            while ticks.len() < 3 {
                let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                assert_eq!(v["status"]["engine_running"], true);
                assert_eq!(v["sensors"][0]["id"], "cpu");
                assert_eq!(v["fans"].as_array().unwrap().len(), 2);
                assert_eq!(v["active_profile"], "flat");
                ticks.push(v["ticks"].as_u64().unwrap());
            }
            ticks
        });
        assert!(ticks.windows(2).all(|w| w[1] > w[0]), "{ticks:?}");
        server.shutdown();
    }

    #[test]
    fn shutdown_does_not_wait_for_an_open_websocket() {
        let r = rig(vec![at(40.0)]);
        let server = ApiServer::start(r.state.clone(), 0).unwrap();
        let url = format!("ws://{}/ws", server.local_addr());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_ws, _) = rt.block_on(tokio_tungstenite::connect_async(url)).unwrap();
        let t = Instant::now();
        server.shutdown();
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
