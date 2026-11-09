//! Fan health and dust verdicts (SPEC §7.6, M16). Pure: history rows and readings come in, a
//! verdict comes out. They only ever raise alerts; they never change control.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

const DAY: i64 = 86_400;
/// Fewest samples in a bucket before a comparison is trusted (an hour of 1/min rows).
pub const MIN_SAMPLES: usize = 60;
/// A fan whose RPM at the same duty fell by this many percent "may be failing".
pub const DECLINE_PCT: u32 = 15;
/// A component this much hotter at the same load, with fans this many points faster.
pub const DUST_TEMP_C: f32 = 5.0;
pub const DUST_DUTY_PTS: f32 = 10.0;
/// Zero RPM for this long, from a fan whose tach has been seen, is a stall.
pub const STALL_SECS: u64 = 30;
/// Below this duty a fan may legitimately be stopped (0-RPM mode) or not yet turning: the
/// start duty, as far as the daemon knows it (a profile's per-fan `start_percent` is not here).
pub const MIN_STALL_DUTY: u8 = 20;
/// A fan must show this much history before a decline is judged.
const MIN_SPAN_DAYS: i64 = 14;
const BASELINE_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanHealth {
    Ok,
    /// RPM at the same duty (bucket start) fell by `drop_pct`.
    Declining {
        drop_pct: u32,
        duty: u8,
    },
    /// The fan never reported RPM: its tach is not connected. Not the same as healthy.
    Unmonitored,
    InsufficientHistory,
}

fn median<T: Copy + PartialOrd>(v: &mut [T]) -> T {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// `rows` are `(unix time, rpm, duty %)` for one fan, up to 30 days. The first week of the
/// window is the baseline, the last week the present; only rows at the same 5-point duty
/// bucket are compared, and rows with 0 RPM never count (0-RPM mode is not a decline).
pub fn fan_history(rows: &[(i64, u32, u8)], now: i64) -> FanHealth {
    let window: Vec<_> = rows
        .iter()
        .filter(|r| r.0 >= now - 30 * DAY && r.0 <= now)
        .collect();
    if !window.iter().any(|r| r.1 > 0) {
        return FanHealth::Unmonitored;
    }
    let Some(oldest) = window.iter().map(|r| r.0).min() else {
        return FanHealth::Unmonitored;
    };
    if oldest > now - MIN_SPAN_DAYS * DAY {
        return FanHealth::InsufficientHistory;
    }
    let mut then: BTreeMap<u8, Vec<u32>> = BTreeMap::new();
    let mut recent: BTreeMap<u8, Vec<u32>> = BTreeMap::new();
    for &&(t, rpm, duty) in &window {
        if rpm == 0 {
            continue;
        }
        if t < oldest + BASELINE_DAYS * DAY {
            then.entry(duty / 5).or_default().push(rpm);
        } else if t >= now - BASELINE_DAYS * DAY {
            recent.entry(duty / 5).or_default().push(rpm);
        }
    }
    let mut worst: Option<(u32, u8)> = None;
    let mut compared = false;
    for (bucket, a) in &mut then {
        let Some(b) = recent.get_mut(bucket) else {
            continue;
        };
        if a.len() < MIN_SAMPLES || b.len() < MIN_SAMPLES {
            continue;
        }
        compared = true;
        let (was, is) = (median(a), median(b));
        let drop = (was.saturating_sub(is) * 100) / was;
        if drop >= DECLINE_PCT && worst.is_none_or(|(d, _)| drop > d) {
            worst = Some((drop, *bucket * 5));
        }
    }
    match (worst, compared) {
        (Some((drop_pct, duty)), _) => FanHealth::Declining { drop_pct, duty },
        (None, true) => FanHealth::Ok,
        (None, false) => FanHealth::InsufficientHistory,
    }
}

/// Live stall detection: a fan whose tach has been seen reading 0 RPM for `STALL_SECS` at or
/// above the start duty. It stays stalled for as long as that holds, however long ago the fan
/// last turned: a dead fan must not clear its own alert.
#[derive(Default)]
pub struct StallTracker {
    zero_since: HashMap<String, Duration>,
}

impl StallTracker {
    /// `fans` are `(id, rpm, duty)`; `tach_seen(id)` says whether the fan has ever reported
    /// RPM above 0. Returns the ids stalled now.
    pub fn step(
        &mut self,
        now: Duration,
        fans: &[(String, u32, u8)],
        tach_seen: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut stalled = Vec::new();
        self.zero_since
            .retain(|id, _| fans.iter().any(|f| &f.0 == id));
        for (id, rpm, duty) in fans {
            if *rpm == 0 && *duty >= MIN_STALL_DUTY && tach_seen(id) {
                let since = *self.zero_since.entry(id.clone()).or_insert(now);
                if now.saturating_sub(since).as_secs() >= STALL_SECS {
                    stalled.push(id.clone());
                }
            } else {
                self.zero_since.remove(id);
            }
        }
        stalled
    }
}

/// One minute of a component: its temperature, its load and the fans' median duty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aligned {
    pub temp: f32,
    pub load: f32,
    pub duty: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DustVerdict {
    Clear,
    Dusty {
        load_pct: u32,
        temp_rise: f32,
        duty_rise: f32,
    },
    InsufficientHistory,
}

/// Same 10-point load bucket, `DUST_TEMP_C` hotter and `DUST_DUTY_PTS` more fan duty. Hotter
/// with the fans no faster is a warm room, not dust.
pub fn dust(baseline: &[Aligned], recent: &[Aligned]) -> DustVerdict {
    let by_load = |v: &[Aligned]| {
        let mut m: BTreeMap<u32, Vec<Aligned>> = BTreeMap::new();
        for a in v {
            m.entry((a.load / 10.0) as u32).or_default().push(*a);
        }
        m
    };
    let (then, now) = (by_load(baseline), by_load(recent));
    let mut compared = false;
    let mut worst: Option<DustVerdict> = None;
    for (bucket, a) in &then {
        let Some(b) = now.get(bucket) else { continue };
        if a.len() < MIN_SAMPLES || b.len() < MIN_SAMPLES {
            continue;
        }
        compared = true;
        let med = |v: &[Aligned], f: fn(&Aligned) -> f32| {
            median(&mut v.iter().map(f).collect::<Vec<_>>())
        };
        let temp_rise = med(b, |x| x.temp) - med(a, |x| x.temp);
        let duty_rise = med(b, |x| x.duty) - med(a, |x| x.duty);
        let worse = match worst {
            Some(DustVerdict::Dusty { temp_rise: t, .. }) => temp_rise > t,
            _ => true,
        };
        if temp_rise >= DUST_TEMP_C && duty_rise >= DUST_DUTY_PTS && worse {
            worst = Some(DustVerdict::Dusty {
                load_pct: bucket * 10,
                temp_rise,
                duty_rise,
            });
        }
    }
    match (worst, compared) {
        (Some(v), _) => v,
        (None, true) => DustVerdict::Clear,
        (None, false) => DustVerdict::InsufficientHistory,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DAY: i64 = 86_400;
    const NOW: i64 = 100 * DAY;

    /// One row a minute for `days` days ending at NOW, all at `duty`, with `rpm_at(age_days)`.
    fn rows(days: i64, duty: u8, rpm_at: impl Fn(i64) -> u32) -> Vec<(i64, u32, u8)> {
        (0..days * 24 * 60)
            .map(|m| {
                let t = NOW - days * DAY + m * 60;
                (t, rpm_at((NOW - t) / DAY), duty)
            })
            .collect()
    }

    #[test]
    fn a_fan_that_never_reported_rpm_is_unmonitored_never_healthy() {
        assert_eq!(
            fan_history(&rows(30, 60, |_| 0), NOW),
            FanHealth::Unmonitored
        );
        assert_eq!(fan_history(&[], NOW), FanHealth::Unmonitored);
    }

    #[test]
    fn a_young_history_is_insufficient_not_ok() {
        assert_eq!(
            fan_history(&rows(10, 60, |_| 1000), NOW),
            FanHealth::InsufficientHistory
        );
    }

    #[test]
    fn rpm_down_15_percent_at_the_same_duty_over_the_window_is_declining() {
        // 1000 RPM in the first 7 days of the window, 840 (-16%) in the last 7.
        let r = rows(30, 60, |age| {
            if age >= 23 {
                1000
            } else if age < 7 {
                840
            } else {
                920
            }
        });
        assert_eq!(
            fan_history(&r, NOW),
            FanHealth::Declining {
                drop_pct: 16,
                duty: 60
            }
        );
        // -14% is not enough.
        let r = rows(30, 60, |age| {
            if age >= 23 {
                1000
            } else if age < 7 {
                860
            } else {
                920
            }
        });
        assert_eq!(fan_history(&r, NOW), FanHealth::Ok);
    }

    #[test]
    fn only_the_same_duty_bucket_is_compared() {
        // Early rows at 80%, recent rows at 40%: slower, but not at the same duty.
        let mut r = rows(30, 80, |_| 1600);
        r.retain(|(t, _, _)| NOW - t >= 23 * DAY);
        r.extend(rows(7, 40, |_| 800));
        assert_eq!(fan_history(&r, NOW), FanHealth::InsufficientHistory);
    }

    #[test]
    fn a_bucket_with_few_samples_is_not_trusted() {
        let mut r = rows(30, 60, |age| if age >= 23 { 1000 } else { 500 });
        r.retain(|(t, _, _)| {
            NOW - t >= 23 * DAY || (t % 3600) == 0 && NOW - t < 7 * DAY && (t / 3600) % 24 < 1
        });
        assert_eq!(fan_history(&r, NOW), FanHealth::InsufficientHistory);
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Only `f` has ever reported RPM.
    fn tach_seen(id: &str) -> bool {
        id == "f"
    }

    #[test]
    fn a_stall_needs_30_seconds_of_zero_rpm_from_a_fan_whose_tach_has_been_seen() {
        let mut t = StallTracker::default();
        let fan = |rpm| vec![("f".to_owned(), rpm, 60u8)];
        assert!(t.step(secs(0), &fan(0), tach_seen).is_empty());
        assert!(t.step(secs(29), &fan(0), tach_seen).is_empty());
        assert_eq!(t.step(secs(30), &fan(0), tach_seen), ["f"]);
        assert_eq!(t.step(secs(40), &fan(0), tach_seen), ["f"], "still stalled");
        assert!(
            t.step(secs(41), &fan(900), tach_seen).is_empty(),
            "it spins again"
        );
        assert!(
            t.step(secs(42), &fan(0), tach_seen).is_empty(),
            "the clock restarted"
        );
    }

    /// M16 review: a fan that stalls and stays dead must stay stalled, however long that is.
    #[test]
    fn a_dead_fan_stays_stalled_for_as_long_as_it_reads_zero() {
        let mut t = StallTracker::default();
        let dead = [("f".to_owned(), 0u32, 60u8)];
        t.step(secs(0), &dead, tach_seen);
        assert_eq!(t.step(secs(30), &dead, tach_seen), ["f"]);
        for days in [1, 30, 31, 90] {
            assert_eq!(
                t.step(secs(30 + days * 86_400), &dead, tach_seen),
                ["f"],
                "after {days} days"
            );
        }
    }

    #[test]
    fn zero_rpm_is_no_stall_for_a_fan_with_no_tach_or_below_the_start_duty() {
        let mut t = StallTracker::default();
        let never = |_: &str| false;
        for s in [0, 31, 60] {
            assert!(
                t.step(secs(s), &[("f".to_owned(), 0, 60)], never)
                    .is_empty(),
                "a tach that never read anything is unmonitored, not stalled"
            );
            assert!(
                t.step(
                    secs(s),
                    &[("f".to_owned(), 0u32, MIN_STALL_DUTY - 1)],
                    tach_seen
                )
                .is_empty(),
                "below the start duty a fan may be stopped"
            );
        }
        t.step(secs(100), &[("f".to_owned(), 0, MIN_STALL_DUTY)], tach_seen);
        assert_eq!(
            t.step(secs(130), &[("f".to_owned(), 0, MIN_STALL_DUTY)], tach_seen),
            ["f"],
            "at the start duty it counts"
        );
    }

    fn a(temp: f32, load: f32, duty: f32) -> Aligned {
        Aligned { temp, load, duty }
    }

    fn many(n: usize, s: Aligned) -> Vec<Aligned> {
        vec![s; n]
    }

    #[test]
    fn dust_needs_5_degrees_hotter_and_10_points_more_duty_at_the_same_load() {
        let base = many(MIN_SAMPLES, a(60.0, 55.0, 40.0));
        let dusty = many(MIN_SAMPLES, a(65.0, 52.0, 50.0));
        assert_eq!(
            dust(&base, &dusty),
            DustVerdict::Dusty {
                load_pct: 50,
                temp_rise: 5.0,
                duty_rise: 10.0
            }
        );
        // Hotter but the fans did not speed up: a hot day, not dust.
        let summer = many(MIN_SAMPLES, a(66.0, 52.0, 42.0));
        assert_eq!(dust(&base, &summer), DustVerdict::Clear);
        // Fans faster but only 4 degrees hotter.
        let near = many(MIN_SAMPLES, a(64.0, 52.0, 55.0));
        assert_eq!(dust(&base, &near), DustVerdict::Clear);
    }

    #[test]
    fn dust_compares_the_same_load_bucket_and_needs_enough_samples() {
        let base = many(MIN_SAMPLES, a(60.0, 55.0, 40.0));
        let other_load = many(MIN_SAMPLES, a(80.0, 90.0, 80.0));
        assert_eq!(dust(&base, &other_load), DustVerdict::InsufficientHistory);
        let few = many(MIN_SAMPLES - 1, a(80.0, 52.0, 80.0));
        assert_eq!(dust(&base, &few), DustVerdict::InsufficientHistory);
        assert_eq!(dust(&[], &[]), DustVerdict::InsufficientHistory);
    }
}
