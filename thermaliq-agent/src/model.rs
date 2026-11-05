//! Thermal model (SPEC §7.2): predicts a component's temperature `ahead` ticks later.
//! Linear on the temperature change: dT = w · [1, temp, duty, load, power], fitted by
//! ridge least squares. This is exact for a first-order system with steady inputs.
//! shortcut: linear, one model per component, no duty/power lag; upgrade if held-out
//! error on real history exceeds 2 °C (SPEC §7.5).

use crate::inputs::Sample;

const N: usize = 5;
const MIN_ROWS: usize = 10 * N;
const RIDGE: f64 = 1e-6;

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
    /// `history` must be consecutive ticks. `None` if it is too short, has a non-finite
    /// value, or is degenerate: the caller then has no prediction, not a wrong one.
    pub fn fit(history: &[Sample], ahead: usize) -> Option<Self> {
        if ahead == 0 || history.len() < ahead + MIN_ROWS || !history.iter().all(finite) {
            return None;
        }
        let mut a = [[0.0; N + 1]; N]; // augmented [XᵀX + λI | Xᵀy]
        for (now, later) in history.iter().zip(&history[ahead..]) {
            let (x, y) = (row(now), later.temp - now.temp);
            for i in 0..N {
                for j in 0..N {
                    a[i][j] += x[i] * x[j];
                }
                a[i][N] += x[i] * y;
            }
        }
        for (i, r) in a.iter_mut().enumerate() {
            r[i] += RIDGE;
        }
        // Gaussian elimination with partial pivoting.
        for c in 0..N {
            let p = (c..N).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
            if a[p][c].abs() < 1e-12 {
                return None;
            }
            a.swap(c, p);
            for r in 0..N {
                if r != c {
                    let f = a[r][c] / a[c][c];
                    let pivot_row = a[c];
                    for (x, p) in a[r].iter_mut().zip(pivot_row).skip(c) {
                        *x -= f * p;
                    }
                }
            }
        }
        let mut w = [0.0; N];
        for i in 0..N {
            w[i] = a[i][N] / a[i][i];
        }
        w.iter().all(|v| v.is_finite()).then_some(Self { w })
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

    /// First-order plant with inputs held for 40 s at a time (a game starts, the fan
    /// ramps). Deterministic LCG so the test never flakes.
    fn plant(ticks: usize, seed: u64) -> Vec<Sample> {
        let mut rng = seed;
        let mut next = move |max: f64| {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as f64 / (1u64 << 31) as f64 * max
        };
        let (mut t, mut duty, mut load, mut power) = (40.0, 30.0, 10.0, 20.0);
        (0..ticks)
            .map(|i| {
                if i % 120 == 0 {
                    duty = 20.0 + next(80.0);
                    load = next(100.0);
                    power = load * 2.0 + next(10.0);
                }
                let s = Sample {
                    temp: t,
                    duty,
                    load,
                    power,
                };
                let eq = 28.0 + 0.12 * power + 0.1 * load - 0.15 * duty;
                t += (eq - t) * 0.08;
                s
            })
            .collect()
    }

    #[test]
    fn twenty_second_error_within_2c_on_held_out_history() {
        let train = plant(4000, 1);
        let test = plant(2000, 99);
        let model = ThermalModel::fit(&train, 20).expect("fit");
        // Windows where the inputs change mid-way are unpredictable by any model; the
        // real-history number comes from the M20 spike (SPEC §7.5).
        let errs: Vec<f64> = test
            .iter()
            .zip(&test[20..])
            .filter(|(now, later)| (now.duty, now.load) == (later.duty, later.load))
            .map(|(now, later)| (model.predict(now).expect("predict") - later.temp).abs())
            .collect();
        let mean = errs.iter().sum::<f64>() / errs.len() as f64;
        assert!(mean <= 2.0, "mean error {mean}");
    }

    #[test]
    fn short_or_bad_history_gives_no_model() {
        let h = plant(200, 1);
        assert!(ThermalModel::fit(&h[..30], 20).is_none());
        assert!(ThermalModel::fit(&h, 0).is_none());
        let mut bad = h.clone();
        bad[50].power = f64::NAN;
        assert!(ThermalModel::fit(&bad, 20).is_none());
    }

    #[test]
    fn non_finite_input_gives_no_prediction() {
        let h = plant(300, 1);
        let m = ThermalModel::fit(&h, 20).expect("fit");
        let mut s = h[0];
        s.load = f64::INFINITY;
        assert!(m.predict(&s).is_none());
    }
}
