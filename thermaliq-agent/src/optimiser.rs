//! Live optimiser and short boosts (SPEC §7.2 fast layer). Pure logic: it decides what to
//! ask for; the daemon submits it to the engine (S1), which enforces the 15-point floor again.
//! Every request expires in 3 s and is renewed each tick, so a stopped agent costs nothing.
//!
//! Three behaviours, from most to least urgent:
//! - **Raise** (a temperature is over target *now*): ask for the curve + 2 points more each
//!   tick, up to 100, with no time cap. Hold that while the part is within 2 °C of target;
//!   once every temperature has been 2 °C under target for 10 s, ease back 1 point per tick
//!   to the curve. An unknown reading holds.
//! - **Boost** (only *predicted* over target): curve + 20, for at most 60 s, re-armed after
//!   60 s without a predicted need. Over-target time does not use the window up.
//! - **Lower** (everything known, under target, model says it stays there): up to 15 points.
//!   Never while a raise is held or easing, nor right after a boost.
//!
//! shortcut: the boost is a fixed +20 and the raise step is fixed at 2; revisit with real-game data.

use crate::inputs::{Component, Sample};
use crate::model::ThermalModel;
use std::time::Duration;
use thermaliq_common::AGENT_MAX_LOWER_PCT;

/// Prediction horizon in ticks (1 s each), inside SPEC's 10–30 s.
pub const AHEAD: usize = 20;
pub const REQUEST_TTL: Duration = Duration::from_secs(3);
/// A boost lasts at most this long, and re-arms only after this long without a need.
pub const BOOST_MAX: Duration = Duration::from_secs(60);
const BOOST_POINTS: u8 = 20;
/// Points added per tick while a temperature is over target.
const RAISE_STEP: u8 = 2;
/// A raise is eased back only once every temperature has been this far under target...
const EASE_BAND: f64 = 2.0;
/// ...for this long; then it comes back 1 point per tick.
const EASE_AFTER: Duration = Duration::from_secs(10);

/// The temperature to hold, from the quiet ↔ cool slider (0–100): a straight line between
/// the quiet target (GPU 80, CPU 85 °C) and the cool target (GPU 65, CPU 70 °C). SPEC §7.2.
pub fn target(component: Component, slider: u8) -> f64 {
    let (quiet, cool) = match component {
        Component::Gpu => (80.0, 65.0),
        Component::Cpu => (85.0, 70.0),
    };
    quiet + (cool - quiet) * f64::from(slider.min(100)) / 100.0
}

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

/// Per-fan state.
#[derive(Debug, Default)]
pub struct Optimiser {
    /// Points above the curve that a temperature over target has earned and not yet given back.
    extra: u8,
    /// Since when every temperature has been `EASE_BAND` under target.
    under_since: Option<Duration>,
    /// When the current boost window started, and when a boost was last needed.
    boost_since: Option<Duration>,
    last_need: Duration,
}

impl Cooled<'_> {
    /// Over target right now (a real reading, not a prediction).
    fn over(&self) -> bool {
        self.sample.is_some_and(|s| s.temp > self.target)
    }

    /// Known, and more than `band` under target.
    fn under(&self, band: f64) -> bool {
        self.sample.is_some_and(|s| s.temp < self.target - band)
    }

    /// Predicted over target `AHEAD` ticks out (the model refuses samples outside its range).
    fn predicted_over(&self) -> bool {
        let Some(s) = self.sample else { return false };
        self.model
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
            *self = Self::default();
            return None;
        }
        let expires_at = now + REQUEST_TTL;
        if now.saturating_sub(self.last_need) >= BOOST_MAX {
            self.boost_since = None;
        }
        let over = cooled.iter().any(Cooled::over);
        if over {
            self.extra = self
                .extra
                .saturating_add(RAISE_STEP)
                .min(100 - curve.min(100));
            self.under_since = None;
        } else if cooled.iter().all(|c| c.under(EASE_BAND)) {
            let since = *self.under_since.get_or_insert(now);
            if now.saturating_sub(since) >= EASE_AFTER {
                self.extra = self.extra.saturating_sub(1);
            }
        } else {
            self.under_since = None; // within 2 °C, or unknown: hold
        }
        let mut want = (self.extra > 0).then(|| curve.saturating_add(self.extra).min(100));
        let boost = Some(curve.saturating_add(BOOST_POINTS).min(100));
        if over {
            // Being over target does not start or extend a boost, but one already running
            // keeps its level (the request must not fall just as things get worse).
            if self
                .boost_since
                .is_some_and(|s| now.saturating_sub(s) < BOOST_MAX)
            {
                want = want.max(boost);
            }
        } else if cooled.iter().any(Cooled::predicted_over) {
            self.last_need = now;
            let since = *self.boost_since.get_or_insert(now);
            if now.saturating_sub(since) < BOOST_MAX {
                want = want.max(boost);
            }
        }
        if let Some(duty) = want {
            return Some(Request { duty, expires_at });
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
    fn targets_are_a_straight_line_from_quiet_to_cool() {
        let t = |c, s| target(c, s);
        assert_eq!((t(Component::Gpu, 0), t(Component::Gpu, 100)), (80.0, 65.0));
        assert_eq!((t(Component::Cpu, 0), t(Component::Cpu, 100)), (85.0, 70.0));
        assert_eq!((t(Component::Gpu, 50), t(Component::Cpu, 50)), (72.5, 77.5));
        assert_eq!(t(Component::Gpu, 200), 65.0, "slider is clamped to 100");
        assert_eq!(t(Component::Gpu, 20), 77.0);
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

    /// One `decide` per second from `T0 + from`, slider 0 (quiet), returning each duty asked.
    fn run(o: &mut Optimiser, from: u64, n: u64, curve: u8, c: &[Cooled]) -> Vec<Option<u8>> {
        (from..from + n)
            .map(|t| o.decide(T0 + secs(t), curve, 0, false, c).map(|r| r.duty))
            .collect()
    }

    #[test]
    fn every_cooled_temperature_must_allow_lowering() {
        let m = model(2.0);
        let c = [cooled(70.0, 75.0, Some(&m)), cooled(76.0, 75.0, Some(&m))];
        let r = Optimiser::default().decide(T0, 50, 0, false, &c);
        assert_eq!(
            r.map(|r| r.duty),
            Some(52),
            "the hot one raises; nothing is lowered"
        );
    }

    #[test]
    fn over_target_raises_2_points_a_tick_with_no_time_cap_up_to_100() {
        let over = [cooled(80.0, 75.0, None)];
        let mut o = Optimiser::default();
        let asks = run(&mut o, 0, 400, 50, &over);
        assert_eq!(asks[..3], [Some(52), Some(54), Some(56)]);
        assert_eq!(asks[24], Some(100));
        assert!(
            asks[24..].iter().all(|a| *a == Some(100)),
            "held at 100 for the rest of 400 s, far past 60 s"
        );
        let mut o = Optimiser::default();
        let asks = run(&mut o, 0, 5, 97, &over);
        assert_eq!(asks, [Some(99), Some(100), Some(100), Some(100), Some(100)]);
    }

    #[test]
    fn holds_until_2_under_target_for_10_s_then_eases_1_a_tick_to_the_curve() {
        let over = [cooled(80.0, 75.0, None)];
        let near = [cooled(74.0, 75.0, None)]; // under target, but not by 2
        let cool = [cooled(72.0, 75.0, None)]; // 3 under
        let mut o = Optimiser::default();
        assert_eq!(run(&mut o, 0, 5, 50, &over).last(), Some(&Some(60)));
        assert!(
            run(&mut o, 5, 30, 50, &near).iter().all(|a| *a == Some(60)),
            "holds for as long as it is within 2 °C"
        );
        let held = run(&mut o, 35, 10, 50, &cool);
        assert!(held.iter().all(|a| *a == Some(60)), "10 s first: {held:?}");
        let eased = run(&mut o, 45, 12, 50, &cool);
        assert_eq!(eased[..4], [Some(59), Some(58), Some(57), Some(56)]);
        assert_eq!(eased[8], Some(51));
        assert!(
            eased[9..].iter().all(Option::is_none),
            "back on the curve: no request"
        );
    }

    #[test]
    fn a_dip_that_does_not_last_10_s_does_not_start_the_easing() {
        let over = [cooled(80.0, 75.0, None)];
        let near = [cooled(74.0, 75.0, None)];
        let cool = [cooled(72.0, 75.0, None)];
        let mut o = Optimiser::default();
        run(&mut o, 0, 5, 50, &over);
        assert!(run(&mut o, 5, 9, 50, &cool).iter().all(|a| *a == Some(60)));
        assert_eq!(
            run(&mut o, 14, 1, 50, &near),
            [Some(60)],
            "the timer restarts"
        );
        assert!(run(&mut o, 15, 10, 50, &cool)
            .iter()
            .all(|a| *a == Some(60)));
        assert_eq!(run(&mut o, 25, 1, 50, &cool), [Some(59)]);
    }

    #[test]
    fn easing_stops_and_raising_resumes_if_it_gets_hot_again() {
        let over = [cooled(80.0, 75.0, None)];
        let cool = [cooled(72.0, 75.0, None)];
        let mut o = Optimiser::default();
        run(&mut o, 0, 5, 50, &over);
        assert_eq!(run(&mut o, 5, 12, 50, &cool).last(), Some(&Some(58)));
        assert_eq!(run(&mut o, 17, 1, 50, &over), [Some(60)], "8 + 2 points");
    }

    #[test]
    fn an_unknown_reading_holds_the_raise_instead_of_easing_or_dropping_it() {
        let over = [cooled(80.0, 75.0, None)];
        let unknown = [Cooled {
            sample: None,
            ..cooled(0.0, 75.0, None)
        }];
        let mut o = Optimiser::default();
        run(&mut o, 0, 5, 50, &over);
        assert!(run(&mut o, 5, 120, 50, &unknown)
            .iter()
            .all(|a| *a == Some(60)));
    }

    #[test]
    fn nothing_is_lowered_while_a_raise_is_held_or_easing() {
        let m = model(2.0);
        let over = [cooled(80.0, 75.0, Some(&m))];
        let ok = [cooled(60.0, 75.0, Some(&m))]; // would be lowered 15 if nothing were raised
        let mut o = Optimiser::default();
        run(&mut o, 0, 3, 50, &over); // +6
        let asks = run(&mut o, 3, 16, 50, &ok);
        assert!(asks[..10].iter().all(|a| *a == Some(56)), "held: {asks:?}");
        assert_eq!(
            asks[10..15],
            [Some(55), Some(54), Some(53), Some(52), Some(51)]
        );
        assert_eq!(asks[15], Some(35), "only once it is gone");
    }

    #[test]
    fn a_predicted_only_boost_keeps_plus_20_and_the_60_s_cap() {
        let m = model(3.0); // 74.5 + 3 − 2 = 75.5 predicted, but not over now
        let pred = [cooled(74.5, 75.0, Some(&m))];
        let ok = [cooled(60.0, 75.0, None)];
        let mut o = Optimiser::default();
        assert_eq!(run(&mut o, 0, 1, 50, &pred), [Some(70)]);
        assert!(run(&mut o, 1, 59, 50, &pred).iter().all(|a| *a == Some(70)));
        assert_eq!(run(&mut o, 60, 1, 50, &pred), [None], "spent");
        assert_eq!(
            run(&mut o, 100, 1, 50, &pred),
            [None],
            "still spent while the need lasts"
        );
        assert_eq!(run(&mut o, 101, 1, 50, &ok), [None]);
        assert_eq!(run(&mut o, 161, 1, 50, &pred), [Some(70)], "re-armed");
        let hot = [cooled(80.0, 75.0, Some(&m))];
        let mut o = Optimiser::default();
        assert_eq!(run(&mut o, 0, 1, 90, &pred), [Some(100)], "capped at 100");
        assert!(run(&mut o, 1, 5, 90, &hot).iter().all(|a| *a == Some(100)));
    }

    #[test]
    fn being_over_target_does_not_use_up_the_boost_window() {
        // 100 s over target is a raise, not a boost: the 60 s window is untouched, so a
        // predicted spike right after still gets its full +20.
        let over = [cooled(80.0, 75.0, None)];
        let mut o = Optimiser::default();
        run(&mut o, 0, 100, 50, &over);
        assert!(o.boost_since.is_none());
        let m = model(3.0);
        let pred = [cooled(74.5, 75.0, Some(&m))];
        let mut fresh = Optimiser::default();
        assert_eq!(run(&mut fresh, 0, 1, 50, &pred), [Some(70)]);
        assert!(fresh.boost_since.is_some(), "while a predicted boost does");
    }

    #[test]
    fn the_request_is_the_larger_of_the_raise_and_the_boost() {
        let over = [cooled(80.0, 75.0, None)];
        let m = model(3.0);
        let pred = [cooled(74.5, 75.0, Some(&m))];
        let mut o = Optimiser::default();
        run(&mut o, 0, 2, 50, &over); // +4
        assert_eq!(
            run(&mut o, 2, 1, 50, &pred),
            [Some(70)],
            "boost 70 beats raise 54"
        );
        assert!(run(&mut o, 3, 59, 50, &pred).iter().all(|a| *a == Some(70)));
        assert_eq!(
            run(&mut o, 62, 1, 50, &pred),
            [Some(54)],
            "boost spent: the raise remains"
        );
    }

    #[test]
    fn no_lowering_straight_after_a_boost() {
        let m = model(2.0);
        let m3 = model(3.0);
        let pred = [cooled(74.5, 75.0, Some(&m3))];
        let ok = [cooled(70.0, 75.0, Some(&m))];
        let mut o = Optimiser::default();
        assert!(o.decide(T0, 50, 0, false, &pred).is_some());
        assert!(o.decide(T0 + secs(1), 50, 0, false, &ok).is_none());
        assert!(o.decide(T0 + secs(61), 50, 0, false, &ok).is_some());
    }

    #[test]
    fn never_touches_a_pump_and_a_pump_clears_any_raise() {
        let over = [cooled(80.0, 75.0, None)];
        let ok = [cooled(60.0, 75.0, None)];
        let mut o = Optimiser::default();
        assert!(o.decide(T0, 50, 0, false, &over).is_some());
        assert_eq!(o.decide(T0 + secs(1), 50, 0, true, &over), None);
        assert_eq!(
            o.decide(T0 + secs(2), 50, 0, false, &ok),
            None,
            "no raise left to hold"
        );
    }

    #[test]
    fn the_startup_prediction_that_boosted_a_cold_gpu_is_refused() {
        // 2026-11-05: GPU 40 °C, the fan still reading duty 0 as it spun up. The fitted model
        // (never seen below duty 30) predicted 84.6 °C and the agent boosted the fan to 61%.
        let m = ThermalModel::with_weights(AHEAD, [57.73, -0.3738, -0.6987, -0.0959, 0.0472])
            .with_range([28.0, 30.0, 0.0, 51.0], [75.0, 64.0, 100.0, 123.0]);
        let startup = Cooled {
            sample: Some(Sample {
                temp: 40.0,
                duty: 0.0,
                load: 7.0,
                power: 52.0,
            }),
            target: 72.5,
            model: Some(&m),
        };
        assert_eq!(
            Optimiser::default().decide(T0, 41, 50, false, &[startup]),
            None
        );
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
