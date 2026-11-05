//! Live optimiser and short boosts (SPEC §7.2 fast layer). Pure logic: it decides what to
//! ask for; the daemon submits it to the engine (S1), which enforces the 15-point floor again.
//! Every request expires in 3 s and is renewed each tick, so a stopped agent costs nothing.
//!
//! Lower only when every temperature the fan cools is known, under target, and the model,
//! whose duty effect must be clearly negative, says it stays under target at the lowered
//! duty. Anything unknown means no lowering. Raising is free.
//! shortcut: the boost is a fixed +20 points, and `target` comes from the caller; derive both
//! from the slider with the daemon wiring.

use crate::inputs::Sample;
use crate::model::ThermalModel;
use std::time::Duration;
use thermaliq_common::AGENT_MAX_LOWER_PCT;

/// Prediction horizon in ticks (1 s each), inside SPEC's 10–30 s.
pub const AHEAD: usize = 20;
pub const REQUEST_TTL: Duration = Duration::from_secs(3);
/// A boost lasts at most this long, and re-arms only after this long without a need.
pub const BOOST_MAX: Duration = Duration::from_secs(60);
const BOOST_POINTS: u8 = 20;

/// One temperature a fan cools. `sample`/`model` are `None` when unknown.
pub struct Cooled<'a> {
    pub sample: Option<Sample>,
    pub target: f64,
    pub model: Option<&'a ThermalModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub duty: u8,
    pub expires_at: Duration,
}

/// Per-fan state: when the current boost window started and when a boost was last needed.
#[derive(Debug, Default)]
pub struct Optimiser {
    boost_since: Option<Duration>,
    last_need: Duration,
}

impl Cooled<'_> {
    /// Over target now, or predicted over it `AHEAD` ticks out.
    fn needs_more(&self) -> bool {
        let Some(s) = self.sample else { return false };
        s.temp > self.target
            || self
                .model
                .and_then(|m| m.predict(&s, AHEAD))
                .is_some_and(|p| p > self.target)
    }

    /// Known, under target, and still under target `AHEAD` ticks after the fan is set to the
    /// duty that will be requested, `curve − by` (not the measured duty, which lags a ramp).
    fn can_lower(&self, curve: u8, by: u8) -> bool {
        let (Some(s), Some(m)) = (self.sample, self.model) else {
            return false;
        };
        let lowered = Sample {
            duty: f64::from(curve.saturating_sub(by)),
            ..s
        };
        s.temp < self.target
            && m.lowering_is_safe(&s, AHEAD)
            && m.predict(&s, AHEAD).is_some_and(|p| p < self.target)
            && m.predict(&lowered, AHEAD).is_some_and(|p| p < self.target)
    }
}

impl Optimiser {
    /// `curve` is the engine's `curve_output` for this fan; `slider` 0 = quiet, 100 = cool.
    /// `None` means no request: the profile runs.
    pub fn decide(
        &mut self,
        now: Duration,
        curve: u8,
        slider: u8,
        pump: bool,
        cooled: &[Cooled],
    ) -> Option<Request> {
        if pump || cooled.is_empty() {
            return None;
        }
        let expires_at = now + REQUEST_TTL;
        if now.saturating_sub(self.last_need) >= BOOST_MAX {
            self.boost_since = None;
        }
        if cooled.iter().any(Cooled::needs_more) {
            self.last_need = now;
            let since = *self.boost_since.get_or_insert(now);
            return (now.saturating_sub(since) < BOOST_MAX).then(|| Request {
                duty: curve.saturating_add(BOOST_POINTS).min(100),
                expires_at,
            });
        }
        // Just boosted: let the part settle before turning anything down.
        if self.boost_since.is_some() {
            return None;
        }
        let quiet = u32::from(100 - slider.min(100));
        let by = (u32::from(AGENT_MAX_LOWER_PCT) * quiet + 50) / 100;
        let by = u8::try_from(by).ok().filter(|&b| b > 0)?;
        cooled
            .iter()
            .all(|c| c.can_lower(curve, by))
            .then(|| Request {
                duty: curve.saturating_sub(by),
                expires_at,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Duration = Duration::from_secs(1000);

    /// dT over AHEAD = bias − 0.05·duty.
    fn model(bias: f64) -> ThermalModel {
        ThermalModel::with_weights(AHEAD, [bias, 0.0, -0.05, 0.0, 0.0])
    }

    fn sample(temp: f64) -> Option<Sample> {
        Some(Sample {
            temp,
            duty: 40.0,
            load: 50.0,
            power: 100.0,
        })
    }

    fn cooled(temp: f64, target: f64, m: Option<&ThermalModel>) -> Cooled<'_> {
        Cooled {
            sample: sample(temp),
            target,
            model: m,
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn lowers_by_the_quiet_share_of_15_points_and_expires_in_3_s() {
        let m = model(2.0);
        let c = [cooled(70.0, 75.0, Some(&m))];
        let r = |slider| Optimiser::default().decide(T0, 50, slider, false, &c);
        assert_eq!(
            r(0),
            Some(Request {
                duty: 35,
                expires_at: T0 + secs(3)
            })
        );
        assert_eq!(r(50).map(|r| r.duty), Some(42), "7.5 rounds to 8 → 42");
        assert_eq!(r(100), None, "full cool never lowers");
    }

    #[test]
    fn never_lowers_without_clear_evidence() {
        let m = model(2.0);
        let mut o = Optimiser::default();
        let mut go = |c: &[Cooled]| o.decide(T0, 50, 0, false, c);
        assert!(
            go(&[cooled(70.0, 75.0, Some(&m))]).is_some(),
            "control case"
        );
        assert!(go(&[]).is_none(), "nothing cooled");
        assert!(go(&[cooled(70.0, 75.0, None)]).is_none(), "no model");
        let unknown = Cooled {
            sample: None,
            ..cooled(70.0, 75.0, Some(&m))
        };
        assert!(go(&[unknown]).is_none(), "unknown reading");
        assert!(go(&[cooled(75.0, 75.0, Some(&m))]).is_none(), "at target");
        let flat = ThermalModel::with_weights(AHEAD, [2.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(
            go(&[cooled(70.0, 75.0, Some(&flat))]).is_none(),
            "no duty effect"
        );
        let wrong_horizon = ThermalModel::with_weights(10, [2.0, 0.0, -0.05, 0.0, 0.0]);
        assert!(go(&[cooled(70.0, 75.0, Some(&wrong_horizon))]).is_none());
    }

    #[test]
    fn never_lowers_if_the_lowered_duty_would_go_over_target() {
        // now (duty 40): 74.9 + 2 − 2 = 74.9; at curve 50 − 15 = 35: 75.15 ≥ 75
        let m = model(2.0);
        let c = [cooled(74.9, 75.0, Some(&m))];
        assert_eq!(Optimiser::default().decide(T0, 50, 0, false, &c), None);
    }

    #[test]
    fn the_safety_prediction_is_made_at_the_duty_that_will_be_requested() {
        // measured duty 60 lags a curve of 50: the request is 35, not 60 − 15 = 45.
        // At duty 45 the model says 69.75 (< 70); at 35 it says 70.25 (≥ 70).
        let m = model(2.0);
        let mut c = cooled(70.0, 70.0, Some(&m));
        c.target = 70.5; // temp 70 < 70.5; 35 gives 70.25 < 70.5: allowed
        c.sample = c.sample.map(|s| Sample { duty: 60.0, ..s });
        assert_eq!(
            Optimiser::default()
                .decide(T0, 50, 0, false, &[c])
                .map(|r| r.duty),
            Some(35)
        );
        let mut c = cooled(70.0, 70.2, Some(&m));
        c.sample = c.sample.map(|s| Sample { duty: 60.0, ..s });
        assert_eq!(
            Optimiser::default().decide(T0, 50, 0, false, &[c]),
            None,
            "35 → 70.25 ≥ 70.2 must refuse, though 45 → 69.75 would pass"
        );
    }

    #[test]
    fn every_cooled_temperature_must_allow_it() {
        let m = model(2.0);
        let c = [cooled(70.0, 75.0, Some(&m)), cooled(76.0, 75.0, Some(&m))];
        let r = Optimiser::default().decide(T0, 50, 0, false, &c);
        assert_eq!(
            r.map(|r| r.duty),
            Some(70),
            "the hot one forces a boost, not a lowering"
        );
    }

    #[test]
    fn boosts_when_over_or_predicted_over_target() {
        let m = model(3.0); // 74.5 + 3 − 2 = 75.5 predicted
        let predicted = [cooled(74.5, 75.0, Some(&m))];
        assert_eq!(
            Optimiser::default()
                .decide(T0, 50, 0, false, &predicted)
                .map(|r| r.duty),
            Some(70)
        );
        let over = [cooled(80.0, 75.0, None)];
        assert_eq!(
            Optimiser::default()
                .decide(T0, 90, 0, false, &over)
                .map(|r| r.duty),
            Some(100),
            "capped at 100"
        );
    }

    #[test]
    fn a_boost_ends_after_60_s_and_re_arms_after_60_s_without_need() {
        let over = [cooled(80.0, 75.0, None)];
        let ok = [cooled(60.0, 75.0, None)];
        let mut o = Optimiser::default();
        assert!(o.decide(T0, 50, 0, false, &over).is_some());
        assert!(o.decide(T0 + secs(59), 50, 0, false, &over).is_some());
        assert!(
            o.decide(T0 + secs(60), 50, 0, false, &over).is_none(),
            "spent"
        );
        assert!(
            o.decide(T0 + secs(100), 50, 0, false, &over).is_none(),
            "still spent"
        );
        assert!(o.decide(T0 + secs(101), 50, 0, false, &ok).is_none());
        assert!(
            o.decide(T0 + secs(161), 50, 0, false, &over).is_some(),
            "re-armed"
        );
    }

    #[test]
    fn no_lowering_straight_after_a_boost() {
        let m = model(2.0);
        let over = [cooled(80.0, 75.0, None)];
        let ok = [cooled(70.0, 75.0, Some(&m))];
        let mut o = Optimiser::default();
        o.decide(T0, 50, 0, false, &over);
        assert!(o.decide(T0 + secs(1), 50, 0, false, &ok).is_none());
        assert!(o.decide(T0 + secs(61), 50, 0, false, &ok).is_some());
    }

    #[test]
    fn never_touches_a_pump() {
        let over = [cooled(80.0, 75.0, None)];
        assert_eq!(Optimiser::default().decide(T0, 50, 0, true, &over), None);
    }

    #[test]
    fn a_lowering_never_goes_past_15_points_or_over_target() {
        for bias in [0.0, 1.0, 2.0, 3.0, 5.0] {
            let m = model(bias);
            for temp in (40..90).map(f64::from) {
                for slider in [0, 25, 50, 75, 100] {
                    for curve in [0, 15, 50, 100] {
                        let c = [cooled(temp, 75.0, Some(&m))];
                        let Some(r) = Optimiser::default().decide(T0, curve, slider, false, &c)
                        else {
                            continue;
                        };
                        if r.duty < curve {
                            assert!(
                                temp < 75.0 && curve - r.duty <= 15,
                                "{bias} {temp} {slider} {curve}"
                            );
                        }
                    }
                }
            }
        }
    }
}
