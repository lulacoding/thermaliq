//! Fan and sensor identity (SPEC §5.2): the saved inventory, role mapping, and the checks that
//! name a missing fan. Pure logic; the daemon does the file IO.

use crate::{invalid, Curve, CustomSensor, Error, FanKind, HardwareSnapshot, Profile, SensorKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub const FAN_ROLES: [&str; 6] = [
    "cpu_fan",
    "pump",
    "case_front",
    "case_rear",
    "case_top",
    "gpu_fan",
];
pub const SENSOR_ROLES: [&str; 2] = ["cpu_temp", "gpu_temp"];

/// A tach reading must rise at least this much for the probe to call a fan "moved".
pub const RPM_RISE: u32 = 150;

fn is_role(id: &str) -> bool {
    FAN_ROLES.contains(&id) || SENSOR_ROLES.contains(&id)
}

/// `roles.toml`: role -> the real fan or sensor id on this PC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    #[serde(default)]
    roles: BTreeMap<String, String>,
}

impl Roles {
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let r: Self = toml::from_str(s)?;
        let mut used = HashSet::new();
        for (role, id) in &r.roles {
            if !is_role(role) {
                return invalid(format!("`{role}` is not a role (known: {})", known_roles()));
            }
            if id.is_empty() {
                return invalid(format!("role `{role}` maps to an empty id"));
            }
            if !used.insert(id.as_str()) {
                return invalid(format!("`{id}` is mapped to two roles"));
            }
        }
        Ok(r)
    }

    pub fn to_toml(&self) -> String {
        // A map of strings always serialises.
        toml::to_string(self).unwrap_or_default()
    }

    pub fn get(&self, role: &str) -> Option<&str> {
        self.roles.get(role).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.roles.iter().map(|(r, i)| (r.as_str(), i.as_str()))
    }

    /// Maps `role` to `id`, taking `id` away from any other role.
    pub fn assign(&mut self, role: &str, id: &str) -> Result<(), Error> {
        if !is_role(role) || id.is_empty() {
            return invalid(format!("cannot map `{role}` to `{id}`"));
        }
        self.roles.retain(|_, v| v != id);
        self.roles.insert(role.to_owned(), id.to_owned());
        Ok(())
    }

    /// Roles no one has mapped yet, in a fixed order.
    pub fn unmapped(&self) -> Vec<&'static str> {
        FAN_ROLES
            .iter()
            .chain(&SENSOR_ROLES)
            .copied()
            .filter(|r| !self.roles.contains_key(*r))
            .collect()
    }
}

fn known_roles() -> String {
    FAN_ROLES
        .iter()
        .chain(&SENSOR_ROLES)
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// A profile with its roles replaced by real ids, and what had to be left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub profile: Profile,
    /// One line per thing skipped, naming the role or fan. Never empty when something was.
    pub skipped: Vec<String>,
}

enum Sub {
    Real(String),
    Unmapped(String),
}

/// Replaces each role id in `p` with its mapped id. An unmapped role is skipped and reported
/// (SPEC §5.2): a control on it is dropped, a curve on an unmapped sensor is dead and so is
/// every control, curve, custom sensor and alert built on it.
pub fn resolve(p: &Profile, roles: &Roles) -> Resolved {
    let mut out = p.clone();
    let mut skipped = Vec::new();
    let name = &p.name;
    let sub = |id: &str| -> Option<Sub> {
        match roles.get(id) {
            Some(real) => Some(Sub::Real(real.to_owned())),
            None if is_role(id) => Some(Sub::Unmapped(id.to_owned())),
            None => None,
        }
    };

    // Custom sensors whose source is an unmapped role, and those built on a dead one.
    let mut dead_sensors: HashSet<String> = HashSet::new();
    let mut dead_why: Vec<String> = Vec::new();
    loop {
        let before = dead_sensors.len();
        for s in &mut out.sensors {
            let (id, sources): (&str, Vec<&mut String>) = match s {
                CustomSensor::TimeAverage { id, source, .. }
                | CustomSensor::Offset { id, source, .. } => (id, vec![source]),
                CustomSensor::Mix { id, sources, .. } => (id, sources.iter_mut().collect()),
            };
            for src in sources {
                if dead_sensors.contains(src.as_str()) {
                    dead_sensors.insert(id.to_owned());
                } else if let Some(sub) = sub(src) {
                    match sub {
                        Sub::Real(real) => *src = real,
                        Sub::Unmapped(role) => {
                            dead_sensors.insert(id.to_owned());
                            dead_why.push(role);
                        }
                    }
                }
            }
        }
        if dead_sensors.len() == before {
            break;
        }
    }
    let sensor_dead = |id: &str| dead_sensors.contains(id);

    // Curves: dead if their source is an unmapped role or dead sensor, or they build on a dead curve.
    let mut dead_curves: HashSet<String> = HashSet::new();
    loop {
        let before = dead_curves.len();
        for c in &mut out.curves {
            let id = c.id().to_owned();
            match c {
                Curve::Linear { source, .. }
                | Curve::Graph { source, .. }
                | Curve::Trigger { source, .. } => {
                    if sensor_dead(source) {
                        dead_curves.insert(id);
                    } else if let Some(sub) = sub(source) {
                        match sub {
                            Sub::Real(real) => *source = real,
                            Sub::Unmapped(role) => {
                                dead_curves.insert(id);
                                skipped.push(format!(
                                    "profile '{name}': sensor role '{role}' is not mapped, so \
                                     curves and fans that read it are skipped"
                                ));
                            }
                        }
                    }
                }
                Curve::Mix { curves, .. } if curves.iter().any(|x| dead_curves.contains(x)) => {
                    dead_curves.insert(id);
                }
                Curve::Sync { target, .. } if dead_curves.contains(target.as_str()) => {
                    dead_curves.insert(id);
                }
                _ => {}
            }
        }
        if dead_curves.len() == before {
            break;
        }
    }
    for role in dead_why {
        skipped.push(format!(
            "profile '{name}': sensor role '{role}' is not mapped, so custom sensors built on it \
             are skipped"
        ));
    }

    let mut seen = HashSet::new();
    let mut controls = Vec::new();
    for mut c in std::mem::take(&mut out.controls) {
        if let Some(sub) = sub(&c.fan) {
            match sub {
                Sub::Real(real) => c.fan = real,
                Sub::Unmapped(role) => {
                    skipped.push(format!(
                        "profile '{name}': fan role '{role}' is not mapped, so its control is skipped"
                    ));
                    continue;
                }
            }
        }
        if dead_curves.contains(&c.curve) {
            skipped.push(format!(
                "profile '{name}': the control for '{}' is skipped, its curve reads a sensor that is not mapped",
                c.fan
            ));
            continue;
        }
        if !seen.insert(c.fan.clone()) {
            skipped.push(format!(
                "profile '{name}': a second control for '{}' is skipped",
                c.fan
            ));
            continue;
        }
        controls.push(c);
    }
    out.controls = controls;

    let mut alerts = Vec::new();
    for mut a in std::mem::take(&mut out.alerts) {
        if sensor_dead(&a.sensor) {
            skipped.push(format!(
                "profile '{name}': the alert on '{}' is skipped, it reads a sensor that is not mapped",
                a.sensor
            ));
            continue;
        }
        if let Some(sub) = sub(&a.sensor) {
            match sub {
                Sub::Real(real) => a.sensor = real,
                Sub::Unmapped(role) => {
                    skipped.push(format!(
                        "profile '{name}': the alert on sensor role '{role}' is skipped, it is not mapped"
                    ));
                    continue;
                }
            }
        }
        alerts.push(a);
    }
    out.alerts = alerts;

    let mut once = HashSet::new();
    skipped.retain(|s| once.insert(s.clone()));
    Resolved {
        profile: out,
        skipped,
    }
}

/// Fans a profile controls that no backend lists, in profile order.
pub fn missing_fans(p: &Profile, known: &[String]) -> Vec<String> {
    p.controls
        .iter()
        .filter(|c| !known.contains(&c.fan))
        .map(|c| c.fan.clone())
        .collect()
}

/// The error text for a profile that names fans no backend lists.
pub fn missing_message(profile: &str, fans: &[String]) -> String {
    format!(
        "profile '{profile}' names fan{} no backend lists: {}",
        if fans.len() == 1 { "" } else { "s" },
        fans.iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryFan {
    pub id: String,
    pub label: String,
    pub kind: FanKind,
}

/// What `inventory.json` holds: the board and every fan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub board: Option<String>,
    pub fans: Vec<InventoryFan>,
}

impl Inventory {
    pub fn of(snap: &HardwareSnapshot, board: Option<String>) -> Self {
        let mut fans: Vec<_> = snap
            .fans
            .iter()
            .map(|f| InventoryFan {
                id: f.id.clone(),
                label: f.label.clone(),
                kind: f.kind,
            })
            .collect();
        fans.sort_by(|a, b| a.id.cmp(&b.id));
        Self { board, fans }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InventoryDiff {
    pub new: Vec<String>,
    pub missing: Vec<String>,
    /// Same id, different label or kind.
    pub changed: Vec<String>,
    /// `(saved, now)` when the board is not the one saved.
    pub board_changed: Option<(Option<String>, Option<String>)>,
}

impl InventoryDiff {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty()
            && self.missing.is_empty()
            && self.changed.is_empty()
            && self.board_changed.is_none()
    }

    /// One line per difference, for the log and `degraded_reasons`.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((was, now)) = &self.board_changed {
            let show = |b: &Option<String>| b.clone().unwrap_or_else(|| "none".into());
            out.push(format!(
                "inventory: the board is now {} (saved: {})",
                show(now),
                show(was)
            ));
        }
        out.extend(
            self.missing
                .iter()
                .map(|f| format!("inventory: fan '{f}' is missing")),
        );
        out.extend(
            self.changed
                .iter()
                .map(|f| format!("inventory: fan '{f}' changed")),
        );
        out.extend(
            self.new
                .iter()
                .map(|f| format!("inventory: fan '{f}' is new")),
        );
        out
    }
}

pub fn diff(saved: &Inventory, now: &Inventory) -> InventoryDiff {
    let mut d = InventoryDiff::default();
    // An unknown board now is a gap in what we can see, not a different board: only two known,
    // different names count.
    if let (Some(a), Some(b)) = (&saved.board, &now.board) {
        if a != b {
            d.board_changed = Some((saved.board.clone(), now.board.clone()));
        }
    }
    for f in &now.fans {
        match saved.fans.iter().find(|s| s.id == f.id) {
            None => d.new.push(f.id.clone()),
            Some(s) if s != f => d.changed.push(f.id.clone()),
            Some(_) => {}
        }
    }
    d.missing = saved
        .fans
        .iter()
        .filter(|s| !now.fans.iter().any(|f| f.id == s.id))
        .map(|s| s.id.clone())
        .collect();
    d
}

/// A role the first-run step is sure of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub role: &'static str,
    pub id: String,
    pub why: String,
}

/// Roles that can be set without asking: the NVIDIA fan, a pump by kind or name, `CPU` in a
/// header's label, and the one CPU and GPU sensor. A role two fans both fit is left out, so the
/// user is asked.
pub fn propose(snap: &HardwareSnapshot, controllable: &[String]) -> Vec<Proposal> {
    let mut by_role: BTreeMap<&'static str, Vec<Proposal>> = BTreeMap::new();
    for f in snap.fans.iter().filter(|f| controllable.contains(&f.id)) {
        let label = f.label.to_lowercase();
        let (role, why) = if f.id.starts_with("nvml_") {
            ("gpu_fan", "an NVIDIA GPU fan")
        } else if f.kind == FanKind::Pump || label.contains("pump") {
            ("pump", "a pump by its kind or label")
        } else if label
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == "cpu")
        {
            ("cpu_fan", "its label says CPU")
        } else {
            continue;
        };
        by_role.entry(role).or_default().push(Proposal {
            role,
            id: f.id.clone(),
            why: format!("{why}: '{}'", f.label),
        });
    }
    for (role, kind) in [("cpu_temp", SensorKind::Cpu), ("gpu_temp", SensorKind::Gpu)] {
        let of_kind: Vec<_> = snap.sensors.iter().filter(|s| s.kind == kind).collect();
        // Several sources read the same chip, so the first prefix with exactly one wins.
        let pick = ["nvml_", "lhm_", "hwinfo_"].iter().find_map(|p| {
            let mut it = of_kind.iter().filter(|s| s.id.starts_with(p));
            match (it.next(), it.next()) {
                (Some(s), None) => Some(*s),
                _ => None,
            }
        });
        if let Some(s) = pick {
            by_role.entry(role).or_default().push(Proposal {
                role,
                id: s.id.clone(),
                why: format!("the one {kind:?} sensor: '{}'", s.label),
            });
        }
    }
    by_role
        .into_values()
        .filter_map(|mut v| (v.len() == 1).then(|| v.remove(0)))
        .collect()
}

/// What a probe saw: which tach rose while one fan ran at 100%.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", content = "fan", rename_all = "snake_case")]
pub enum Pairing {
    /// The probed fan's own tach rose.
    Own,
    /// Another fan's tach rose more. Reported, never guessed.
    Other(String),
    /// No tach rose: the fan has none, is unplugged, or was already at full speed.
    None,
}

/// `before` and `during` are `(fan id, rpm)` for every fan.
pub fn pair(probed: &str, before: &[(String, u32)], during: &[(String, u32)]) -> Pairing {
    let rise = |id: &str| {
        let was = before.iter().find(|(i, _)| i == id).map_or(0, |(_, r)| *r);
        let now = during.iter().find(|(i, _)| i == id).map_or(0, |(_, r)| *r);
        now.saturating_sub(was)
    };
    let best = during
        .iter()
        .map(|(id, _)| (id, rise(id)))
        .filter(|(_, r)| *r >= RPM_RISE)
        .max_by_key(|(id, r)| (*r, *id == probed));
    match best {
        None => Pairing::None,
        Some((id, _)) if id == probed => Pairing::Own,
        Some((id, _)) => Pairing::Other(id.clone()),
    }
}

/// The user's answer to "which fan was that?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Heard {
    Front,
    Rear,
    Top,
    None,
}

impl Heard {
    pub fn role(self) -> Option<&'static str> {
        match self {
            Self::Front => Some("case_front"),
            Self::Rear => Some("case_rear"),
            Self::Top => Some("case_top"),
            Self::None => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{FanReading, SensorReading};

    fn roles(pairs: &[(&str, &str)]) -> Roles {
        let mut r = Roles::default();
        for (role, id) in pairs {
            r.assign(role, id).unwrap();
        }
        r
    }

    fn profile(extra: &str) -> Profile {
        Profile::from_toml(&format!(
            "name = \"p\"\n\
             [[curves]]\nid = \"cpu\"\ntype = \"linear\"\nsource = \"cpu_temp\"\nmin_temp = 40\nmax_temp = 80\nmin_pwm = 20\nmax_pwm = 100\n\
             [[curves]]\nid = \"gpu\"\ntype = \"linear\"\nsource = \"gpu_temp\"\nmin_temp = 40\nmax_temp = 80\nmin_pwm = 20\nmax_pwm = 100\n\
             [[controls]]\nfan = \"case_front\"\ncurve = \"cpu\"\n\
             [[controls]]\nfan = \"gpu_fan\"\ncurve = \"gpu\"\n\
             [[alerts]]\nsensor = \"cpu_temp\"\nthreshold = 85\nseverity = \"warn\"\naction = {{ type = \"notify\" }}\n{extra}"
        ))
        .unwrap()
    }

    fn all_mapped() -> Roles {
        roles(&[
            ("case_front", "lhm_nct6687d_0_fan_1"),
            ("gpu_fan", "nvml_GPU-a_fan0"),
            ("cpu_temp", "lhm_cpu"),
            ("gpu_temp", "nvml_GPU-a_temp"),
        ])
    }

    #[test]
    fn mapped_roles_become_real_ids_everywhere() {
        let r = resolve(&profile(""), &all_mapped());
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        let fans: Vec<_> = r.profile.controls.iter().map(|c| c.fan.as_str()).collect();
        assert_eq!(fans, ["lhm_nct6687d_0_fan_1", "nvml_GPU-a_fan0"]);
        assert_eq!(r.profile.alerts[0].sensor, "lhm_cpu");
        let Curve::Linear { source, .. } = &r.profile.curves[1] else {
            panic!()
        };
        assert_eq!(source, "nvml_GPU-a_temp");
    }

    #[test]
    fn every_bundled_profile_uses_roles_only() {
        let mut full = Roles::default();
        for r in FAN_ROLES.iter().chain(&SENSOR_ROLES) {
            full.assign(r, &format!("real_{r}")).unwrap();
        }
        for (name, text) in crate::bundled_profiles() {
            let r = resolve(&Profile::from_toml(text).unwrap(), &full);
            assert!(r.skipped.is_empty(), "{name}: {:?}", r.skipped);
            assert!(!r.profile.controls.is_empty(), "{name}");
            let real = |id: &str| id.starts_with("real_");
            assert!(r.profile.controls.iter().all(|c| real(&c.fan)), "{name}");
            assert!(r.profile.alerts.iter().all(|a| real(&a.sensor)), "{name}");
            for c in &r.profile.curves {
                if let Curve::Linear { source, .. }
                | Curve::Graph { source, .. }
                | Curve::Trigger { source, .. } = c
                {
                    assert!(real(source), "{name}: {source}");
                }
            }
        }
    }

    #[test]
    fn an_unmapped_fan_role_drops_only_its_control_and_says_so() {
        let mut m = all_mapped();
        m.roles.remove("case_front");
        let r = resolve(&profile(""), &m);
        assert_eq!(r.profile.controls.len(), 1);
        assert_eq!(r.skipped.len(), 1);
        assert!(r.skipped[0].contains("'case_front'") && r.skipped[0].contains("profile 'p'"));
    }

    #[test]
    fn an_unmapped_sensor_role_kills_its_curve_controls_and_alerts() {
        let mut m = all_mapped();
        m.roles.remove("cpu_temp");
        let r = resolve(&profile(""), &m);
        let fans: Vec<_> = r.profile.controls.iter().map(|c| c.fan.as_str()).collect();
        assert_eq!(fans, ["nvml_GPU-a_fan0"]);
        assert!(r.profile.alerts.is_empty());
        let all = r.skipped.join("\n");
        assert!(
            all.contains("'cpu_temp'") && all.contains("'lhm_nct6687d_0_fan_1'"),
            "{all}"
        );
    }

    #[test]
    fn a_dead_sensor_kills_what_is_built_on_it_through_custom_sensors_and_mixes() {
        let p = Profile::from_toml(
            "name = \"p\"\n\
             [[sensors]]\nid = \"avg\"\ntype = \"time_average\"\nsource = \"cpu_temp\"\ntime_s = 5\n\
             [[curves]]\nid = \"a\"\ntype = \"linear\"\nsource = \"avg\"\nmin_temp = 40\nmax_temp = 80\nmin_pwm = 20\nmax_pwm = 100\n\
             [[curves]]\nid = \"m\"\ntype = \"mix\"\nfunction = \"max\"\ncurves = [\"a\"]\n\
             [[curves]]\nid = \"s\"\ntype = \"sync\"\ntarget = \"m\"\n\
             [[controls]]\nfan = \"case_front\"\ncurve = \"s\"\n",
        )
        .unwrap();
        let r = resolve(&p, &roles(&[("case_front", "f1")]));
        assert!(r.profile.controls.is_empty());
        assert!(!r.skipped.is_empty());
    }

    #[test]
    fn ids_that_are_not_roles_pass_through_so_a_hand_written_profile_still_works() {
        let p = Profile::from_toml(
            "name = \"p\"\n[[curves]]\nid = \"c\"\ntype = \"flat\"\npwm = 40\n[[controls]]\nfan = \"nvml_GPU-a_fan0\"\ncurve = \"c\"\n",
        )
        .unwrap();
        let r = resolve(&p, &Roles::default());
        assert_eq!(r.profile, p);
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn skipped_lines_are_not_repeated() {
        let p = profile("[[alerts]]\nsensor = \"cpu_temp\"\nthreshold = 92\nseverity = \"crit\"\naction = { type = \"log\" }\n");
        let r = resolve(&p, &Roles::default());
        let mut sorted = r.skipped.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), r.skipped.len());
    }

    #[test]
    fn roles_toml_round_trips_and_refuses_bad_input() {
        let m = all_mapped();
        assert_eq!(Roles::from_toml(&m.to_toml()).unwrap(), m);
        for bad in [
            "[roles]\nfront = \"x\"\n",
            "[roles]\ncpu_fan = \"\"\n",
            "[roles]\ncpu_fan = \"x\"\npump = \"x\"\n",
            "bogus = 1\n",
        ] {
            assert!(Roles::from_toml(bad).is_err(), "{bad}");
        }
        assert_eq!(Roles::from_toml("").unwrap(), Roles::default());
    }

    #[test]
    fn assigning_an_id_takes_it_from_its_old_role_and_rejects_unknown_roles() {
        let mut m = roles(&[("case_front", "f1")]);
        m.assign("case_rear", "f1").unwrap();
        assert_eq!(m.get("case_front"), None);
        assert_eq!(m.get("case_rear"), Some("f1"));
        assert!(m.assign("middle", "f2").is_err());
        assert!(m.unmapped().contains(&"case_front") && !m.unmapped().contains(&"case_rear"));
    }

    #[test]
    fn a_fan_no_backend_lists_is_named() {
        let known = vec!["a".to_owned()];
        let p = Profile::from_toml(
            "name = \"p\"\n[[curves]]\nid = \"c\"\ntype = \"flat\"\npwm = 40\n[[controls]]\nfan = \"a\"\ncurve = \"c\"\n[[controls]]\nfan = \"b\"\ncurve = \"c\"\n",
        )
        .unwrap();
        let missing = missing_fans(&p, &known);
        assert_eq!(missing, ["b"]);
        let msg = missing_message("p", &missing);
        assert!(msg.contains("'b'") && msg.contains("profile 'p'"), "{msg}");
    }

    fn fan(id: &str, label: &str, kind: FanKind) -> FanReading {
        FanReading {
            id: id.into(),
            label: label.into(),
            rpm: 800,
            duty_percent: 40,
            kind,
        }
    }

    fn inv(board: Option<&str>, fans: &[(&str, &str)]) -> Inventory {
        let snap = HardwareSnapshot {
            fans: fans.iter().map(|(i, l)| fan(i, l, FanKind::Fan)).collect(),
            ..Default::default()
        };
        Inventory::of(&snap, board.map(Into::into))
    }

    #[test]
    fn the_inventory_diff_finds_new_missing_and_changed_fans_and_a_new_board() {
        let saved = inv(
            Some("B1"),
            &[("a", "CPU Fan"), ("b", "Sys 1"), ("c", "Sys 2")],
        );
        let now = inv(
            Some("B2"),
            &[("a", "CPU Fan"), ("b", "System 1"), ("d", "Sys 3")],
        );
        let d = diff(&saved, &now);
        assert_eq!(d.new, ["d"]);
        assert_eq!(d.missing, ["c"]);
        assert_eq!(d.changed, ["b"]);
        assert!(d.board_changed.is_some());
        let lines = d.lines().join("\n");
        for want in ["'c' is missing", "'b' changed", "'d' is new", "B2"] {
            assert!(lines.contains(want), "{lines}");
        }
        assert!(diff(&saved, &saved).is_empty());
    }

    #[test]
    fn an_unknown_board_is_not_a_changed_board() {
        let saved = inv(Some("B1"), &[]);
        assert!(diff(&saved, &inv(None, &[])).is_empty());
        assert!(diff(&inv(None, &[]), &inv(Some("B1"), &[])).is_empty());
    }

    #[test]
    fn a_kind_change_counts_as_changed() {
        let mut now = inv(None, &[("a", "Fan")]);
        now.fans[0].kind = FanKind::Pump;
        assert_eq!(diff(&inv(None, &[("a", "Fan")]), &now).changed, ["a"]);
    }

    fn sensor(id: &str, kind: SensorKind) -> SensorReading {
        SensorReading {
            id: id.into(),
            label: id.into(),
            kind,
            temp_celsius: 40.0,
        }
    }

    fn snap() -> HardwareSnapshot {
        HardwareSnapshot {
            fans: vec![
                fan("nvml_GPU-a_fan0", "GPU 0 fan 0", FanKind::Fan),
                fan("lhm_nct6687d_0_fan_0", "CPU Fan", FanKind::Fan),
                fan("lhm_nct6687d_0_fan_1", "Pump Fan", FanKind::Fan),
                fan("lhm_nct6687d_0_fan_2", "System Fan #1", FanKind::Fan),
                fan("lhm_nct6687d_0_fan_3", "System Fan #2", FanKind::Fan),
            ],
            sensors: vec![
                sensor("nvml_GPU-a_temp", SensorKind::Gpu),
                sensor("hwinfo_gpu_temp", SensorKind::Gpu),
                sensor("lhm_cpu", SensorKind::Cpu),
                sensor("hwinfo_cpu", SensorKind::Cpu),
                sensor("lhm_mb", SensorKind::Other),
            ],
            metrics: vec![],
        }
    }

    fn all_fans(s: &HardwareSnapshot) -> Vec<String> {
        s.fans.iter().map(|f| f.id.clone()).collect()
    }

    #[test]
    fn the_first_run_proposes_what_it_is_sure_of_and_leaves_system_fans_to_the_user() {
        let s = snap();
        let got: Vec<_> = propose(&s, &all_fans(&s))
            .into_iter()
            .map(|p| (p.role, p.id))
            .collect();
        assert_eq!(
            got,
            [
                ("cpu_fan", "lhm_nct6687d_0_fan_0".to_owned()),
                ("cpu_temp", "lhm_cpu".into()),
                ("gpu_fan", "nvml_GPU-a_fan0".into()),
                ("gpu_temp", "nvml_GPU-a_temp".into()),
                ("pump", "lhm_nct6687d_0_fan_1".into()),
            ]
        );
    }

    #[test]
    fn a_role_two_fans_fit_is_not_proposed_and_a_read_only_fan_is_never_proposed() {
        let mut s = snap();
        s.fans.push(fan("lhm_x_fan_9", "CPU_OPT", FanKind::Fan));
        let all = all_fans(&s);
        assert!(!propose(&s, &all).iter().any(|p| p.role == "cpu_fan"));
        let only_gpu = vec!["nvml_GPU-a_fan0".to_owned()];
        let roles: Vec<_> = propose(&s, &only_gpu).into_iter().map(|p| p.role).collect();
        assert!(!roles.contains(&"pump") && roles.contains(&"gpu_fan"));
    }

    fn rpms(v: &[(&str, u32)]) -> Vec<(String, u32)> {
        v.iter().map(|(i, r)| ((*i).into(), *r)).collect()
    }

    #[test]
    fn rpm_pairing_follows_the_tach_that_rose() {
        let before = rpms(&[("a", 600), ("b", 700)]);
        assert_eq!(
            pair("a", &before, &rpms(&[("a", 1400), ("b", 700)])),
            Pairing::Own
        );
        assert_eq!(
            pair("a", &before, &rpms(&[("a", 600), ("b", 1500)])),
            Pairing::Other("b".into())
        );
        assert_eq!(
            pair("a", &before, &rpms(&[("a", 700), ("b", 720)])),
            Pairing::None
        );
        // Already at full speed, or no tach at all: nothing to see.
        assert_eq!(pair("a", &before, &before), Pairing::None);
    }

    #[test]
    fn listen_answers_map_to_case_roles_and_none_maps_to_nothing() {
        assert_eq!(Heard::Front.role(), Some("case_front"));
        assert_eq!(Heard::Rear.role(), Some("case_rear"));
        assert_eq!(Heard::Top.role(), Some("case_top"));
        assert_eq!(Heard::None.role(), None);
    }
}
