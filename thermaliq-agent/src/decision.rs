//! The slow layer's logic (SPEC §7.2): typed questions, provider routing, the fixed
//! workload → profile rule, the per-exe cache, and when the local model may run. Pure, like
//! the rest of this crate: the daemon supplies the providers, the clock and the observations.

use crate::classifier::Workload;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use thermaliq_common::{Curve, CustomSensor, HardwareSnapshot, Profile};

/// SPEC §7.2: every 60 s, and whenever the foreground app changes.
pub const CADENCE: Duration = Duration::from_secs(60);
/// An unseen exe waits this long: an alt-tab costs no call, and the averages mean something.
pub const SETTLE: Duration = Duration::from_secs(10);
/// How far back the state's averages reach, within this exe's time in the foreground.
pub const STATE_WINDOW: Duration = Duration::from_secs(60);
/// SPEC §7.4: slow-layer changes per rolling hour.
pub const MAX_CHANGES_PER_HOUR: usize = 6;
const HOUR: Duration = Duration::from_secs(3600);
pub const MAX_QUESTIONS: usize = 32;
/// A local model that failed to load is not retried sooner than this (it costs 37 s and 8 GB).
pub const LOCAL_RETRY: Duration = Duration::from_secs(600);

/// Sam's review of PR #54. GPU load above this, or a fullscreen foreground app, is a game
/// whether or not anything has recognised it.
pub const GAME_GPU_LOAD_PCT: f64 = 50.0;
pub const IDLE_CPU_LOAD_PCT: f64 = 30.0;
pub const IDLE_GPU_LOAD_PCT: f64 = 20.0;
pub const IDLE_FOR: Duration = Duration::from_secs(300);
/// Observations further apart than this say nothing about the time in between.
const MAX_OBSERVE_GAP: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Hosted,
    Local,
}

/// Who answered. A built-in answer has no model confidence: it is a default the user can correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Hosted,
    Local,
    BuiltIn,
    User,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Local => "local",
            Self::BuiltIn => "built-in",
            Self::User => "user",
        }
    }

    /// Only what the per-exe cache stores: a built-in answer is never cached.
    pub fn from_cached(s: &str) -> Option<Self> {
        match s {
            "hosted" => Some(Self::Hosted),
            "local" => Some(Self::Local),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub workload: f64,
    pub big: f64,
}

impl Provider {
    /// shortcut: starting values, not calibrated. Providers' scales differ (the spike's local
    /// answers were mostly under 0.6, except for games), so each has its own. M20e calibrates
    /// them with a Brier score from accept/undo history, which doesn't exist before it.
    pub const fn thresholds(self) -> Thresholds {
        match self {
            Self::Hosted => Thresholds {
                workload: 0.7,
                big: 0.85,
            },
            Self::Local => Thresholds {
                workload: 0.6,
                big: 0.85,
            },
        }
    }

    const fn source(self) -> Source {
        match self {
            Self::Hosted => Source::Hosted,
            Self::Local => Source::Local,
        }
    }
}

/// SPEC §7.2: the fixed rule. The model never picks the profile.
pub const fn profile_for(w: Workload) -> &'static str {
    match w {
        Workload::Idle => "silent",
        Workload::Browsing => "efficiency",
        Workload::Gaming => "gaming",
        Workload::Compiling | Workload::Rendering => "performance",
        Workload::Other => "balanced",
    }
}

const fn describe(w: Workload) -> &'static str {
    match w {
        Workload::Idle => "Nothing demanding is running",
        Workload::Browsing => "Web browser or light desktop use",
        Workload::Gaming => "A game is running",
        Workload::Compiling => "Building software",
        Workload::Rendering => "Rendering, encoding or other long GPU or CPU compute",
        Workload::Other => "None of these",
    }
}

/// The built-in list of well-known exes. It is only asked when no model can answer.
pub fn builtin_workload(exe: &str) -> Option<Workload> {
    use Workload::{Browsing, Compiling, Gaming, Other, Rendering};
    const LIST: &[(&str, Workload)] = &[
        ("apexlegends.exe", Gaming),
        ("baldursgate3.exe", Gaming),
        ("bg3.exe", Gaming),
        ("blender.exe", Rendering),
        ("brave.exe", Browsing),
        ("cargo.exe", Compiling),
        ("chrome.exe", Browsing),
        ("cl.exe", Compiling),
        ("code.exe", Other),
        ("cs2.exe", Gaming),
        ("cyberpunk2077.exe", Gaming),
        ("devenv.exe", Compiling),
        ("discord.exe", Other),
        ("dota2.exe", Gaming),
        ("eldenring.exe", Gaming),
        ("excel.exe", Other),
        ("ffmpeg.exe", Rendering),
        ("firefox.exe", Browsing),
        ("fortniteclient-win64-shipping.exe", Gaming),
        ("gcc.exe", Compiling),
        ("handbrake.exe", Rendering),
        ("javaw.exe", Other),
        ("leagueoflegends.exe", Gaming),
        ("minecraft.windows.exe", Gaming),
        ("msbuild.exe", Compiling),
        ("msedge.exe", Browsing),
        ("obs64.exe", Rendering),
        ("opera.exe", Browsing),
        ("outlook.exe", Other),
        ("overwatch.exe", Gaming),
        ("premiere.exe", Rendering),
        ("resolve.exe", Rendering),
        ("rustc.exe", Compiling),
        ("spotify.exe", Other),
        ("steam.exe", Other),
        ("valorant-win64-shipping.exe", Gaming),
        ("vlc.exe", Other),
        ("winword.exe", Other),
    ];
    let exe = exe.to_ascii_lowercase();
    LIST.iter().find(|(e, _)| *e == exe).map(|(_, w)| *w)
}

/// The exe's bare file name in lower case, or `None`. A path would leak the user name to a
/// hosted model, so one is refused rather than trimmed.
pub fn clean_exe(raw: &str) -> Option<String> {
    let ok = !raw.is_empty()
        && raw.len() <= 128
        && !raw.contains(['/', '\\', ':'])
        && !raw.chars().any(char::is_control);
    ok.then(|| raw.to_lowercase())
}

/// All a provider is ever told: an exe name and numbers (SPEC §7.2). A reading that was never
/// seen stays `None`, never 0.
#[derive(Debug, Clone, PartialEq)]
pub struct ExeState {
    pub exe: String,
    pub seconds_observed: u32,
    pub cpu_load_pct: Option<f64>,
    pub gpu_load_pct: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub id: &'static str,
    pub instructions: String,
    /// Option name and its description.
    pub options: Vec<(String, String)>,
    pub state: ExeState,
}

pub fn workload_question(state: ExeState) -> Question {
    Question {
        id: "workload",
        instructions: "Which workload is this app, given its recent average readings?".into(),
        options: Workload::ALL
            .into_iter()
            .map(|w| (w.name().into(), describe(w).into()))
            .collect(),
        state,
    }
}

/// A big call (SPEC §7.2: accept a learned curve, create a profile).
pub fn accept_question(state: ExeState, instructions: &str) -> Question {
    Question {
        id: "accept",
        instructions: instructions.into(),
        options: vec![
            ("accept".into(), "Apply this change".into()),
            ("reject".into(), "Keep things as they are".into()),
        ],
        state,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub choice: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// Offline, no key, timed out, failed to load, bad response: try the next route.
    Unavailable(String),
    /// `cancel` turned true (a game came to the foreground, pause, shutdown).
    Cancelled,
}

/// One way of getting a typed answer. Implementations check `cancel` while they wait.
pub trait DecisionProvider {
    fn ask(&mut self, q: &Question, cancel: &dyn Fn() -> bool) -> Result<Answer, AskError>;
}

/// An answer is only usable if it is one of the options, with a real confidence.
fn checked(
    p: &mut dyn DecisionProvider,
    q: &Question,
    cancel: &dyn Fn() -> bool,
) -> Option<Answer> {
    p.ask(q, cancel).ok().filter(|a| {
        (0.0..=1.0).contains(&a.confidence) && q.options.iter().any(|(n, _)| *n == a.choice)
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suggestion {
    pub workload: Workload,
    pub source: Source,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadRoute {
    Decided {
        workload: Workload,
        source: Source,
        confidence: Option<f64>,
    },
    /// Below the threshold, or nothing could answer: the user decides.
    Ask { suggestion: Option<Suggestion> },
}

/// `None` if this provider gave no usable answer: the next route gets its turn.
fn try_provider(
    provider: Provider,
    p: &mut dyn DecisionProvider,
    q: &Question,
    cancel: &dyn Fn() -> bool,
) -> Option<WorkloadRoute> {
    let answer = checked(p, q, cancel)?;
    let workload = Workload::from_name(&answer.choice)?;
    let source = provider.source();
    Some(if answer.confidence >= provider.thresholds().workload {
        WorkloadRoute::Decided {
            workload,
            source,
            confidence: Some(answer.confidence),
        }
    } else {
        WorkloadRoute::Ask {
            suggestion: Some(Suggestion {
                workload,
                source,
                confidence: answer.confidence,
            }),
        }
    })
}

/// SPEC §7.2 order: hosted → local (the caller passes it only when the PC is idle) → the
/// built-in list → ask. A low-confidence answer asks: it does not shop for a better one.
pub fn route_workload(
    state: &ExeState,
    hosted: Option<&mut dyn DecisionProvider>,
    local: Option<&mut dyn DecisionProvider>,
    cancel: &dyn Fn() -> bool,
) -> WorkloadRoute {
    let q = workload_question(state.clone());
    if let Some(r) = hosted.and_then(|p| try_provider(Provider::Hosted, p, &q, cancel)) {
        return r;
    }
    if let Some(r) = local.and_then(|p| try_provider(Provider::Local, p, &q, cancel)) {
        return r;
    }
    match builtin_workload(&state.exe) {
        Some(workload) => WorkloadRoute::Decided {
            workload,
            source: Source::BuiltIn,
            confidence: None,
        },
        None => WorkloadRoute::Ask { suggestion: None },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BigOutcome {
    Apply(String),
    Ask,
}

/// SPEC §7.2. Both answer: agreement applies, disagreement asks. One answers: it needs its
/// big-change threshold. None answers: ask. The built-in list knows nothing about big calls.
/// M20e's learning is the first caller; nothing produces a big call before it.
pub fn decide_big(
    q: &Question,
    hosted: Option<&mut dyn DecisionProvider>,
    local: Option<&mut dyn DecisionProvider>,
    cancel: &dyn Fn() -> bool,
) -> BigOutcome {
    let h = hosted.and_then(|p| checked(p, q, cancel));
    let l = local.and_then(|p| checked(p, q, cancel));
    let alone = |a: Answer, p: Provider| {
        if a.confidence >= p.thresholds().big {
            BigOutcome::Apply(a.choice)
        } else {
            BigOutcome::Ask
        }
    };
    match (h, l) {
        (Some(h), Some(l)) if h.choice == l.choice => BigOutcome::Apply(h.choice),
        (Some(_), Some(_)) | (None, None) => BigOutcome::Ask,
        (Some(a), None) => alone(a, Provider::Hosted),
        (None, Some(a)) => alone(a, Provider::Local),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// `None` when the window's process can't be read (an anti-cheat can refuse that).
    pub exe: Option<String>,
    pub fullscreen: bool,
}

/// One second of what the slow layer can see. `None` means unknown, never 0.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Observation {
    pub foreground: Option<Foreground>,
    pub cpu_load_pct: Option<f64>,
    pub gpu_load_pct: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
}

/// Whether the local model must stay out (SPEC §7.2). It doesn't rely on recognising the game,
/// and whatever can't be seen counts as a game.
pub fn game_like(o: &Observation, known_game: bool) -> bool {
    known_game
        || o.foreground.as_ref().is_none_or(|f| f.fullscreen)
        || o.gpu_load_pct.is_none_or(|g| g > GAME_GPU_LOAD_PCT)
}

/// No fullscreen or game-like app, CPU < 30 % and GPU < 20 %, unbroken for 5 minutes.
#[derive(Debug, Default)]
pub struct IdleTracker {
    since: Option<Duration>,
    last: Option<Duration>,
}

impl IdleTracker {
    pub fn update(&mut self, now: Duration, o: &Observation, known_game: bool) -> bool {
        let quiet = !game_like(o, known_game)
            && o.cpu_load_pct.is_some_and(|c| c < IDLE_CPU_LOAD_PCT)
            && o.gpu_load_pct.is_some_and(|g| g < IDLE_GPU_LOAD_PCT);
        let gap = self
            .last
            .is_some_and(|l| now.saturating_sub(l) > MAX_OBSERVE_GAP);
        self.last = Some(now);
        if !quiet {
            self.since = None;
            return false;
        }
        if gap || self.since.is_none() {
            self.since = Some(now);
        }
        self.since
            .is_some_and(|s| now.saturating_sub(s) >= IDLE_FOR)
    }
}

/// SPEC §7.4: at most 6 slow-layer changes in any hour.
#[derive(Debug, Default)]
pub struct ChangeLimit(VecDeque<Duration>);

impl ChangeLimit {
    pub fn allowed(&mut self, now: Duration) -> bool {
        while self
            .0
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= HOUR)
        {
            self.0.pop_front();
        }
        self.0.len() < MAX_CHANGES_PER_HOUR
    }

    pub fn record(&mut self, now: Duration) {
        self.0.push_back(now);
    }
}

/// A cached answer for an exe.
#[derive(Debug, Clone, PartialEq)]
pub struct Known {
    pub workload: Workload,
    pub source: Source,
    pub confidence: Option<f64>,
}

/// A question waiting for the user.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuestion {
    pub exe: String,
    /// What a model answered below its threshold, if one did.
    pub suggestion: Option<Suggestion>,
}

/// The rule's result, for the daemon to validate and apply through the engine (S1).
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub exe: String,
    pub workload: Workload,
    pub profile: &'static str,
    pub source: Source,
    pub confidence: Option<f64>,
}

/// One entry for the decision log. `workload` is `None` when nothing could answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub exe: String,
    pub workload: Option<Workload>,
    pub source: Option<Source>,
    pub confidence: Option<f64>,
    pub asked: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub change: Option<Change>,
    pub decision: Option<Decision>,
}

#[derive(Default)]
struct Mean(f64, u32);

impl Mean {
    fn add(&mut self, v: Option<f64>) {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            self.0 += v;
            self.1 += 1;
        }
    }
    fn get(&self) -> Option<f64> {
        (self.1 > 0).then(|| self.0 / f64::from(self.1))
    }
}

/// Keeps `failed` so the slow layer can back off a local model that won't load.
struct Watched<'a> {
    inner: &'a mut dyn DecisionProvider,
    failed: bool,
}

impl DecisionProvider for Watched<'_> {
    fn ask(&mut self, q: &Question, cancel: &dyn Fn() -> bool) -> Result<Answer, AskError> {
        let r = self.inner.ask(q, cancel);
        self.failed = matches!(r, Err(AskError::Unavailable(_)));
        r
    }
}

pub struct SlowLayer {
    known: HashMap<String, Known>,
    learned: Vec<(String, Known)>,
    questions: Vec<PendingQuestion>,
    readings: VecDeque<(Duration, Observation)>,
    exe: Option<String>,
    exe_since: Duration,
    last_cycle: Option<Duration>,
    due: bool,
    idle: IdleTracker,
    was_idle: bool,
    limit: ChangeLimit,
    /// The profile the rule last put in place. It isn't proposed again, so a manual choice
    /// stands until the workload calls for a different profile.
    settled: Option<&'static str>,
    local_retry_at: Option<Duration>,
    last_decision: Option<Decision>,
}

impl SlowLayer {
    pub fn new(known: HashMap<String, Known>) -> Self {
        Self {
            known,
            learned: Vec::new(),
            questions: Vec::new(),
            readings: VecDeque::new(),
            exe: None,
            exe_since: Duration::ZERO,
            last_cycle: None,
            due: false,
            idle: IdleTracker::default(),
            was_idle: false,
            limit: ChangeLimit::default(),
            settled: None,
            local_retry_at: None,
            last_decision: None,
        }
    }

    pub fn questions(&self) -> &[PendingQuestion] {
        &self.questions
    }

    /// New per-exe answers for the daemon to persist.
    pub fn take_learned(&mut self) -> Vec<(String, Known)> {
        std::mem::take(&mut self.learned)
    }

    /// A cached answer loaded back, or the user's answer: it beats everything, and takes
    /// effect at the next step if that exe is in the foreground.
    pub fn set_known(&mut self, exe: &str, known: Known) {
        self.known.insert(exe.into(), known);
        self.questions.retain(|q| q.exe != exe);
        self.due = true;
    }

    pub fn is_known_game(&self, exe: &str) -> bool {
        self.known
            .get(exe)
            .map(|k| k.workload)
            .or_else(|| builtin_workload(exe))
            == Some(Workload::Gaming)
    }

    /// The exes cached as games, for a `cancel` that runs while `step` holds this layer.
    pub fn known_games(&self) -> HashSet<String> {
        self.known
            .iter()
            .filter(|(_, k)| k.workload == Workload::Gaming)
            .map(|(e, _)| e.clone())
            .collect()
    }

    /// Counts against the hourly limit.
    pub fn applied(&mut self, now: Duration, c: &Change) {
        self.limit.record(now);
        self.settled = Some(c.profile);
    }

    /// The profile was already active: nothing changed, so nothing counts.
    pub fn already_active(&mut self, c: &Change) {
        self.settled = Some(c.profile);
    }

    /// Records one second. Any change of foreground app starts this exe's averages over.
    pub fn observe(&mut self, now: Duration, o: &Observation) {
        let exe = o
            .foreground
            .as_ref()
            .and_then(|f| f.exe.as_deref())
            .and_then(clean_exe);
        if exe != self.exe {
            self.exe = exe;
            self.exe_since = now;
            self.readings.clear();
            self.due = true;
        }
        self.readings.push_back((now, o.clone()));
        while self
            .readings
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) >= STATE_WINDOW)
        {
            self.readings.pop_front();
        }
    }

    /// The foreground exe with its recent averages.
    pub fn current_state(&self) -> Option<ExeState> {
        let exe = self.exe.clone()?;
        let mean = |pick: fn(&Observation) -> Option<f64>| {
            let mut m = Mean::default();
            self.readings.iter().for_each(|(_, o)| m.add(pick(o)));
            m.get()
        };
        let newest = self.readings.back().map_or(self.exe_since, |(t, _)| *t);
        Some(ExeState {
            exe,
            seconds_observed: u32::try_from(newest.saturating_sub(self.exe_since).as_secs())
                .unwrap_or(u32::MAX),
            cpu_load_pct: mean(|o| o.cpu_load_pct),
            gpu_load_pct: mean(|o| o.gpu_load_pct),
            cpu_power_w: mean(|o| o.cpu_power_w),
            gpu_power_w: mean(|o| o.gpu_power_w),
            cpu_temp_c: mean(|o| o.cpu_temp_c),
            gpu_temp_c: mean(|o| o.gpu_temp_c),
        })
    }

    /// One second of the slow layer. It may block for as long as a provider takes, so the
    /// daemon runs it on its own thread and `cancel` must watch for a game by itself.
    pub fn step(
        &mut self,
        now: Duration,
        o: &Observation,
        hosted: Option<&mut dyn DecisionProvider>,
        local: Option<&mut dyn DecisionProvider>,
        cancel: &dyn Fn() -> bool,
    ) -> Step {
        self.observe(now, o);
        let known_game = self.exe.as_deref().is_some_and(|e| self.is_known_game(e));
        let idle = self.idle.update(now, o, known_game);
        // The local model has just become an option: an unanswered exe shouldn't wait.
        self.due |= idle && !self.was_idle;
        self.was_idle = idle;
        self.due |= self
            .last_cycle
            .is_none_or(|l| now.saturating_sub(l) >= CADENCE);
        let Some(exe) = self.exe.clone() else {
            return Step::default();
        };
        if !self.due {
            return Step::default();
        }
        let (workload, source, confidence) = if let Some(k) = self.known.get(&exe) {
            (k.workload, k.source, k.confidence)
        } else {
            if now.saturating_sub(self.exe_since) < SETTLE {
                return Step::default(); // still due
            }
            let local_ok = idle && self.local_retry_at.is_none_or(|t| now >= t);
            let mut watched = local.filter(|_| local_ok).map(|inner| Watched {
                inner,
                failed: false,
            });
            let routed = if self.has_model_suggestion(&exe) {
                None // already answered below the threshold: the user decides, at no further cost
            } else {
                self.current_state().map(|state| {
                    route_workload(
                        &state,
                        hosted,
                        watched.as_mut().map(|w| w as &mut dyn DecisionProvider),
                        cancel,
                    )
                })
            };
            if watched.is_some_and(|w| w.failed) {
                self.local_retry_at = Some(now + LOCAL_RETRY);
            }
            self.due = false;
            self.last_cycle = Some(now);
            match routed {
                None => return Step::default(),
                Some(WorkloadRoute::Ask { suggestion }) => {
                    return Step {
                        change: None,
                        decision: self.asks(&exe, suggestion),
                    };
                }
                Some(WorkloadRoute::Decided {
                    workload,
                    source,
                    confidence,
                }) => {
                    if source != Source::BuiltIn {
                        let k = Known {
                            workload,
                            source,
                            confidence,
                        };
                        self.known.insert(exe.clone(), k.clone());
                        self.learned.push((exe.clone(), k));
                    }
                    self.questions.retain(|q| q.exe != exe);
                    (workload, source, confidence)
                }
            }
        };
        self.due = false;
        self.last_cycle = Some(now);
        let decision = self.log(Decision {
            exe: exe.clone(),
            workload: Some(workload),
            source: Some(source),
            confidence,
            asked: false,
        });
        let profile = profile_for(workload);
        let wanted = self.settled != Some(profile) && self.limit.allowed(now);
        let change = wanted.then_some(Change {
            exe,
            workload,
            profile,
            source,
            confidence,
        });
        Step { change, decision }
    }

    fn has_model_suggestion(&self, exe: &str) -> bool {
        self.questions
            .iter()
            .any(|q| q.exe == exe && q.suggestion.is_some())
    }

    fn asks(&mut self, exe: &str, suggestion: Option<Suggestion>) -> Option<Decision> {
        if let Some(q) = self.questions.iter_mut().find(|q| q.exe == exe) {
            q.suggestion = suggestion;
        } else {
            if self.questions.len() == MAX_QUESTIONS {
                self.questions.remove(0);
            }
            self.questions.push(PendingQuestion {
                exe: exe.into(),
                suggestion,
            });
        }
        self.log(Decision {
            exe: exe.into(),
            workload: suggestion.map(|s| s.workload),
            source: suggestion.map(|s| s.source),
            confidence: suggestion.map(|s| s.confidence),
            asked: true,
        })
    }

    /// The same result minute after minute is one log entry.
    fn log(&mut self, d: Decision) -> Option<Decision> {
        if self.last_decision.as_ref() == Some(&d) {
            return None;
        }
        self.last_decision = Some(d.clone());
        Some(d)
    }
}

/// Why a rule-selected profile must not replace the active one, if so. `set_profile` hands back
/// any fan the new profile drops, and a curve on a missing sensor trips S3 (100%). Bundled
/// profiles use role ids until M13, so without this a switch could release the real fan.
pub fn why_not_switch(next: &Profile, active: &Profile, snap: &HardwareSnapshot) -> Option<String> {
    let controlled: HashSet<&str> = next.controls.iter().map(|c| c.fan.as_str()).collect();
    let real = |id: &str| snap.fans.iter().any(|f| f.id == id);
    if let Some(c) = active
        .controls
        .iter()
        .find(|c| real(&c.fan) && !controlled.contains(c.fan.as_str()))
    {
        return Some(format!(
            "profile '{}' does not control fan '{}'",
            next.name, c.fan
        ));
    }
    if let Some(f) = controlled.iter().find(|f| !real(f)) {
        return Some(format!(
            "profile '{}' controls unknown fan '{f}'",
            next.name
        ));
    }
    let custom: HashSet<&str> = next.sensors.iter().map(custom_id).collect();
    let readable = |id: &str| custom.contains(id) || snap.sensors.iter().any(|s| s.id == id);
    let mut sources: Vec<&str> = Vec::new();
    for c in &next.curves {
        if let Curve::Linear { source, .. }
        | Curve::Graph { source, .. }
        | Curve::Trigger { source, .. } = c
        {
            sources.push(source);
        }
    }
    for s in &next.sensors {
        match s {
            CustomSensor::TimeAverage { source, .. } | CustomSensor::Offset { source, .. } => {
                sources.push(source);
            }
            CustomSensor::Mix { sources: many, .. } => {
                sources.extend(many.iter().map(String::as_str))
            }
        }
    }
    sources
        .into_iter()
        .find(|s| !readable(s))
        .map(|s| format!("profile '{}' reads missing sensor '{s}'", next.name))
}

fn custom_id(s: &CustomSensor) -> &str {
    match s {
        CustomSensor::TimeAverage { id, .. }
        | CustomSensor::Offset { id, .. }
        | CustomSensor::Mix { id, .. } => id,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::classifier::Workload::{Browsing, Compiling, Gaming, Idle, Other, Rendering};
    use std::cell::Cell;
    use std::rc::Rc;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Answers with a script, and counts how often it was asked.
    #[derive(Clone)]
    struct Fake {
        reply: Result<(&'static str, f64), AskError>,
        calls: Rc<Cell<u32>>,
    }

    impl Fake {
        fn says(choice: &'static str, confidence: f64) -> Self {
            Self {
                reply: Ok((choice, confidence)),
                calls: Rc::default(),
            }
        }
        fn down() -> Self {
            Self {
                reply: Err(AskError::Unavailable("offline".into())),
                calls: Rc::default(),
            }
        }
    }

    impl DecisionProvider for Fake {
        fn ask(&mut self, _: &Question, _: &dyn Fn() -> bool) -> Result<Answer, AskError> {
            self.calls.set(self.calls.get() + 1);
            self.reply.clone().map(|(c, confidence)| Answer {
                choice: c.into(),
                confidence,
            })
        }
    }

    fn state(exe: &str) -> ExeState {
        ExeState {
            exe: exe.into(),
            seconds_observed: 10,
            cpu_load_pct: Some(20.0),
            gpu_load_pct: None,
            cpu_power_w: None,
            gpu_power_w: Some(80.0),
            cpu_temp_c: Some(55.0),
            gpu_temp_c: None,
        }
    }

    fn route(exe: &str, hosted: Option<&mut Fake>, local: Option<&mut Fake>) -> WorkloadRoute {
        route_workload(
            &state(exe),
            hosted.map(|f| f as &mut dyn DecisionProvider),
            local.map(|f| f as &mut dyn DecisionProvider),
            &|| false,
        )
    }

    // ---- the fixed rule (SPEC §7.2) ----

    #[test]
    fn the_approved_workload_to_profile_rule() {
        let table = [
            (Idle, "silent"),
            (Browsing, "efficiency"),
            (Gaming, "gaming"),
            (Compiling, "performance"),
            (Rendering, "performance"),
            (Other, "balanced"),
        ];
        for (w, p) in table {
            assert_eq!(profile_for(w), p);
        }
    }

    // ---- routing order ----

    #[test]
    fn a_confident_hosted_answer_wins_and_nothing_else_is_asked() {
        let (mut h, mut l) = (Fake::says("gaming", 0.9), Fake::says("other", 0.99));
        let r = route("cs2.exe", Some(&mut h), Some(&mut l));
        assert_eq!(
            r,
            WorkloadRoute::Decided {
                workload: Gaming,
                source: Source::Hosted,
                confidence: Some(0.9)
            }
        );
        assert_eq!(l.calls.get(), 0);
    }

    #[test]
    fn hosted_down_falls_to_local_then_the_list_then_ask() {
        let (mut h, mut l) = (Fake::down(), Fake::says("compiling", 0.8));
        assert!(matches!(
            route("cargo.exe", Some(&mut h), Some(&mut l)),
            WorkloadRoute::Decided {
                source: Source::Local,
                ..
            }
        ));
        let (mut h, mut l) = (Fake::down(), Fake::down());
        assert_eq!(
            route("cargo.exe", Some(&mut h), Some(&mut l)),
            WorkloadRoute::Decided {
                workload: Compiling,
                source: Source::BuiltIn,
                confidence: None
            },
            "a built-in answer carries no model confidence"
        );
        assert_eq!(
            route("unknown_tool.exe", Some(&mut h), Some(&mut l)),
            WorkloadRoute::Ask { suggestion: None },
            "nothing can answer: asked, nothing faked (S8)"
        );
    }

    #[test]
    fn no_key_and_not_idle_skips_straight_to_the_list() {
        assert!(matches!(
            route("chrome.exe", None, None),
            WorkloadRoute::Decided {
                workload: Browsing,
                source: Source::BuiltIn,
                ..
            }
        ));
    }

    #[test]
    fn a_low_confidence_answer_asks_and_does_not_shop_for_a_better_one() {
        let (mut h, mut l) = (Fake::says("gaming", 0.3), Fake::says("gaming", 0.99));
        assert_eq!(
            route("chrome.exe", Some(&mut h), Some(&mut l)),
            WorkloadRoute::Ask {
                suggestion: Some(Suggestion {
                    workload: Gaming,
                    source: Source::Hosted,
                    confidence: 0.3
                })
            }
        );
        assert_eq!(l.calls.get(), 0);
    }

    #[test]
    fn thresholds_are_per_provider_and_bigger_changes_need_more() {
        for p in [Provider::Hosted, Provider::Local] {
            let t = p.thresholds();
            assert!(t.workload < t.big, "{p:?}");
        }
        // each provider is judged on its own scale: the spike's local answers ran much lower
        assert_ne!(
            Provider::Hosted.thresholds().workload,
            Provider::Local.thresholds().workload
        );
    }

    #[test]
    fn an_answer_outside_the_options_or_with_a_bad_confidence_counts_as_no_answer() {
        for bad in [("banana", 0.99), ("gaming", f64::NAN), ("gaming", 1.5)] {
            let mut h = Fake::says(bad.0, bad.1);
            assert!(
                matches!(
                    route("cargo.exe", Some(&mut h), None),
                    WorkloadRoute::Decided {
                        source: Source::BuiltIn,
                        ..
                    }
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_cancelled_call_falls_through_like_any_other_failure() {
        let mut l = Fake {
            reply: Err(AskError::Cancelled),
            calls: Rc::default(),
        };
        assert_eq!(
            route("unknown_tool.exe", None, Some(&mut l)),
            WorkloadRoute::Ask { suggestion: None }
        );
    }

    // ---- built-in list ----

    #[test]
    fn the_built_in_list_is_case_insensitive_and_does_not_guess() {
        assert_eq!(builtin_workload("CS2.EXE"), Some(Gaming));
        assert_eq!(builtin_workload("rustc.exe"), Some(Compiling));
        assert_eq!(builtin_workload("blender.exe"), Some(Rendering));
        assert_eq!(builtin_workload("notchrome.exe"), None);
        assert_eq!(builtin_workload(""), None);
    }

    // ---- the payload: numbers and exe names only (SPEC §7.2) ----

    #[test]
    fn the_question_carries_the_exe_and_averages_only_and_never_a_zero_for_missing() {
        let q = workload_question(state("cs2.exe"));
        assert_eq!(q.id, "workload");
        assert_eq!(q.options.len(), 6);
        assert_eq!(q.state.exe, "cs2.exe");
        assert_eq!(q.state.gpu_load_pct, None, "missing stays missing");
    }

    #[test]
    fn exe_names_must_be_a_bare_file_name() {
        assert_eq!(
            clean_exe("C:\\Users\\sam\\game.EXE"),
            None,
            "a path leaks the user name"
        );
        assert_eq!(clean_exe("/home/sam/game"), None);
        assert_eq!(clean_exe(""), None);
        assert_eq!(clean_exe(&"a".repeat(300)), None);
        assert_eq!(clean_exe("bad\nname.exe"), None);
        assert_eq!(clean_exe("Game.EXE").as_deref(), Some("game.exe"));
    }

    // ---- big calls ----

    fn big(hosted: Option<&mut Fake>, local: Option<&mut Fake>) -> BigOutcome {
        decide_big(
            &accept_question(state("cs2.exe"), "Accept this learned curve?"),
            hosted.map(|f| f as &mut dyn DecisionProvider),
            local.map(|f| f as &mut dyn DecisionProvider),
            &|| false,
        )
    }

    #[test]
    fn two_providers_agreeing_apply_and_disagreeing_ask() {
        let (mut h, mut l) = (Fake::says("accept", 0.51), Fake::says("accept", 0.51));
        assert_eq!(
            big(Some(&mut h), Some(&mut l)),
            BigOutcome::Apply("accept".into())
        );
        let (mut h, mut l) = (Fake::says("accept", 0.99), Fake::says("reject", 0.99));
        assert_eq!(big(Some(&mut h), Some(&mut l)), BigOutcome::Ask);
    }

    #[test]
    fn one_provider_alone_needs_its_big_change_threshold() {
        let t = Provider::Hosted.thresholds().big;
        let mut h = Fake::says("accept", t);
        assert_eq!(big(Some(&mut h), None), BigOutcome::Apply("accept".into()));
        let mut h = Fake::says("accept", t - 0.01);
        assert_eq!(big(Some(&mut h), None), BigOutcome::Ask);
        let t = Provider::Local.thresholds().big;
        let mut l = Fake::says("reject", t);
        assert_eq!(big(None, Some(&mut l)), BigOutcome::Apply("reject".into()));
        let mut l = Fake::says("reject", t - 0.01);
        assert_eq!(big(None, Some(&mut l)), BigOutcome::Ask);
    }

    #[test]
    fn a_second_provider_that_fails_leaves_the_single_provider_rule() {
        let (mut h, mut l) = (Fake::says("accept", 0.4), Fake::down());
        assert_eq!(big(Some(&mut h), Some(&mut l)), BigOutcome::Ask);
        let t = Provider::Hosted.thresholds().big;
        let (mut h, mut l) = (Fake::says("accept", t), Fake::down());
        assert_eq!(
            big(Some(&mut h), Some(&mut l)),
            BigOutcome::Apply("accept".into())
        );
    }

    #[test]
    fn a_big_call_nothing_can_answer_is_asked_and_the_list_never_answers_it() {
        assert_eq!(big(None, None), BigOutcome::Ask);
        let (mut h, mut l) = (Fake::down(), Fake::down());
        assert_eq!(big(Some(&mut h), Some(&mut l)), BigOutcome::Ask);
    }

    // ---- game-like and idle (Sam's review of PR #54) ----

    fn obs(exe: Option<&str>, fullscreen: bool, cpu: Option<f64>, gpu: Option<f64>) -> Observation {
        Observation {
            foreground: Some(Foreground {
                exe: exe.map(Into::into),
                fullscreen,
            }),
            cpu_load_pct: cpu,
            gpu_load_pct: gpu,
            ..Observation::default()
        }
    }

    fn quiet() -> Observation {
        obs(Some("notepad.exe"), false, Some(5.0), Some(3.0))
    }

    #[test]
    fn game_like_does_not_rely_on_the_list() {
        assert!(!game_like(&quiet(), false));
        assert!(game_like(&quiet(), true), "a known game");
        assert!(
            game_like(
                &obs(Some("new_game.exe"), true, Some(5.0), Some(3.0)),
                false
            ),
            "fullscreen"
        );
        assert!(
            game_like(
                &obs(Some("new_game.exe"), false, Some(5.0), Some(51.0)),
                false
            ),
            "GPU over 50%"
        );
        assert!(
            !game_like(&obs(Some("x.exe"), false, Some(5.0), Some(50.0)), false),
            "the rule is over 50%"
        );
    }

    #[test]
    fn what_cannot_be_seen_counts_as_a_game() {
        let no_helper = Observation {
            foreground: None,
            ..quiet()
        };
        assert!(game_like(&no_helper, false), "foreground unknown");
        assert!(
            game_like(&obs(Some("x.exe"), false, Some(5.0), None), false),
            "GPU load unknown"
        );
    }

    /// Quiet, one observation a second, for `range`. Returns the last answer.
    fn walk(i: &mut IdleTracker, range: std::ops::RangeInclusive<u64>) -> bool {
        let mut last = false;
        for t in range {
            last = i.update(secs(t), &quiet(), false);
        }
        last
    }

    #[test]
    fn idle_needs_five_quiet_minutes() {
        let mut i = IdleTracker::default();
        assert!(!walk(&mut i, 0..=299));
        assert!(walk(&mut i, 300..=300));
    }

    #[test]
    fn anything_loud_restarts_the_five_minutes() {
        let loud = [
            obs(Some("x.exe"), false, Some(30.0), Some(3.0)), // the rule is under 30%
            obs(Some("x.exe"), false, Some(5.0), Some(20.0)), // the rule is under 20%
            obs(Some("x.exe"), true, Some(5.0), Some(3.0)),
            obs(Some("x.exe"), false, None, Some(3.0)),
        ];
        for o in &loud {
            let mut i = IdleTracker::default();
            walk(&mut i, 0..=299);
            assert!(!i.update(secs(300), o, false), "{o:?}");
            assert!(!walk(&mut i, 301..=600), "{o:?}");
            assert!(walk(&mut i, 601..=601), "{o:?}");
        }
    }

    #[test]
    fn a_gap_in_observing_restarts_the_five_minutes() {
        let mut i = IdleTracker::default();
        i.update(secs(0), &quiet(), false);
        assert!(!i.update(secs(400), &quiet(), false), "unseen in between");
    }

    // ---- 6 changes an hour ----

    #[test]
    fn six_changes_an_hour_in_a_rolling_window() {
        let mut r = ChangeLimit::default();
        for t in 0..6 {
            assert!(r.allowed(secs(t * 60)));
            r.record(secs(t * 60));
        }
        assert!(!r.allowed(secs(360)));
        assert!(!r.allowed(secs(3599)));
        assert!(r.allowed(secs(3600)), "the first change has left the hour");
    }

    // ---- the slow layer: cadence, cache, questions ----

    fn seen(exe: &str) -> Observation {
        obs(Some(exe), false, Some(60.0), Some(70.0))
    }

    struct Rig {
        s: SlowLayer,
        hosted: Fake,
        local: Fake,
    }

    impl Rig {
        fn new(hosted: Fake, local: Fake) -> Self {
            Self {
                s: SlowLayer::new(HashMap::new()),
                hosted,
                local,
            }
        }
        fn step(&mut self, t: u64, o: &Observation) -> Step {
            self.s.step(
                secs(t),
                o,
                Some(&mut self.hosted),
                Some(&mut self.local),
                &|| false,
            )
        }
    }

    #[test]
    fn a_new_exe_settles_for_ten_seconds_before_it_is_asked_about() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        for t in 0..10 {
            assert_eq!(r.step(t, &seen("a.exe")), Step::default(), "t={t}");
        }
        assert_eq!(r.hosted.calls.get(), 0);
        let s = r.step(10, &seen("a.exe"));
        assert_eq!(r.hosted.calls.get(), 1);
        assert_eq!(s.change.unwrap().profile, "gaming");
    }

    #[test]
    fn an_alt_tab_flicker_costs_nothing() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        for t in 0..30 {
            let exe = if t % 2 == 0 { "a.exe" } else { "b.exe" };
            r.step(t, &seen(exe));
        }
        assert_eq!(r.hosted.calls.get(), 0);
    }

    #[test]
    fn a_confident_answer_is_cached_per_exe_and_never_asked_again() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        for t in 0..11 {
            r.step(t, &seen("a.exe"));
        }
        let learned = r.s.take_learned();
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].0, "a.exe");
        assert_eq!(learned[0].1.workload, Gaming);
        for t in 11..200 {
            r.step(t, &seen("a.exe"));
        }
        assert_eq!(r.hosted.calls.get(), 1);
    }

    #[test]
    fn a_cached_exe_applies_at_once_when_it_comes_back() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        r.s.set_known(
            "a.exe",
            Known {
                workload: Gaming,
                source: Source::User,
                confidence: None,
            },
        );
        let s = r.step(0, &seen("a.exe"));
        assert_eq!(s.change.unwrap().profile, "gaming");
        assert_eq!(r.hosted.calls.get(), 0);
    }

    #[test]
    fn a_change_is_only_proposed_once_for_the_same_app_so_a_manual_choice_stands() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        r.s.set_known(
            "a.exe",
            Known {
                workload: Gaming,
                source: Source::User,
                confidence: None,
            },
        );
        let c = r.step(0, &seen("a.exe")).change.unwrap();
        r.s.applied(secs(0), &c);
        for t in 1..300 {
            assert!(r.step(t, &seen("a.exe")).change.is_none());
        }
        // another app with the same workload needs no change either
        r.s.set_known(
            "same.exe",
            Known {
                workload: Gaming,
                source: Source::User,
                confidence: None,
            },
        );
        assert!(r.step(300, &seen("same.exe")).change.is_none());
        r.s.set_known(
            "b.exe",
            Known {
                workload: Compiling,
                source: Source::User,
                confidence: None,
            },
        );
        let c = r.step(301, &seen("b.exe")).change.unwrap();
        assert_eq!(c.profile, "performance");
    }

    #[test]
    fn a_profile_that_was_already_active_uses_up_no_change() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        r.s.set_known(
            "a.exe",
            Known {
                workload: Gaming,
                source: Source::User,
                confidence: None,
            },
        );
        let c = r.step(0, &seen("a.exe")).change.unwrap();
        r.s.already_active(&c);
        assert!(r.step(60, &seen("a.exe")).change.is_none());
        for t in 0..6 {
            assert!(r.s.limit.allowed(secs(61 + t)));
            r.s.limit.record(secs(61 + t));
        }
        assert!(!r.s.limit.allowed(secs(70)), "exactly six were left");
    }

    #[test]
    fn a_change_that_could_not_be_applied_is_retried_at_the_next_cycle_not_every_second() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        r.s.set_known(
            "a.exe",
            Known {
                workload: Gaming,
                source: Source::User,
                confidence: None,
            },
        );
        assert!(r.step(0, &seen("a.exe")).change.is_some());
        for t in 1..60 {
            assert!(r.step(t, &seen("a.exe")).change.is_none(), "t={t}");
        }
        assert!(r.step(60, &seen("a.exe")).change.is_some());
    }

    #[test]
    fn the_seventh_change_in_an_hour_is_not_proposed() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        let exes = ["a.exe", "b.exe"];
        for (exe, workload) in exes.into_iter().zip([Gaming, Browsing]) {
            r.s.set_known(
                exe,
                Known {
                    workload,
                    source: Source::User,
                    confidence: None,
                },
            );
        }
        let (mut t, mut made) = (0, 0);
        for n in 0..10 {
            let s = r.step(t, &seen(exes[n % 2]));
            if let Some(c) = s.change {
                r.s.applied(secs(t), &c);
                made += 1;
            }
            t += 60;
        }
        assert_eq!(made, 6);
    }

    #[test]
    fn a_low_confidence_answer_queues_one_question_and_stops_asking() {
        let mut r = Rig::new(Fake::says("gaming", 0.2), Fake::down());
        let mut change = None;
        for t in 0..200 {
            change = change.or(r.step(t, &seen("a.exe")).change);
        }
        assert!(change.is_none(), "below the threshold, nothing changes");
        assert_eq!(r.hosted.calls.get(), 1, "the cost stays one call");
        let q = r.s.questions();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].exe, "a.exe");
        assert_eq!(
            q[0].suggestion.unwrap(),
            Suggestion {
                workload: Gaming,
                source: Source::Hosted,
                confidence: 0.2
            }
        );
    }

    #[test]
    fn with_nothing_to_answer_it_queues_a_question_and_retries_at_the_cadence() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        for t in 0..71 {
            let s = r.step(t, &seen("mystery.exe"));
            assert!(s.change.is_none(), "nothing pretends to have decided");
        }
        assert_eq!(r.s.questions().len(), 1, "deduplicated");
        assert_eq!(r.hosted.calls.get(), 2, "t=10 and t=70");
        // the provider comes back: the question goes away and the answer applies
        r.hosted = Fake::says("rendering", 0.95);
        let mut c = None;
        for t in 71..131 {
            c = c.or(r.step(t, &seen("mystery.exe")).change);
        }
        assert_eq!(c.unwrap().profile, "performance");
        assert!(r.s.questions().is_empty());
    }

    #[test]
    fn the_users_answer_resolves_the_question_and_beats_everything() {
        let mut r = Rig::new(Fake::says("gaming", 0.2), Fake::down());
        for t in 0..11 {
            r.step(t, &seen("a.exe"));
        }
        r.s.set_known(
            "a.exe",
            Known {
                workload: Compiling,
                source: Source::User,
                confidence: None,
            },
        );
        assert!(r.s.questions().is_empty());
        let c = r.step(11, &seen("a.exe")).change.unwrap();
        assert_eq!(c.profile, "performance");
        assert_eq!(c.source, Source::User);
    }

    #[test]
    fn a_built_in_answer_is_a_default_it_is_not_cached_and_a_model_can_replace_it() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        let mut first = None;
        for t in 0..11 {
            first = first.or(r.step(t, &seen("chrome.exe")).change);
        }
        let first = first.unwrap();
        assert_eq!(first.source, Source::BuiltIn);
        assert_eq!(first.confidence, None);
        r.s.applied(secs(10), &first);
        assert!(r.s.take_learned().is_empty(), "not cached");
        r.hosted = Fake::says("other", 0.9);
        let mut c = None;
        for t in 11..80 {
            c = c.or(r.step(t, &seen("chrome.exe")).change);
        }
        let c = c.unwrap();
        assert_eq!((c.profile, c.source), ("balanced", Source::Hosted));
    }

    #[test]
    fn questions_are_bounded() {
        let mut r = Rig::new(Fake::down(), Fake::down());
        let mut t = 0;
        for n in 0..(MAX_QUESTIONS + 5) {
            let exe = format!("app{n}.exe");
            for _ in 0..11 {
                r.step(t, &seen(&exe));
                t += 1;
            }
        }
        assert_eq!(r.s.questions().len(), MAX_QUESTIONS);
        assert_eq!(r.s.questions()[0].exe, "app5.exe", "the oldest went first");
    }

    #[test]
    fn nothing_is_decided_without_a_foreground_app() {
        let mut r = Rig::new(Fake::says("gaming", 0.9), Fake::down());
        let none = Observation::default();
        let no_exe = obs(None, false, Some(5.0), Some(3.0));
        for t in 0..200 {
            let o = if t % 2 == 0 { &none } else { &no_exe };
            assert_eq!(r.step(t, o), Step::default());
        }
        assert_eq!(r.hosted.calls.get(), 0);
    }

    // ---- the local model and games (SPEC §7.2, §7.5) ----

    fn quiet_rig(hosted: Fake, local: Fake) -> Rig {
        let mut r = Rig::new(hosted, local);
        for t in 0..300 {
            r.step(t, &obs(Some("notepad.exe"), false, Some(5.0), Some(3.0)));
        }
        r
    }

    #[test]
    fn the_local_model_is_not_asked_until_the_pc_has_been_idle() {
        let mut r = Rig::new(Fake::down(), Fake::says("other", 0.9));
        for t in 0..299 {
            r.step(t, &obs(Some("notepad.exe"), false, Some(5.0), Some(3.0)));
        }
        assert_eq!(r.local.calls.get(), 0);
        r.step(300, &obs(Some("notepad.exe"), false, Some(5.0), Some(3.0)));
        assert_eq!(r.local.calls.get(), 1);
    }

    #[test]
    fn the_local_model_does_not_start_during_a_game_it_does_not_recognise() {
        let mut r = quiet_rig(Fake::down(), Fake::says("other", 0.9));
        let calls = r.local.calls.get();
        for t in 300..1000 {
            r.step(
                t,
                &obs(Some("brand_new_game.exe"), true, Some(5.0), Some(3.0)),
            );
        }
        assert_eq!(r.local.calls.get(), calls, "fullscreen");
        for t in 1000..1700 {
            r.step(
                t,
                &obs(Some("windowed_game.exe"), false, Some(5.0), Some(90.0)),
            );
        }
        assert_eq!(r.local.calls.get(), calls, "GPU load");
    }

    #[test]
    fn the_local_model_does_not_start_for_a_game_it_already_knows() {
        let mut r = quiet_rig(Fake::down(), Fake::says("other", 0.9));
        let calls = r.local.calls.get();
        // windowed, quiet and idle by the numbers, but the list says it's a game
        for t in 300..1000 {
            r.step(t, &obs(Some("cs2.exe"), false, Some(5.0), Some(3.0)));
        }
        assert_eq!(r.local.calls.get(), calls);
    }

    #[test]
    fn a_failing_local_model_is_not_reloaded_every_minute() {
        let mut r = quiet_rig(Fake::down(), Fake::down());
        for t in 300..1200 {
            r.step(t, &obs(Some("mystery.exe"), false, Some(5.0), Some(3.0)));
        }
        // once right after the five idle minutes, then at most once per LOCAL_RETRY
        let most = 1 + 900 / LOCAL_RETRY.as_secs() as u32 + 1;
        assert!(r.local.calls.get() <= most, "{}", r.local.calls.get());
    }

    // ---- the state: exe-centric averages ----

    fn reading(cpu: f64, gpu_temp: f64) -> Observation {
        Observation {
            cpu_load_pct: Some(cpu),
            gpu_temp_c: Some(gpu_temp),
            ..seen("a.exe")
        }
    }

    #[test]
    fn the_state_averages_this_exes_recent_readings_only() {
        let mut s = SlowLayer::new(HashMap::new());
        for t in 0..5 {
            s.observe(
                secs(t),
                &Observation {
                    cpu_load_pct: Some(99.0),
                    ..seen("other.exe")
                },
            );
        }
        for t in 5..15 {
            s.observe(secs(t), &reading(if t < 10 { 10.0 } else { 30.0 }, 60.0));
        }
        let st = s.current_state().unwrap();
        assert_eq!(st.exe, "a.exe");
        assert_eq!(st.seconds_observed, 9);
        assert_eq!(st.cpu_load_pct, Some(20.0), "no other.exe in it");
        assert_eq!(st.gpu_temp_c, Some(60.0));
        assert_eq!(st.cpu_temp_c, None, "never a reading: missing, not 0");
    }

    #[test]
    fn the_state_only_looks_back_over_the_window() {
        let mut s = SlowLayer::new(HashMap::new());
        for t in 0..200 {
            let cpu = if t < 200 - STATE_WINDOW.as_secs() {
                90.0
            } else {
                10.0
            };
            s.observe(secs(t), &reading(cpu, 60.0));
        }
        assert_eq!(s.current_state().unwrap().cpu_load_pct, Some(10.0));
    }

    // ---- never fan-level: a rule-selected profile must cover this PC ----

    fn profile(name: &str, fans: &[&str], source: &str) -> Profile {
        let controls: String = fans
            .iter()
            .map(|f| format!("[[controls]]\nfan = \"{f}\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n"))
            .collect();
        Profile::from_toml(&format!(
            "name = \"{name}\"\n[[curves]]\ntype = \"linear\"\nid = \"c\"\nsource = \"{source}\"\nmin_temp = 30.0\nmax_temp = 90.0\nmin_pwm = 20\nmax_pwm = 100\n{controls}"
        ))
        .unwrap()
    }

    fn snapshot() -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![thermaliq_common::SensorReading {
                id: "nvml_gpu0_temp".into(),
                label: "gpu".into(),
                kind: thermaliq_common::SensorKind::Gpu,
                temp_celsius: 50.0,
            }],
            fans: vec![thermaliq_common::FanReading {
                id: "nvml_gpu0_fan0".into(),
                label: "fan".into(),
                rpm: 1000,
                duty_percent: 40,
                kind: thermaliq_common::FanKind::Fan,
            }],
            metrics: vec![],
        }
    }

    #[test]
    fn a_profile_that_covers_this_pcs_fans_may_replace_the_active_one() {
        let active = profile("a", &["nvml_gpu0_fan0"], "nvml_gpu0_temp");
        let next = profile("b", &["nvml_gpu0_fan0"], "nvml_gpu0_temp");
        assert_eq!(why_not_switch(&next, &active, &snapshot()), None);
    }

    #[test]
    fn a_bundled_profile_with_role_ids_would_hand_the_real_fan_back_and_is_refused() {
        let active = profile("a", &["nvml_gpu0_fan0"], "nvml_gpu0_temp");
        let bundled = profile("gaming", &["gpu1"], "nvml_gpu0_temp");
        let why = why_not_switch(&bundled, &active, &snapshot()).unwrap();
        assert!(why.contains("nvml_gpu0_fan0"), "{why}");
    }

    #[test]
    fn a_profile_reading_a_missing_sensor_would_trip_s3_and_is_refused() {
        let active = profile("a", &["nvml_gpu0_fan0"], "nvml_gpu0_temp");
        let next = profile("b", &["nvml_gpu0_fan0"], "cpu_package");
        let why = why_not_switch(&next, &active, &snapshot()).unwrap();
        assert!(why.contains("cpu_package"), "{why}");
    }

    #[test]
    fn workload_names_round_trip() {
        for w in Workload::ALL {
            assert_eq!(Workload::from_name(w.name()), Some(w));
        }
        assert_eq!(Workload::from_name("banana"), None);
    }
}
