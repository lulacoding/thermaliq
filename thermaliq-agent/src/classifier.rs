//! Workload classifier (SPEC §7.2): gradient-boosted decision stumps, softmax over the
//! six workloads, trained offline on labelled history, predicts in microseconds.
//! shortcut: depth-1 trees on 4 load/power features (the foreground exe is the slow
//! layer's job); upgrade to deeper trees if held-out accuracy on real history is poor.

use crate::inputs::WorkloadBinding;
use thermaliq_common::HardwareSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    Idle,
    Browsing,
    Gaming,
    Compiling,
    Rendering,
    Other,
}

const ALL: [Workload; 6] = [
    Workload::Idle,
    Workload::Browsing,
    Workload::Gaming,
    Workload::Compiling,
    Workload::Rendering,
    Workload::Other,
];
const K: usize = ALL.len();
const F: usize = 4;
const ROUNDS: usize = 40;
const RATE: f64 = 0.5;

#[derive(Debug, Clone)]
struct Stump {
    class: usize,
    feat: usize,
    thr: f64,
    left: f64,
    right: f64,
}

/// Below this softmax probability the answer is "unknown": a forced argmax on an input
/// unlike the training data is a confident wrong answer.
pub const MIN_CONFIDENCE: f64 = 0.6;

#[derive(Debug, Clone)]
pub struct Classifier {
    stumps: Vec<Stump>,
    /// Features this machine has (see `WorkloadBinding::used`); the rest are ignored.
    used: [bool; F],
}

fn scores(stumps: &[Stump], x: &[f64; F]) -> [f64; K] {
    let mut s = [0.0; K];
    for st in stumps {
        s[st.class] += if x[st.feat] <= st.thr {
            st.left
        } else {
            st.right
        };
    }
    s
}

fn softmax(s: [f64; K]) -> [f64; K] {
    let max = s.iter().cloned().fold(f64::MIN, f64::max);
    let e = s.map(|v| (v - max).exp());
    let sum: f64 = e.iter().sum();
    e.map(|v| v / sum)
}

/// Best single split of `g` (the residuals) over the rows: leaf values are scaled means.
fn fit_stump(
    data: &[([f64; F], Workload)],
    g: &[f64],
    class: usize,
    used: &[bool; F],
) -> Option<Stump> {
    let total: f64 = g.iter().sum();
    let mut best: Option<(f64, Stump)> = None;
    for feat in (0..F).filter(|&f| used[f]) {
        let mut idx: Vec<usize> = (0..data.len()).collect();
        idx.sort_by(|&a, &b| data[a].0[feat].total_cmp(&data[b].0[feat]));
        let mut sum_l = 0.0;
        for (n, w) in idx.windows(2).enumerate() {
            sum_l += g[w[0]];
            let (lo, hi) = (data[w[0]].0[feat], data[w[1]].0[feat]);
            if lo == hi {
                continue;
            }
            let (nl, nr) = ((n + 1) as f64, (data.len() - n - 1) as f64);
            let (sum_r, gain) = (
                total - sum_l,
                sum_l * sum_l / nl + (total - sum_l).powi(2) / nr,
            );
            if best.as_ref().is_none_or(|(b, _)| gain > *b) {
                best = Some((
                    gain,
                    Stump {
                        class,
                        feat,
                        thr: (lo + hi) / 2.0,
                        left: RATE * sum_l / nl,
                        right: RATE * sum_r / nr,
                    },
                ));
            }
        }
    }
    best.map(|(_, s)| s)
}

impl Classifier {
    /// `used` marks the features this machine has; only those are read. `None` if no
    /// feature is used, there is no data, a used feature is non-finite, or fewer than two
    /// classes were seen.
    pub fn train(data: &[([f64; F], Workload)], used: [bool; F]) -> Option<Self> {
        let classes = ALL
            .iter()
            .filter(|w| data.iter().any(|d| d.1 == **w))
            .count();
        let bad = |x: &[f64; F]| (0..F).any(|i| used[i] && !x[i].is_finite());
        if data.is_empty() || classes < 2 || !used.contains(&true) || data.iter().any(|d| bad(&d.0))
        {
            return None;
        }
        let mut stumps = Vec::new();
        for _ in 0..ROUNDS {
            let p: Vec<[f64; K]> = data
                .iter()
                .map(|d| softmax(scores(&stumps, &d.0)))
                .collect();
            for class in 0..K {
                let g: Vec<f64> = data
                    .iter()
                    .zip(&p)
                    .map(|(d, p)| (d.1 as usize == class) as u8 as f64 - p[class])
                    .collect();
                stumps.extend(fit_stump(data, &g, class, &used));
            }
        }
        Some(Self { stumps, used })
    }

    /// The workload and its probability, or `None` ("unknown") for a non-finite used
    /// feature or a probability below `MIN_CONFIDENCE`.
    pub fn predict(&self, x: &[f64; F]) -> Option<(Workload, f64)> {
        if (0..F).any(|i| self.used[i] && !x[i].is_finite()) {
            return None;
        }
        let p = softmax(scores(&self.stumps, x));
        let best = (0..K).max_by(|&a, &b| p[a].total_cmp(&p[b]))?;
        (p[best] >= MIN_CONFIDENCE).then_some((ALL[best], p[best]))
    }

    /// `None` ("unknown") when a bound metric is missing or invalid, or the binding has
    /// different features than this classifier was trained on.
    pub fn classify(
        &self,
        snap: &HardwareSnapshot,
        b: &WorkloadBinding,
    ) -> Option<(Workload, f64)> {
        if b.used() != self.used {
            return None;
        }
        self.predict(&b.features(snap)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use thermaliq_common::{MetricKind, MetricReading};

    /// (cpu load, gpu load, cpu W, gpu W) centres per workload, plus LCG jitter.
    fn data(n: usize, seed: u64) -> Vec<([f64; F], Workload)> {
        let centres = [
            ([2.0, 1.0, 10.0, 8.0], Workload::Idle),
            ([20.0, 12.0, 25.0, 20.0], Workload::Browsing),
            ([45.0, 95.0, 70.0, 170.0], Workload::Gaming),
            ([98.0, 3.0, 120.0, 12.0], Workload::Compiling),
            ([90.0, 90.0, 110.0, 160.0], Workload::Rendering),
            ([60.0, 40.0, 60.0, 90.0], Workload::Other),
        ];
        let mut rng = seed;
        let mut jitter = move || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as f64 / (1u64 << 31) as f64 * 8.0 - 4.0
        };
        (0..n)
            .map(|i| {
                let (c, w) = centres[i % K];
                // Only the loads (first two) are percentages; watts are not capped.
                let x: [f64; F] = std::array::from_fn(|k| {
                    let v = c[k] + jitter();
                    if k < 2 {
                        v.clamp(0.0, 100.0)
                    } else {
                        v.max(0.0)
                    }
                });
                (x, w)
            })
            .collect()
    }

    const ALL_FEATURES: [bool; F] = [true; F];

    fn class_of(c: &Classifier, x: &[f64; F]) -> Option<Workload> {
        c.predict(x).map(|p| p.0)
    }

    #[test]
    fn learns_workloads_and_predicts_fast() {
        let c = Classifier::train(&data(600, 1), ALL_FEATURES).expect("train");
        let test = data(600, 7);
        let hits = test
            .iter()
            .filter(|(x, w)| class_of(&c, x) == Some(*w))
            .count();
        assert!(hits as f64 / test.len() as f64 > 0.95, "{hits}/600");
        assert!(test
            .iter()
            .all(|(x, _)| c.predict(x).is_none_or(|p| p.1 >= MIN_CONFIDENCE)));
        // watts above 100 reach the model unclamped
        assert!(test.iter().any(|(x, _)| x[3] > 100.0));
        let t = Instant::now();
        for (x, _) in &test {
            c.predict(x);
        }
        assert!(
            t.elapsed().as_micros() / 600 < 1000,
            "over the 1 ms tick budget"
        );
    }

    #[test]
    fn bad_training_data_gives_no_model() {
        assert!(Classifier::train(&[], ALL_FEATURES).is_none());
        let one_class: Vec<_> = data(60, 1)
            .into_iter()
            .filter(|d| d.1 == Workload::Idle)
            .collect();
        assert!(Classifier::train(&one_class, ALL_FEATURES).is_none());
        assert!(Classifier::train(&data(60, 1), [false; F]).is_none());
        let mut nan = data(60, 1);
        nan[3].0[2] = f64::NAN;
        assert!(Classifier::train(&nan, ALL_FEATURES).is_none());
        // a NaN in a feature the machine doesn't have is ignored
        assert!(Classifier::train(&nan, [true, true, false, true]).is_some());
    }

    #[test]
    fn ambiguous_input_is_unknown_not_a_forced_guess() {
        let c = Classifier::train(&data(600, 1), ALL_FEATURES).expect("train");
        // halfway between the Gaming and Rendering centres
        assert_eq!(c.predict(&[67.5, 92.5, 90.0, 165.0]), None);
    }

    #[test]
    fn gpu_only_machine_tells_gaming_from_idle() {
        let gpu_only = [false, true, false, true];
        let train: Vec<_> = data(600, 1)
            .into_iter()
            .filter(|d| matches!(d.1, Workload::Idle | Workload::Browsing | Workload::Gaming))
            .collect();
        let c = Classifier::train(&train, gpu_only).expect("train");
        let b = WorkloadBinding {
            gpu_load: Some("gl".into()),
            gpu_power: Some("gp".into()),
            ..Default::default()
        };
        let m = |id: &str, kind, value| MetricReading {
            id: id.into(),
            kind,
            value,
        };
        let snap = |load, watts| HardwareSnapshot {
            metrics: vec![
                m("gl", MetricKind::GpuLoad, load),
                m("gp", MetricKind::GpuPower, watts),
            ],
            ..Default::default()
        };
        assert_eq!(
            c.classify(&snap(95.0, 170.0), &b).map(|p| p.0),
            Some(Workload::Gaming)
        );
        assert_eq!(
            c.classify(&snap(1.0, 8.0), &b).map(|p| p.0),
            Some(Workload::Idle)
        );
        // a binding with different features than the training is unknown
        let full = WorkloadBinding {
            cpu_load: Some("cl".into()),
            ..b.clone()
        };
        assert_eq!(c.classify(&snap(95.0, 170.0), &full), None);
    }

    #[test]
    fn missing_or_bad_inputs_are_unknown() {
        let c = Classifier::train(&data(120, 1), ALL_FEATURES).expect("train");
        assert_eq!(c.predict(&[f64::NAN, 0.0, 0.0, 0.0]), None);
        let b = WorkloadBinding {
            cpu_load: Some("cl".into()),
            gpu_load: Some("gl".into()),
            cpu_power: Some("cp".into()),
            gpu_power: Some("gp".into()),
        };
        let m = |id: &str, kind, value| MetricReading {
            id: id.into(),
            kind,
            value,
        };
        let mut snap = HardwareSnapshot {
            metrics: vec![
                m("cl", MetricKind::CpuLoad, 98.0),
                m("gl", MetricKind::GpuLoad, 3.0),
                m("cp", MetricKind::CpuPower, 120.0),
            ],
            ..Default::default()
        };
        assert_eq!(c.classify(&snap, &b), None); // no gpu power
        snap.metrics.push(m("gp", MetricKind::GpuPower, 12.0));
        assert_eq!(
            c.classify(&snap, &b).map(|p| p.0),
            Some(Workload::Compiling)
        );
    }
}
