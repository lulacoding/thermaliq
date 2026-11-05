//! Thermal model (SPEC §7.2): predicts a component's temperature `ahead` ticks later.
//! Linear on the temperature change: dT = w · [1, temp, duty, load, power], fitted by
//! ridge least squares on standardised columns. A fit that says "more fan heats the part"
//! or "more watts cool it" is rejected: history is recorded in closed loop (the curve sets
//! duty from temperature), which makes a plain fit identify those signs wrongly, and
//! M20b would act on them (SPEC §7.5).
//! shortcut: linear, one model per component, no duty/power lag; upgrade if held-out
//! error on real history exceeds 2 °C (SPEC §7.5).

use crate::inputs::Sample;

const N: usize = 5; // bias, temp, duty, load, power
const P: usize = N - 1; // fitted (standardised) columns
const MIN_ROWS: usize = 10 * N;
/// Ridge strength relative to the row count (standardised columns have diagonal ~ n).
const RIDGE: f64 = 0.01;

#[derive(Debug, Clone, PartialEq)]
pub struct ThermalModel {
    ahead: usize,
    w: [f64; N],
    /// The range of [temp, duty, load, power] the fit saw. A linear model is only believable
    /// inside it: at duty 0 (a fan spinning up) with a −0.7 °C-per-point coefficient it
    /// predicted +44 °C and boosted a cold GPU.
    lo: [f64; P],
    hi: [f64; P],
}

/// Outside the fitted range by up to this share of it still counts as inside (quantisation).
const RANGE_SLACK: f64 = 0.05;

fn row(s: &Sample) -> [f64; N] {
    [1.0, s.temp, s.duty, s.load, s.power]
}

fn finite(s: &Sample) -> bool {
    row(s).iter().all(|v| v.is_finite())
}

impl ThermalModel {
    #[cfg(test)]
    pub(crate) fn with_weights(ahead: usize, w: [f64; N]) -> Self {
        Self {
            ahead,
            w,
            lo: [f64::NEG_INFINITY; P],
            hi: [f64::INFINITY; P],
        }
    }

    #[cfg(test)]
    pub(crate) fn with_range(mut self, lo: [f64; P], hi: [f64; P]) -> Self {
        (self.lo, self.hi) = (lo, hi);
        self
    }

    /// `history` must be **consecutive 1-tick samples**: restarts and missing ticks make
    /// `ahead` mean something else, so the caller splits history at every gap and fits
    /// one gap-free run. `None` if the run is too short, has a non-finite value, has a
    /// constant column (an effect that cannot be identified), or gives a physically
    /// impossible fit (`w[duty] > 0` or `w[power] < 0`).
    #[allow(clippy::needless_range_loop)] // small fixed-size linear algebra
    pub fn fit(history: &[Sample], ahead: usize) -> Option<Self> {
        if ahead == 0 || history.len() < ahead + MIN_ROWS || !history.iter().all(finite) {
            return None;
        }
        let pairs = || history.iter().zip(&history[ahead..]);
        let n = pairs().count() as f64;
        let col = |now: &Sample| -> [f64; P] { [now.temp, now.duty, now.load, now.power] };
        let (mut mean, mut mean_y) = ([0.0; P], 0.0);
        for (now, later) in pairs() {
            for (m, x) in mean.iter_mut().zip(col(now)) {
                *m += x / n;
            }
            mean_y += (later.temp - now.temp) / n;
        }
        let mut std = [0.0; P];
        for (now, _) in pairs() {
            for i in 0..P {
                std[i] += (col(now)[i] - mean[i]).powi(2) / n;
            }
        }
        let std = std.map(f64::sqrt);
        if std.iter().any(|s| *s < 1e-9) {
            return None;
        }
        let mut a = [[0.0; P + 1]; P]; // augmented [ZᵀZ + λnI | Zᵀ(y - ȳ)]
        for (now, later) in pairs() {
            let z: [f64; P] = std::array::from_fn(|i| (col(now)[i] - mean[i]) / std[i]);
            let y = later.temp - now.temp - mean_y;
            for i in 0..P {
                for j in 0..P {
                    a[i][j] += z[i] * z[j];
                }
                a[i][P] += z[i] * y;
            }
        }
        for (i, r) in a.iter_mut().enumerate() {
            r[i] += RIDGE * n;
        }
        // Gaussian elimination with partial pivoting.
        for c in 0..P {
            let p = (c..P).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
            if a[p][c].abs() < 1e-12 {
                return None;
            }
            a.swap(c, p);
            for r in 0..P {
                if r != c {
                    let f = a[r][c] / a[c][c];
                    let pivot_row = a[c];
                    for (x, p) in a[r].iter_mut().zip(pivot_row).skip(c) {
                        *x -= f * p;
                    }
                }
            }
        }
        // Back to raw units: w_i = v_i / std_i, bias absorbs the means.
        let mut w = [0.0; N];
        for i in 0..P {
            w[i + 1] = a[i][P] / a[i][i] / std[i];
        }
        w[0] = mean_y - (0..P).map(|i| w[i + 1] * mean[i]).sum::<f64>();
        let (mut lo, mut hi) = ([f64::INFINITY; P], [f64::NEG_INFINITY; P]);
        for (now, _) in pairs() {
            for (i, x) in col(now).into_iter().enumerate() {
                (lo[i], hi[i]) = (lo[i].min(x), hi[i].max(x));
            }
        }
        (w.iter().all(|v| v.is_finite()) && w[2] <= 0.0 && w[4] >= 0.0).then_some(Self {
            ahead,
            w,
            lo,
            hi,
        })
    }

    fn in_range(&self, s: &Sample) -> bool {
        [s.temp, s.duty, s.load, s.power]
            .iter()
            .zip(self.lo.iter().zip(&self.hi))
            .all(|(x, (lo, hi))| {
                let slack = RANGE_SLACK * (hi - lo);
                *x >= lo - slack && *x <= hi + slack
            })
    }

    /// Temperature `ahead` ticks from now. `None` for a non-finite input, an `ahead` other
    /// than the one this model was fitted for, or a sample outside the range it was fitted on.
    pub fn predict(&self, now: &Sample, ahead: usize) -> Option<f64> {
        if ahead != self.ahead || !finite(now) || !self.in_range(now) {
            return None;
        }
        let d: f64 = row(now).iter().zip(&self.w).map(|(x, w)| x * w).sum();
        Some(now.temp + d).filter(|t| t.is_finite())
    }

    /// May a fan be turned *down* on this model's word? Only if dropping its duty 10 points
    /// visibly warms the part (>= 0.1 °C at `ahead`). The fit allows `w[duty] == 0`, which
    /// would make every drop look free (M20a review).
    pub fn lowering_is_safe(&self, now: &Sample, ahead: usize) -> bool {
        let lower = Sample {
            duty: now.duty - 10.0,
            ..*now
        };
        match (self.predict(now, ahead), self.predict(&lower, ahead)) {
            (Some(a), Some(b)) => b - a >= 0.1,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plant {
        /// Duty follows a curve of the measured temperature, as with a real fan curve.
        closed_loop: bool,
        duty_gain: f64,
        power_gain: f64,
    }

    const COOLING: Plant = Plant {
        closed_loop: false,
        duty_gain: -0.15,
        power_gain: 0.12,
    };

    /// Noisy first-order plant: 1 °C sensor quantisation, ±0.5 °C noise, the plant sees
    /// duty and power 4 ticks late. Load (and power with it) is held 120 ticks at a
    /// time. Deterministic LCG so the tests never flake.
    fn simulate(p: &Plant, ticks: usize, seed: u64) -> Vec<Sample> {
        const LAG: usize = 4;
        let mut rng = seed;
        let mut next = move |max: f64| {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as f64 / (1u64 << 31) as f64 * max
        };
        let (mut t, mut load, mut duty, mut seen) = (40.0, 10.0, 30.0, 40.0_f64);
        let (mut duties, mut powers) = (Vec::new(), Vec::new());
        let mut out = Vec::new();
        for i in 0..ticks {
            if i % 120 == 0 {
                load = next(100.0);
                if !p.closed_loop {
                    duty = 20.0 + next(80.0);
                }
            }
            if p.closed_loop {
                duty = (20.0 + 2.0 * (seen - 40.0)).clamp(20.0, 100.0);
            }
            duties.push(duty);
            powers.push(load * 2.0 + next(10.0));
            out.push(Sample {
                temp: seen,
                duty,
                load,
                power: powers[i],
            });
            let lagged = i.saturating_sub(LAG);
            let eq =
                28.0 + p.power_gain * powers[lagged] + 0.1 * load + p.duty_gain * duties[lagged];
            t += (eq - t) * 0.08;
            seen = (t + next(1.0) - 0.5).round();
        }
        out
    }

    #[test]
    fn twenty_second_error_within_2c_on_held_out_history() {
        let model = ThermalModel::fit(&simulate(&COOLING, 4000, 1), 20).expect("fit");
        let test = simulate(&COOLING, 2000, 99);
        let mean = |steady_only: bool| {
            let errs: Vec<f64> = test
                .iter()
                .zip(&test[20..])
                // Windows where the inputs change mid-way are unpredictable by any model;
                // the real-history number comes from the M20 spike (SPEC §7.5).
                .filter(|(now, later)| {
                    !steady_only || (now.duty, now.load) == (later.duty, later.load)
                })
                // outside the fitted range the model says "unknown": scored separately below
                .filter_map(|(now, later)| Some((model.predict(now, 20)? - later.temp).abs()))
                .collect();
            errs.iter().sum::<f64>() / errs.len() as f64
        };
        let covered = test[..test.len() - 20]
            .iter()
            .filter(|s| model.predict(s, 20).is_some())
            .count() as f64
            / (test.len() - 20) as f64;
        assert!(
            covered > 0.9,
            "the model refuses {:.0}% of ordinary samples",
            (1.0 - covered) * 100.0
        );
        println!(
            "20 s error: steady windows {:.2} °C, all windows {:.2} °C",
            mean(true),
            mean(false)
        );
        assert!(mean(true) <= 2.0, "steady-window error {}", mean(true));
    }

    #[test]
    fn closed_loop_history_never_gives_a_wrong_sign() {
        let closed = Plant {
            closed_loop: true,
            ..COOLING
        };
        let mut fitted = 0;
        for seed in 1..=20 {
            if let Some(m) = ThermalModel::fit(&simulate(&closed, 3000, seed), 20) {
                assert!(m.w[2] <= 0.0 && m.w[4] >= 0.0, "seed {seed}: w = {:?}", m.w);
                fitted += 1;
            }
        }
        println!("closed loop: {fitted}/20 fits accepted");
    }

    #[test]
    fn impossible_physics_is_rejected() {
        let heating_fan = Plant {
            duty_gain: 0.15,
            ..COOLING
        };
        assert!(ThermalModel::fit(&simulate(&heating_fan, 3000, 1), 20).is_none());
        let cooling_watts = Plant {
            power_gain: -0.12,
            ..COOLING
        };
        assert!(ThermalModel::fit(&simulate(&cooling_watts, 3000, 1), 20).is_none());
    }

    #[test]
    fn constant_column_is_not_identifiable() {
        let mut h = simulate(&COOLING, 300, 1);
        h.iter_mut().for_each(|s| s.duty = 50.0);
        assert!(ThermalModel::fit(&h, 20).is_none());
    }

    #[test]
    fn short_or_bad_history_gives_no_model() {
        let h = simulate(&COOLING, 200, 1);
        assert!(ThermalModel::fit(&h[..30], 20).is_none());
        assert!(ThermalModel::fit(&h, 0).is_none());
        let mut bad = h.clone();
        bad[50].power = f64::NAN;
        assert!(ThermalModel::fit(&bad, 20).is_none());
    }

    #[test]
    fn non_finite_input_gives_no_prediction() {
        let h = simulate(&COOLING, 600, 1);
        let m = ThermalModel::fit(&h, 20).expect("fit");
        let mut s = h[0];
        s.load = f64::INFINITY;
        assert!(m.predict(&s, 20).is_none());
    }

    #[test]
    fn a_prediction_for_a_different_horizon_is_refused() {
        let h = simulate(&COOLING, 600, 1);
        let m = ThermalModel::fit(&h, 20).expect("fit");
        assert!(m.predict(&h[0], 20).is_some());
        assert!(m.predict(&h[0], 10).is_none());
        assert!(!m.lowering_is_safe(&h[0], 10));
    }

    /// The coefficients and the ranges of Sam's 2026-11-05 run, where the agent's first tick
    /// (GPU 40 °C, fan reading duty 0 while it spun up) predicted 84.6 °C and boosted the fan.
    fn incident_model() -> ThermalModel {
        ThermalModel::with_weights(20, [57.73, -0.3738, -0.6987, -0.0959, 0.0472])
            .with_range([28.0, 30.0, 0.0, 51.0], [75.0, 64.0, 100.0, 123.0])
    }

    #[test]
    fn a_startup_sample_far_outside_the_fitted_range_gets_no_prediction() {
        let m = incident_model();
        let startup = Sample {
            temp: 40.0,
            duty: 0.0,
            load: 7.0,
            power: 52.0,
        };
        assert_eq!(
            m.predict(&startup, 20),
            None,
            "duty 0 was never seen: a linear fit would say 84.6"
        );
        let one_tick_later = Sample {
            duty: 30.0,
            ..startup
        };
        assert!(
            m.predict(&one_tick_later, 20).is_some(),
            "inside the range it still predicts"
        );
    }

    #[test]
    fn every_input_has_its_own_range() {
        let m = incident_model();
        let ok = Sample {
            temp: 50.0,
            duty: 45.0,
            load: 50.0,
            power: 80.0,
        };
        assert!(m.predict(&ok, 20).is_some());
        for bad in [
            Sample { temp: 10.0, ..ok },
            Sample { temp: 90.0, ..ok },
            Sample { duty: 100.0, ..ok },
            Sample { load: -10.0, ..ok },
            Sample { power: 10.0, ..ok },
            Sample { power: 300.0, ..ok },
        ] {
            assert_eq!(m.predict(&bad, 20), None, "{bad:?}");
        }
        // a little outside (5% of the range) is tolerated: sensors are quantised
        assert!(m.predict(&Sample { duty: 29.0, ..ok }, 20).is_some());
    }

    #[test]
    fn a_fitted_model_remembers_the_range_it_was_fitted_on() {
        let h = simulate(&COOLING, 3000, 1);
        let m = ThermalModel::fit(&h, 20).expect("fit");
        let inside = h[100];
        assert!(m.predict(&inside, 20).is_some());
        assert_eq!(
            m.predict(
                &Sample {
                    duty: -50.0,
                    ..inside
                },
                20
            ),
            None
        );
        assert_eq!(
            m.predict(
                &Sample {
                    temp: 500.0,
                    ..inside
                },
                20
            ),
            None
        );
        // lowering by 10 points from the bottom of the fitted duty range leaves it
        let bottom = Sample {
            duty: 20.5,
            ..inside
        };
        assert!(!m.lowering_is_safe(&bottom, 20));
    }

    #[test]
    fn only_a_clearly_cooling_fan_may_be_lowered() {
        let h = simulate(&COOLING, 3000, 1);
        let m = ThermalModel::fit(&h, 20).expect("fit");
        assert!(m.lowering_is_safe(&h[0], 20));
        // duty effect of exactly 0 (the fit accepts w[duty] == 0): 10 points must show >= 0.1 °C
        let flat = Plant {
            duty_gain: 0.0,
            ..COOLING
        };
        if let Some(m) = ThermalModel::fit(&simulate(&flat, 3000, 1), 20) {
            assert!(!m.lowering_is_safe(&h[0], 20), "no real duty effect");
        }
        let weak = ThermalModel::with_weights(20, [0.0, 0.0, -0.009, 0.0, 0.0]);
        assert!(!weak.lowering_is_safe(&h[0], 20), "-0.09 °C per 10 points");
        let enough = ThermalModel::with_weights(20, [0.0, 0.0, -0.011, 0.0, 0.0]);
        assert!(enough.lowering_is_safe(&h[0], 20));
    }
}
