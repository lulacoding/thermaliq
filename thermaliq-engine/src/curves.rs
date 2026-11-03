use crate::{valid_temp, Temps};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thermaliq_common::{Curve, GraphPoint, MixFunction, Profile};

/// Mix/sync chains are acyclic (checked by `Profile` validation); this only
/// stops a hand-built cyclic profile from overflowing the stack.
const MAX_DEPTH: u8 = 16;

#[derive(Default)]
struct TriggerState {
    loaded: bool,
    /// When the opposite state was first wanted; `None` if not currently wanted.
    wanting_since: Option<Duration>,
}

/// Evaluates a profile's curves to a duty. `None` means an input is missing or
/// invalid: the caller (S3) must not guess a duty.
pub struct CurveEngine {
    curves: HashMap<String, Arc<Curve>>,
    triggers: HashMap<String, TriggerState>,
}

impl CurveEngine {
    pub fn new(profile: &Profile) -> Self {
        Self {
            curves: profile
                .curves
                .iter()
                .map(|c| (c.id().to_string(), Arc::new(c.clone())))
                .collect(),
            triggers: HashMap::new(),
        }
    }

    /// `now` is monotonic time since the engine started; calling twice with the
    /// same `now` gives the same answer.
    pub fn duty(&mut self, curve: &str, temps: &Temps, now: Duration) -> Option<u8> {
        self.eval(curve, temps, now, 0).map(|d| d.min(100))
    }

    fn eval(&mut self, id: &str, temps: &Temps, now: Duration, depth: u8) -> Option<u8> {
        if depth > MAX_DEPTH {
            return None;
        }
        let curve = self.curves.get(id)?.clone();
        let source = |s: &String| temps.get(s).copied().filter(|t| valid_temp(*t));
        match &*curve {
            Curve::Flat { pwm, .. } => Some(*pwm),
            Curve::Linear {
                source: s,
                min_temp,
                max_temp,
                min_pwm,
                max_pwm,
                ..
            } => {
                let t = source(s)?;
                Some(if t <= *min_temp {
                    *min_pwm
                } else if t >= *max_temp {
                    *max_pwm
                } else {
                    let f = (t - min_temp) / (max_temp - min_temp);
                    lerp(*min_pwm, *max_pwm, f)
                })
            }
            Curve::Graph {
                source: s, points, ..
            } => graph(points, source(s)?),
            Curve::Mix {
                function, curves, ..
            } => {
                let mut duties = Vec::with_capacity(curves.len());
                for c in curves {
                    duties.push(u32::from(self.eval(c, temps, now, depth + 1)?));
                }
                mix(*function, &duties)
            }
            Curve::Sync { target, offset, .. } => {
                let d = i16::from(self.eval(target, temps, now, depth + 1)?);
                Some((d + i16::from(*offset)).clamp(0, 100) as u8)
            }
            Curve::Trigger {
                source: s,
                idle_temp,
                idle_pwm,
                load_temp,
                load_pwm,
                response_ms,
                ..
            } => {
                let t = source(s)?;
                let st = self.triggers.entry(id.to_string()).or_default();
                // Between idle_temp and load_temp nothing is wanted: hold the state.
                let want = if t >= *load_temp {
                    Some(true)
                } else if t <= *idle_temp {
                    Some(false)
                } else {
                    None
                };
                match want {
                    Some(w) if w != st.loaded => {
                        let since = *st.wanting_since.get_or_insert(now);
                        if now.saturating_sub(since) >= Duration::from_millis(*response_ms) {
                            st.loaded = w;
                            st.wanting_since = None;
                        }
                    }
                    _ => st.wanting_since = None,
                }
                Some(if st.loaded { *load_pwm } else { *idle_pwm })
            }
        }
    }
}

fn lerp(a: u8, b: u8, f: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * f).round() as u8
}

fn graph(points: &[GraphPoint], t: f32) -> Option<u8> {
    let (first, last) = (points.first()?, points.last()?);
    if t <= first.temp {
        return Some(first.pwm);
    }
    if t >= last.temp {
        return Some(last.pwm);
    }
    let w = points.windows(2).find(|w| t <= w[1].temp)?;
    let span = w[1].temp - w[0].temp;
    if span <= 0.0 {
        return Some(w[1].pwm); // unsorted/duplicate points; validation rejects these
    }
    Some(lerp(w[0].pwm, w[1].pwm, (t - w[0].temp) / span))
}

fn mix(f: MixFunction, d: &[u32]) -> Option<u8> {
    let n = u32::try_from(d.len()).ok().filter(|n| *n > 0)?;
    let v = match f {
        MixFunction::Max => *d.iter().max()?,
        MixFunction::Min => *d.iter().min()?,
        MixFunction::Average => (d.iter().sum::<u32>() + n / 2) / n,
        MixFunction::Sum => d.iter().sum(),
    };
    Some(v.min(100) as u8)
}
