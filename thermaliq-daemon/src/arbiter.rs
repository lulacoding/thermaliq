//! Which profile is in effect (SPEC §4.1, M15). One authoritative place: the winning profile
//! comes from, in order, an active alert, a foreground app rule, a live Agent profile intent,
//! a schedule, and the user's chosen fallback. The engine applies whatever this selects; nothing else decides.

use std::sync::Arc;
use std::time::Duration;
use thermaliq_agent::decision::ChangeLimit;
use thermaliq_common::{
    pick_app_rule, pick_schedule, AppRuleEntry, DaemonConfig, LocalClock, LocalTime, ScheduleEntry,
    Weekday,
};

/// How long an Agent profile intent lives without a renewal (two missed 1 s renewals expire it).
pub const LEASE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    /// An active `apply_profile` alert (SPEC §4.1, M16): above every other source.
    Alert,
    App,
    Agent,
    Schedule,
    Fallback,
}

impl ProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::App => "app",
            Self::Agent => "agent",
            Self::Schedule => "schedule",
            Self::Fallback => "fallback",
        }
    }
}

/// What `/status` and `POST /profiles/active` report: the user's choice and what wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub chosen: String,
    pub effective: String,
    pub source: ProfileSource,
    pub rule_id: Option<String>,
}

/// Bumped when the foreground executable changes, when the app rules or schedules are replaced,
/// and when the winning profile changes. Work started under an older tuple is discarded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Generations {
    pub foreground: u64,
    pub config: u64,
    pub profile: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// The same live intent, extended.
    Renewed,
    /// A new intent was registered.
    Activated,
    /// The foreground or the configuration changed since the decision was started.
    Stale,
    /// Six Agent-selected changes already happened in the last hour.
    LimitReached,
}

struct Lease {
    profile: String,
    foreground: u64,
    config: u64,
    expires: Duration,
    /// Registered while another profile was effective: the first successful apply counts.
    uncounted: bool,
}

pub struct Arbiter {
    app_rules: Vec<AppRuleEntry>,
    schedules: Vec<ScheduleEntry>,
    chosen: String,
    clock: Arc<dyn LocalClock>,
    gens: Generations,
    foreground: Option<String>,
    intent: Option<Lease>,
    /// `(alert id, profile)` while an `apply_profile` alert is active. It has no lease and no
    /// hourly budget: the alert's own fire count and hysteresis limit flapping, and a cap must
    /// never stop a cooling profile.
    alert: Option<(String, String)>,
    limit: ChangeLimit,
    winner: Option<String>,
    last: Option<Selection>,
}

pub type Usable<'a> = &'a dyn Fn(&str) -> bool;

impl Arbiter {
    pub fn new(chosen: &str, clock: Arc<dyn LocalClock>) -> Self {
        Self {
            app_rules: Vec::new(),
            schedules: Vec::new(),
            chosen: chosen.into(),
            clock,
            gens: Generations::default(),
            foreground: None,
            intent: None,
            alert: None,
            limit: ChangeLimit::default(),
            winner: None,
            last: None,
        }
    }

    /// The selection the engine last applied, for `/status`.
    pub fn last(&self) -> Option<&Selection> {
        self.last.as_ref()
    }

    pub fn generations(&self) -> Generations {
        self.gens
    }

    pub fn chosen(&self) -> &str {
        &self.chosen
    }

    /// `exe` is the cleaned foreground executable. A change starts a new foreground generation.
    pub fn set_foreground(&mut self, exe: Option<&str>) {
        let same = match (&self.foreground, exe) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.foreground = exe.map(str::to_owned);
            self.gens.foreground += 1;
        }
    }

    /// The user's fallback always follows `default_profile`; the config generation moves only
    /// when the app rules or schedules actually change.
    pub fn replace_config(&mut self, cfg: &DaemonConfig) {
        self.chosen.clone_from(&cfg.default_profile);
        if self.app_rules != cfg.app_rule || self.schedules != cfg.schedule {
            self.app_rules.clone_from(&cfg.app_rule);
            self.schedules.clone_from(&cfg.schedule);
            self.gens.config += 1;
        }
    }

    /// The profile an active alert asks for, or `None` when none is active. Set every pass.
    pub fn set_alert(&mut self, wanted: Option<(String, String)>) {
        self.alert = wanted;
    }

    pub fn choose(&mut self, name: &str) {
        self.chosen = name.into();
    }

    fn live(&self, now: Duration) -> Option<&Lease> {
        self.intent.as_ref().filter(|l| {
            l.foreground == self.gens.foreground && l.config == self.gens.config && now < l.expires
        })
    }

    /// Registers or renews the Agent's profile intent. `seen` is the generation tuple the decision
    /// started under. A new intent is an activation and is checked against the hourly limit;
    /// it counts only if it changes the effective profile (`agent_applied`).
    pub fn offer_intent(
        &mut self,
        now: Duration,
        profile: &str,
        seen: Generations,
        usable: Usable,
    ) -> Offer {
        if seen.foreground != self.gens.foreground || seen.config != self.gens.config {
            return Offer::Stale;
        }
        if self.live(now).is_some_and(|l| l.profile == profile) {
            if let Some(l) = self.intent.as_mut() {
                l.expires = now + LEASE;
            }
            return Offer::Renewed;
        }
        if !self.limit.allowed(now) {
            self.intent = None;
            return Offer::LimitReached;
        }
        self.intent = None;
        let uncounted = self.pick(now, usable).effective != profile;
        self.intent = Some(Lease {
            profile: profile.into(),
            foreground: self.gens.foreground,
            config: self.gens.config,
            expires: now + LEASE,
            uncounted,
        });
        Offer::Activated
    }

    /// The engine took an Agent-selected profile: that counts toward six an hour, once per intent.
    pub fn agent_applied(&mut self, now: Duration) {
        if let Some(l) = self.intent.as_mut().filter(|l| l.uncounted) {
            l.uncounted = false;
            self.limit.record(now);
        }
    }

    /// Pause, an Agent crash or supervisor loss: the intent is gone. App rules and schedules go on.
    pub fn invalidate_agent(&mut self) {
        self.intent = None;
    }

    fn pick(&self, now: Duration, usable: Usable) -> Selection {
        let chosen = self.chosen.clone();
        let won = |effective: &str, source, rule_id: Option<&str>| Selection {
            chosen: chosen.clone(),
            effective: effective.into(),
            source,
            rule_id: rule_id.map(str::to_owned),
        };
        if let Some((id, profile)) = self.alert.as_ref().filter(|(_, p)| usable(p)) {
            return won(profile, ProfileSource::Alert, Some(id));
        }
        if let Some(r) = self
            .foreground
            .as_deref()
            .and_then(|exe| pick_app_rule(&self.app_rules, exe, usable))
        {
            return won(&r.profile, ProfileSource::App, Some(&r.id));
        }
        if let Some(l) = self.live(now).filter(|l| usable(&l.profile)) {
            return won(&l.profile, ProfileSource::Agent, None);
        }
        if let Some(s) = pick_schedule(&self.schedules, self.clock.now(), usable) {
            return won(&s.profile, ProfileSource::Schedule, Some(&s.id));
        }
        won(&self.chosen, ProfileSource::Fallback, None)
    }

    /// The engine now runs `s`: only this is what `/status` may report as the profile's source.
    pub fn applied(&mut self, s: &Selection) {
        self.last = Some(s.clone());
    }

    /// The profile that wins now. A different winner starts a new profile generation.
    pub fn select(&mut self, now: Duration, usable: Usable) -> Selection {
        let s = self.pick(now, usable);
        if self.winner.as_deref() != Some(&s.effective) {
            self.winner = Some(s.effective.clone());
            self.gens.profile += 1;
        }
        s
    }
}

/// The OS wall clock. Windows reads `GetLocalTime`, so daylight saving is the OS's job.
pub struct SystemClock;

impl LocalClock for SystemClock {
    fn now(&self) -> LocalTime {
        let (weekday, minute_of_day) = local_time();
        LocalTime {
            weekday: Weekday::parse(weekday).unwrap_or(Weekday::Mon),
            minute_of_day,
        }
    }
}

#[cfg(windows)]
fn local_time() -> (&'static str, u16) {
    crate::win::local_time()
}

/// Other CI targets use UTC. The shipped v2 service is Windows-only and uses local time.
#[cfg(not(windows))]
fn local_time() -> (&'static str, u16) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let weekday =
        ["thu", "fri", "sat", "sun", "mon", "tue", "wed"][(seconds / 86_400 % 7) as usize];
    (weekday, ((seconds % 86_400) / 60) as u16)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeClock(Mutex<LocalTime>);
    impl LocalClock for FakeClock {
        fn now(&self) -> LocalTime {
            *self.0.lock().unwrap()
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn cfg(extra: &str) -> DaemonConfig {
        DaemonConfig::from_toml(&format!("default_profile='balanced'\n{extra}")).unwrap()
    }

    fn arb(extra: &str) -> (Arbiter, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock(Mutex::new(LocalTime {
            weekday: Weekday::Mon,
            minute_of_day: 12 * 60,
        })));
        let mut a = Arbiter::new("balanced", clock.clone());
        a.replace_config(&cfg(extra));
        (a, clock)
    }

    const RULES: &str = "[[app_rule]]\nid='cs2'\nexe='cs2.exe'\nprofile='gaming'\n\
        [[schedule]]\nid='night'\ntime='00:00-24:00'\nprofile='silent'\n";
    const ALL: fn(&str) -> bool = |_| true;

    fn sel(a: &mut Arbiter, now: u64) -> (String, ProfileSource, Option<String>) {
        let s = a.select(secs(now), &ALL);
        (s.effective, s.source, s.rule_id)
    }

    fn offer(a: &mut Arbiter, now: u64, profile: &str) -> Offer {
        let seen = a.generations();
        a.offer_intent(secs(now), profile, seen, &ALL)
    }

    #[test]
    fn an_active_alert_beats_an_app_rule_and_letting_go_brings_it_back() {
        let (mut a, _) = arb(RULES);
        a.set_foreground(Some("cs2.exe"));
        assert_eq!(sel(&mut a, 0).1, ProfileSource::App);
        a.set_alert(Some(("hot".into(), "performance".into())));
        let (p, s, r) = sel(&mut a, 0);
        assert_eq!(
            (p.as_str(), s, r.as_deref()),
            ("performance", ProfileSource::Alert, Some("hot"))
        );
        assert_eq!(offer(&mut a, 0, "silent"), Offer::Activated);
        assert_eq!(
            sel(&mut a, 0).1,
            ProfileSource::Alert,
            "above the Agent too"
        );
        a.set_alert(None);
        assert_eq!(sel(&mut a, 0).1, ProfileSource::App);
    }

    #[test]
    fn an_alert_whose_profile_cannot_be_used_falls_through_to_the_next_source() {
        let (mut a, _) = arb(RULES);
        a.set_alert(Some(("hot".into(), "missing".into())));
        let s = a.select(secs(0), &|p: &str| p != "missing");
        assert_eq!(
            (s.effective.as_str(), s.source),
            ("silent", ProfileSource::Schedule)
        );
    }

    #[test]
    fn alerts_have_no_hourly_budget() {
        let (mut a, _) = arb("");
        for i in 0..20 {
            a.set_alert(Some(("hot".into(), "performance".into())));
            assert_eq!(sel(&mut a, i).1, ProfileSource::Alert, "round {i}");
            a.set_alert(None);
            assert_eq!(sel(&mut a, i).1, ProfileSource::Fallback);
        }
    }

    #[test]
    fn precedence_is_app_then_agent_then_schedule_then_the_chosen_fallback() {
        let (mut a, _) = arb(RULES);
        let (p, s, r) = sel(&mut a, 0);
        assert_eq!(
            (p.as_str(), s, r.as_deref()),
            ("silent", ProfileSource::Schedule, Some("night"))
        );
        assert_eq!(offer(&mut a, 0, "performance"), Offer::Activated);
        assert_eq!(sel(&mut a, 0).1, ProfileSource::Agent);
        a.set_foreground(Some("CS2.EXE"));
        let (p, s, r) = sel(&mut a, 0);
        assert_eq!(
            (p.as_str(), s, r.as_deref()),
            ("gaming", ProfileSource::App, Some("cs2"))
        );
        a.set_foreground(Some("notcs2.exe"));
        assert_eq!(offer(&mut a, 0, "performance"), Offer::Activated);
        assert_eq!(
            sel(&mut a, 0).0,
            "performance",
            "substring is no match; the agent wins"
        );
        a.invalidate_agent();
        assert_eq!(sel(&mut a, 0).0, "silent");
        let (mut b, _) = arb("");
        assert_eq!(
            sel(&mut b, 0),
            ("balanced".into(), ProfileSource::Fallback, None)
        );
    }

    #[test]
    fn a_matching_app_rule_beats_a_live_agent_intent_offered_under_the_new_generation() {
        let (mut a, _) = arb(RULES);
        a.set_foreground(Some("cs2.exe")); // bumps the generation first, as in real use
        let seen = a.generations();
        assert_eq!(
            a.offer_intent(secs(0), "performance", seen, &ALL),
            Offer::Activated
        );
        let s = a.select(secs(0), &ALL);
        assert_eq!(
            (s.effective.as_str(), s.source, s.rule_id.as_deref()),
            ("gaming", ProfileSource::App, Some("cs2")),
            "the intent is live, and the app rule still wins"
        );
    }

    #[test]
    fn a_pending_pick_is_not_reported_until_the_engine_applied_it() {
        let (mut a, _) = arb(RULES);
        a.set_foreground(Some("cs2.exe"));
        let app = a.select(secs(0), &ALL);
        assert_eq!(a.last(), None, "nothing is reported before it is applied");
        a.applied(&app);
        a.set_foreground(Some("notepad.exe"));
        assert_eq!(offer(&mut a, 1, "performance"), Offer::Activated);
        let pending = a.select(secs(1), &ALL);
        assert_eq!(pending.source, ProfileSource::Agent);
        assert_eq!(a.last(), Some(&app), "the agent's pick is only pending");
        a.applied(&pending);
        assert_eq!(a.last(), Some(&pending));
    }

    #[test]
    fn a_live_agent_intent_beats_a_matching_schedule() {
        let (mut a, _) = arb(RULES); // `night` covers the whole day
        assert_eq!(sel(&mut a, 0).1, ProfileSource::Schedule, "control");
        assert_eq!(offer(&mut a, 0, "performance"), Offer::Activated);
        let (p, s, r) = sel(&mut a, 1);
        assert_eq!(
            (p.as_str(), s, r),
            ("performance", ProfileSource::Agent, None)
        );
        // the lease lapses: the schedule is back
        assert_eq!(sel(&mut a, 10).1, ProfileSource::Schedule);
    }

    #[test]
    fn manual_choice_never_preempts_a_live_source_and_is_reported_separately() {
        let (mut a, _) = arb(RULES);
        a.set_foreground(Some("cs2.exe"));
        a.choose("manual");
        let s = a.select(secs(0), &ALL);
        assert_eq!(
            (s.chosen.as_str(), s.effective.as_str()),
            ("manual", "gaming")
        );
        a.set_foreground(None);
        let (mut b, _) = arb("");
        b.choose("manual");
        assert_eq!(
            sel(&mut b, 0),
            ("manual".into(), ProfileSource::Fallback, None)
        );
    }

    #[test]
    fn an_unusable_target_is_skipped_for_the_next_candidate_then_the_next_source() {
        let (mut a, _) = arb(&format!(
            "{RULES}[[app_rule]]\nid='cs2b'\nexe='cs2.exe'\nprofile='performance'\n"
        ));
        a.set_foreground(Some("cs2.exe"));
        let s = a.select(secs(0), &|p: &str| p != "gaming");
        assert_eq!(
            (s.effective.as_str(), s.rule_id.as_deref()),
            ("performance", Some("cs2b"))
        );
        let s = a.select(secs(0), &|p: &str| p == "silent");
        assert_eq!(
            (s.effective.as_str(), s.source),
            ("silent", ProfileSource::Schedule)
        );
    }

    #[test]
    fn an_intent_is_a_three_second_lease_that_a_renewal_extends() {
        let (mut a, _) = arb("");
        assert_eq!(offer(&mut a, 0, "gaming"), Offer::Activated);
        assert_eq!(offer(&mut a, 2, "gaming"), Offer::Renewed);
        assert_eq!(sel(&mut a, 4).1, ProfileSource::Agent, "extended to 5 s");
        assert_eq!(
            sel(&mut a, 5).1,
            ProfileSource::Fallback,
            "expired at exactly 3 s after the renewal"
        );
    }

    #[test]
    fn a_foreground_or_config_change_kills_the_intent_and_rejects_late_decisions() {
        let (mut a, _) = arb("");
        let started = a.generations();
        a.set_foreground(Some("a.exe"));
        assert_eq!(
            a.offer_intent(secs(0), "gaming", started, &ALL),
            Offer::Stale
        );
        assert_eq!(offer(&mut a, 0, "gaming"), Offer::Activated);
        a.set_foreground(Some("b.exe"));
        assert_eq!(
            sel(&mut a, 0).1,
            ProfileSource::Fallback,
            "a.exe's intent is not b.exe's"
        );
        let started = a.generations();
        a.replace_config(&cfg(RULES));
        assert_eq!(
            a.offer_intent(secs(0), "gaming", started, &ALL),
            Offer::Stale
        );
        assert_eq!(a.generations().config, started.config + 1);
        a.replace_config(&cfg(RULES));
        assert_eq!(
            a.generations().config,
            started.config + 1,
            "same rules: no new generation"
        );
    }

    #[test]
    fn changing_only_the_fallback_does_not_bump_the_config_generation() {
        let (mut a, _) = arb(RULES);
        let before = a.generations();
        a.replace_config(&DaemonConfig {
            default_profile: "other".into(),
            ..cfg(RULES)
        });
        assert_eq!(a.generations().config, before.config);
        assert_eq!(a.chosen(), "other");
    }

    #[test]
    fn the_profile_generation_moves_only_when_the_winner_changes() {
        let (mut a, _) = arb("");
        let g = a.generations().profile;
        sel(&mut a, 0);
        let first = a.generations().profile;
        sel(&mut a, 1);
        assert_eq!(a.generations().profile, first, "same winner");
        offer(&mut a, 1, "gaming");
        sel(&mut a, 1);
        assert_eq!(a.generations().profile, first + 1);
        assert!(first >= g);
    }

    #[test]
    fn six_agent_changes_per_hour_are_counted_at_apply_and_the_seventh_is_rejected() {
        let (mut a, _) = arb("");
        for n in 0..6 {
            let t = n * 10;
            assert_eq!(offer(&mut a, t, "gaming"), Offer::Activated, "{n}");
            assert_eq!(sel(&mut a, t).1, ProfileSource::Agent);
            a.agent_applied(secs(t));
            a.invalidate_agent(); // pause
        }
        assert_eq!(offer(&mut a, 60, "gaming"), Offer::LimitReached);
        assert_eq!(sel(&mut a, 60).1, ProfileSource::Fallback);
        assert_eq!(
            offer(&mut a, 3600 + 1, "gaming"),
            Offer::Activated,
            "the hour moved on"
        );
    }

    #[test]
    fn a_renewal_and_an_unapplied_registration_use_no_change() {
        let (mut a, _) = arb("");
        assert_eq!(offer(&mut a, 0, "gaming"), Offer::Activated);
        for t in 1..30 {
            assert_eq!(offer(&mut a, t, "gaming"), Offer::Renewed);
        }
        a.agent_applied(secs(1));
        a.agent_applied(secs(2)); // counted once
        a.invalidate_agent();
        for n in 0..5 {
            assert_eq!(offer(&mut a, 40 + n, "gaming"), Offer::Activated);
            a.agent_applied(secs(40 + n));
            a.invalidate_agent();
        }
        assert_eq!(
            offer(&mut a, 50, "gaming"),
            Offer::LimitReached,
            "1 + 5 counted, no more"
        );
    }

    #[test]
    fn registering_an_already_effective_profile_is_checked_against_the_limit_but_not_counted() {
        let (mut a, _) = arb("");
        a.choose("gaming");
        for n in 0..6 {
            // gaming is already effective as the fallback, so none of these change anything
            assert_eq!(offer(&mut a, n, "gaming"), Offer::Activated);
            a.agent_applied(secs(n));
            a.invalidate_agent();
        }
        assert_eq!(
            offer(&mut a, 7, "gaming"),
            Offer::Activated,
            "nothing was counted"
        );
        a.invalidate_agent();
        a.choose("balanced");
        for n in 0..6 {
            assert_eq!(offer(&mut a, 10 + n, "gaming"), Offer::Activated);
            a.agent_applied(secs(10 + n));
            a.invalidate_agent();
        }
        a.choose("gaming");
        assert_eq!(
            offer(&mut a, 20, "gaming"),
            Offer::LimitReached,
            "an already-effective intent is still checked"
        );
    }

    #[test]
    fn schedules_follow_the_injected_clock_each_time_and_app_rules_are_independent_of_the_agent() {
        let (mut a, clock) =
            arb("[[schedule]]\nid='night'\ndays=['fri']\ntime='22:00-08:00'\nprofile='silent'\n");
        assert_eq!(sel(&mut a, 0).1, ProfileSource::Fallback);
        *clock.0.lock().unwrap() = LocalTime {
            weekday: Weekday::Sat,
            minute_of_day: 7 * 60,
        };
        assert_eq!(
            sel(&mut a, 1),
            (
                "silent".into(),
                ProfileSource::Schedule,
                Some("night".into())
            )
        );
        a.invalidate_agent(); // pause / crash / supervisor loss
        assert_eq!(sel(&mut a, 2).1, ProfileSource::Schedule);
        *clock.0.lock().unwrap() = LocalTime {
            weekday: Weekday::Sat,
            minute_of_day: 8 * 60,
        };
        assert_eq!(sel(&mut a, 3).1, ProfileSource::Fallback);
    }
}
