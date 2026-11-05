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
    w: [f64; N],
}

fn row(s: &Sample) -> [f64; N] {
    [1.0, s.temp, s.duty, s.load, s.power]
}

fn finite(s: &Sample) -> bool {
    row(s).iter().all(|v| v.is_finite())
}

impl ThermalModel {
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
        (w.iter().all(|v| v.is_finite()) && w[2] <= 0.0 && w[4] >= 0.0).then_some(Self { w })
    }

    /// Temperature `ahead` ticks from now, or `None` for a non-finite input.
    pub fn predict(&self, now: &Sample) -> Option<f64> {
        if !finite(now) {
            return None;
        }
        let d: f64 = row(now).iter().zip(&self.w).map(|(x, w)| x * w).sum();
        Some(now.temp + d).filter(|t| t.is_finite())
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
                .map(|(now, later)| (model.predict(now).expect("predict") - later.temp).abs())
                .collect();
            errs.iter().sum::<f64>() / errs.len() as f64
        };
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
        assert!(m.predict(&s).is_none());
    }
}
