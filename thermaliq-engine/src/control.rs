use std::time::Duration;
use thermaliq_common::Control;

/// Who is asking for a duty, highest priority first (SPEC §4). Safety
/// (S2–S4) sits above all of these and is M5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Override,
    Agent,
    AppRule,
    Schedule,
    Profile,
}

impl Source {
    /// SPEC §4: manual overrides and agent interventions are always time-limited.
    fn needs_expiry(self) -> bool {
        matches!(self, Source::Override | Source::Agent)
    }
}

pub(crate) struct Request {
    source: Source,
    duty: u8,
    expires_at: Option<Duration>,
}

/// Resolves one fan's requests to a duty each tick: priority, min_pwm floor,
/// 0-RPM hysteresis, then ramping. Pure logic; the engine loop writes the result.
pub struct FanControl {
    step_up: u8,
    step_down: u8,
    min_pwm: u8,
    /// `(start_percent, stop_percent)`; validated as both-or-neither.
    zero_rpm: Option<(u8, u8)>,
    requests: Vec<Request>,
    /// Starts true: a fan we know nothing about is assumed to be spinning.
    running: bool,
    last: Option<u8>,
}

impl FanControl {
    pub fn new(c: &Control) -> Self {
        Self {
            step_up: c.step_up_pct,
            step_down: c.step_down_pct,
            min_pwm: c.min_pwm,
            zero_rpm: c.start_percent.zip(c.stop_percent),
            requests: Vec::new(),
            running: true,
            last: None,
        }
    }

    /// A source's new request replaces its previous one. `Override` and `Agent`
    /// without an expiry are rejected (S8: no silent success).
    pub fn submit(
        &mut self,
        source: Source,
        duty: u8,
        expires_at: Option<Duration>,
    ) -> Result<(), String> {
        if source.needs_expiry() && expires_at.is_none() {
            return Err(format!("{source:?} requests must expire"));
        }
        self.requests.retain(|r| r.source != source);
        self.requests.push(Request {
            source,
            duty,
            expires_at,
        });
        Ok(())
    }

    /// Everything except the profile's own request, for carrying across a profile switch.
    pub(crate) fn take_requests(&mut self) -> Vec<Request> {
        let (keep, take) = std::mem::take(&mut self.requests)
            .into_iter()
            .partition(|r| r.source == Source::Profile);
        self.requests = keep;
        take
    }

    pub(crate) fn put_requests(&mut self, requests: Vec<Request>) {
        self.requests.extend(requests);
    }

    /// Drops a source's request, e.g. an app closed or its curve has no valid input.
    /// Returns whether there was one.
    pub fn withdraw(&mut self, source: Source) -> bool {
        let before = self.requests.len();
        self.requests.retain(|r| r.source != source);
        self.requests.len() != before
    }

    /// The duty a source is asking for right now, after any clamping (as of the last tick).
    pub(crate) fn request(&self, source: Source) -> Option<u8> {
        self.requests
            .iter()
            .find(|r| r.source == source)
            .map(|r| r.duty.min(100))
    }

    /// Highest duty asked for by any source below Agent (AppRule, Schedule, Profile).
    pub(crate) fn lower_priority_baseline(&self) -> Option<u8> {
        self.requests
            .iter()
            .filter(|r| r.source > Source::Agent)
            .map(|r| r.duty.min(100))
            .max()
    }

    /// Raises an Agent request up to `floor` (SPEC §7.2: never far below the curve).
    pub(crate) fn floor_agent(&mut self, floor: u8) {
        for r in self
            .requests
            .iter_mut()
            .filter(|r| r.source == Source::Agent)
        {
            r.duty = r.duty.max(floor);
        }
    }

    /// Start ramping from the fan's real state (first snapshot) instead of jumping.
    pub fn seed(&mut self, duty: u8, spinning: bool) {
        self.last = Some(duty.min(100));
        self.running = spinning;
    }

    /// The duty to write this tick, or `None` when nothing is requesting one
    /// (the caller decides; M5 turns missing input into S3).
    pub fn tick(&mut self, now: Duration) -> Option<u8> {
        self.requests
            .retain(|r| r.expires_at.is_none_or(|e| now < e));
        let want = self.requests.iter().min_by_key(|r| r.source)?.duty.min(100);
        let mut last = self.last;
        if let Some((start, stop)) = self.zero_rpm {
            // v0 semantics: stop at or below stop, restart strictly above start.
            if self.running && want <= stop {
                self.running = false;
            } else if !self.running && want > start {
                self.running = true;
                // Don't ramp through duties below the fan's own start threshold.
                last = last.map(|l| l.max(start));
            }
        }
        // `running` only gates fans with 0-RPM configured; seed() may set it false for any fan.
        let target = if self.running || self.zero_rpm.is_none() {
            want.max(self.min_pwm).min(100)
        } else {
            0
        };
        let out = match last {
            None => target,
            Some(l) if target > l => l.saturating_add(self.step_up).min(target),
            Some(l) => l.saturating_sub(self.step_down).max(target),
        };
        self.last = Some(out);
        Some(out)
    }
}
