use crate::{invalid, Error, MAX_VALID_TEMP_C, MIN_VALID_TEMP_C};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Longest time-average window; bounds the sample buffer.
const MAX_TIME_AVERAGE_S: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sensors: Vec<CustomSensor>,
    #[serde(default)]
    pub curves: Vec<Curve>,
    #[serde(default)]
    pub controls: Vec<Control>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixFunction {
    Max,
    Min,
    Average,
    Sum,
}

/// SPEC §2: time-average, mix, offset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CustomSensor {
    TimeAverage {
        id: String,
        source: String,
        time_s: u64,
    },
    Mix {
        id: String,
        function: MixFunction,
        sources: Vec<String>,
    },
    Offset {
        id: String,
        source: String,
        offset: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphPoint {
    pub temp: f32,
    pub pwm: u8,
}

/// SPEC §2: linear, flat, graph, mix, sync, trigger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Curve {
    Linear {
        id: String,
        source: String,
        min_temp: f32,
        max_temp: f32,
        min_pwm: u8,
        max_pwm: u8,
    },
    Flat {
        id: String,
        pwm: u8,
    },
    Graph {
        id: String,
        source: String,
        points: Vec<GraphPoint>,
    },
    Mix {
        id: String,
        function: MixFunction,
        curves: Vec<String>,
    },
    Trigger {
        id: String,
        source: String,
        idle_temp: f32,
        idle_pwm: u8,
        load_temp: f32,
        load_pwm: u8,
        #[serde(default = "default_response_ms")]
        response_ms: u64,
    },
    Sync {
        id: String,
        target: String,
        #[serde(default)]
        offset: i8,
    },
}

fn default_response_ms() -> u64 {
    2000
}

impl Curve {
    pub fn id(&self) -> &str {
        match self {
            Curve::Linear { id, .. }
            | Curve::Flat { id, .. }
            | Curve::Graph { id, .. }
            | Curve::Mix { id, .. }
            | Curve::Trigger { id, .. }
            | Curve::Sync { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub fan: String,
    pub curve: String,
    #[serde(default = "default_step_up")]
    pub step_up_pct: u8,
    #[serde(default = "default_step_down")]
    pub step_down_pct: u8,
    /// Floor applied after the curve, before ramping (S7).
    #[serde(default)]
    pub min_pwm: u8,
    /// 0-RPM hysteresis: the fan stops below `stop_percent` and restarts at
    /// `start_percent`. Both or neither.
    #[serde(default)]
    pub start_percent: Option<u8>,
    #[serde(default)]
    pub stop_percent: Option<u8>,
}

fn default_step_up() -> u8 {
    10
}
fn default_step_down() -> u8 {
    3
}

impl Profile {
    /// Parse and validate. Every error names the offending item.
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        if let Ok(t) = s.parse::<toml::Table>() {
            if t.contains_key("app_rules") {
                return invalid("app_rules moved to config.toml [[app_rule]]".into());
            }
            if t.contains_key("alerts") {
                return invalid("alerts moved to config.toml [[alert]]".into());
            }
        }
        let p: Self = toml::from_str(s)?;
        p.validate()?;
        Ok(p)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.name.is_empty() {
            return invalid("profile name must not be empty".into());
        }
        let mut ids = HashSet::new();
        for s in &self.sensors {
            let id = match s {
                CustomSensor::TimeAverage { id, time_s, .. } => {
                    if !(1..=MAX_TIME_AVERAGE_S).contains(time_s) {
                        return invalid(format!(
                            "sensor `{id}`: time_s must be 1..{MAX_TIME_AVERAGE_S}"
                        ));
                    }
                    id
                }
                CustomSensor::Mix { id, sources, .. } => {
                    if sources.is_empty() {
                        return invalid(format!("sensor `{id}`: sources must not be empty"));
                    }
                    id
                }
                CustomSensor::Offset { id, offset, .. } => {
                    finite(&format!("sensor `{id}`"), "offset", *offset)?;
                    id
                }
            };
            if id.is_empty() {
                return invalid("sensor id must not be empty".into());
            }
            if !ids.insert(id.as_str()) {
                return invalid(format!("duplicate sensor id `{id}`"));
            }
        }

        let mut curve_ids = HashSet::new();
        for c in &self.curves {
            if c.id().is_empty() {
                return invalid("curve id must not be empty".into());
            }
            if !curve_ids.insert(c.id()) {
                return invalid(format!("duplicate curve id `{}`", c.id()));
            }
            validate_curve(c)?;
        }
        for c in &self.curves {
            for r in curve_refs(c) {
                if !curve_ids.contains(r.as_str()) {
                    return invalid(format!("curve `{}` references unknown curve `{r}`", c.id()));
                }
            }
        }
        self.check_cycles()?;

        let mut fans = HashSet::new();
        for c in &self.controls {
            if !fans.insert(c.fan.as_str()) {
                return invalid(format!("duplicate control for fan `{}`", c.fan));
            }
            if !curve_ids.contains(c.curve.as_str()) {
                return invalid(format!(
                    "control for fan `{}` references unknown curve `{}`",
                    c.fan, c.curve
                ));
            }
            let ctx = format!("control for fan `{}`", c.fan);
            duty(&ctx, "min_pwm", c.min_pwm)?;
            match (c.start_percent, c.stop_percent) {
                (None, None) => {}
                (Some(start), Some(stop)) => {
                    duty(&ctx, "start_percent", start)?;
                    duty(&ctx, "stop_percent", stop)?;
                    if start < stop {
                        return invalid(format!(
                            "{ctx}: start_percent must not be below stop_percent"
                        ));
                    }
                }
                _ => {
                    return invalid(format!(
                        "{ctx}: set both start_percent and stop_percent, or neither"
                    ))
                }
            }
            for (k, v) in [
                ("step_up_pct", c.step_up_pct),
                ("step_down_pct", c.step_down_pct),
            ] {
                // 0 would freeze the fan at its current duty
                if !(1..=100).contains(&v) {
                    return invalid(format!("{ctx}: {k} {v} is outside 1..100"));
                }
            }
        }
        Ok(())
    }

    /// Mix/sync chains must be a DAG or the engine would recurse forever.
    fn check_cycles(&self) -> Result<(), Error> {
        let edges: HashMap<&str, Vec<&String>> = self
            .curves
            .iter()
            .map(|c| (c.id(), curve_refs(c)))
            .collect();
        // 0 = unvisited, 1 = on the current path, 2 = done
        let mut state: HashMap<&str, u8> = HashMap::new();
        fn visit<'a>(
            n: &'a str,
            edges: &HashMap<&'a str, Vec<&'a String>>,
            state: &mut HashMap<&'a str, u8>,
        ) -> Result<(), Error> {
            match state.get(n) {
                Some(1) => return invalid(format!("curve cycle through `{n}`")),
                Some(_) => return Ok(()),
                None => {}
            }
            state.insert(n, 1);
            for next in &edges[n] {
                visit(next, edges, state)?;
            }
            state.insert(n, 2);
            Ok(())
        }
        for c in &self.curves {
            visit(c.id(), &edges, &mut state)?;
        }
        Ok(())
    }
}

fn curve_refs(c: &Curve) -> Vec<&String> {
    match c {
        Curve::Mix { curves, .. } => curves.iter().collect(),
        Curve::Sync { target, .. } => vec![target],
        _ => vec![],
    }
}

fn finite(ctx: &str, field: &str, v: f32) -> Result<(), Error> {
    if v.is_finite() {
        Ok(())
    } else {
        invalid(format!("{ctx}: {field} must be finite"))
    }
}

fn duty(ctx: &str, field: &str, v: u8) -> Result<(), Error> {
    if v <= 100 {
        Ok(())
    } else {
        invalid(format!("{ctx}: {field} {v} is above 100"))
    }
}

fn validate_curve(c: &Curve) -> Result<(), Error> {
    let ctx = format!("curve `{}`", c.id());
    let ctx = ctx.as_str();
    match c {
        Curve::Linear {
            min_temp,
            max_temp,
            min_pwm,
            max_pwm,
            ..
        } => {
            temp(ctx, "min_temp", *min_temp)?;
            temp(ctx, "max_temp", *max_temp)?;
            if min_temp >= max_temp {
                return invalid(format!("{ctx}: min_temp must be below max_temp"));
            }
            duty(ctx, "min_pwm", *min_pwm)?;
            duty(ctx, "max_pwm", *max_pwm)?;
            // an inverted curve would slow the fan as it gets hotter
            if min_pwm > max_pwm {
                return invalid(format!("{ctx}: min_pwm must not exceed max_pwm"));
            }
            Ok(())
        }
        Curve::Flat { pwm, .. } => duty(ctx, "pwm", *pwm),
        Curve::Graph { points, .. } => {
            if points.len() < 2 {
                return invalid(format!("{ctx}: graph needs at least two points"));
            }
            for p in points {
                temp(ctx, "temp", p.temp)?;
                duty(ctx, "pwm", p.pwm)?;
            }
            if !points.windows(2).all(|w| w[0].temp < w[1].temp) {
                return invalid(format!("{ctx}: point temps must be strictly increasing"));
            }
            if !points.windows(2).all(|w| w[0].pwm <= w[1].pwm) {
                return invalid(format!("{ctx}: point pwm must be non-decreasing"));
            }
            Ok(())
        }
        Curve::Mix { curves, .. } => {
            if curves.is_empty() {
                return invalid(format!("{ctx}: mix needs at least one curve"));
            }
            Ok(())
        }
        Curve::Trigger {
            idle_temp,
            idle_pwm,
            load_temp,
            load_pwm,
            response_ms,
            ..
        } => {
            temp(ctx, "idle_temp", *idle_temp)?;
            temp(ctx, "load_temp", *load_temp)?;
            if load_temp <= idle_temp {
                return invalid(format!("{ctx}: load_temp must be above idle_temp"));
            }
            duty(ctx, "idle_pwm", *idle_pwm)?;
            duty(ctx, "load_pwm", *load_pwm)?;
            if idle_pwm > load_pwm {
                return invalid(format!("{ctx}: idle_pwm must not exceed load_pwm"));
            }
            if *response_ms == 0 {
                return invalid(format!("{ctx}: response_ms must be above 0"));
            }
            Ok(())
        }
        Curve::Sync { offset, .. } => {
            if !(-100..=100).contains(offset) {
                return invalid(format!("{ctx}: offset {offset} is outside -100..100"));
            }
            Ok(())
        }
    }
}

/// Curve temperatures must be plausible sensor values (S3 range), not just finite.
fn temp(ctx: &str, field: &str, v: f32) -> Result<(), Error> {
    if (MIN_VALID_TEMP_C..=MAX_VALID_TEMP_C).contains(&v) {
        Ok(())
    } else {
        invalid(format!(
            "{ctx}: {field} must be a finite value within {MIN_VALID_TEMP_C}..{MAX_VALID_TEMP_C}"
        ))
    }
}
