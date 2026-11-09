//! Fan and sensor identity (SPEC §5.2): the saved inventory, role mapping, and the checks that
//! name a missing fan. Pure logic; the daemon does the file IO.

use crate::{
    invalid, Control, Curve, CustomSensor, Error, FanKind, HardwareSnapshot, Profile, SensorKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The one role that names a group of fans (SPEC §5.2).
pub const CASE_FANS: &str = "case_fans";
/// Floor for every `case_fans` member. A case-fan header can have an AIO pump plugged into it.
pub const CASE_FANS_MIN_PWM: u8 = 30;

pub const FAN_ROLES: [&str; 4] = ["cpu_fan", "pump", "gpu_fan", CASE_FANS];
pub const SENSOR_ROLES: [&str; 2] = ["cpu_temp", "gpu_temp"];

/// A tach reading must rise at least this much for the probe to call a fan "moved".
pub const RPM_RISE: u32 = 150;
/// ... and at least this percent of the RPM it had before the hold.
pub const RPM_RISE_PCT: u64 = 15;

fn is_role(id: &str) -> bool {
    FAN_ROLES.contains(&id) || SENSOR_ROLES.contains(&id)
}

/// What a role maps to: one id, or for `case_fans` a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Role {
    One(String),
    Many(Vec<String>),
}

impl Role {
    fn ids(&self) -> &[String] {
        match self {
            Self::One(id) => std::slice::from_ref(id),
            Self::Many(ids) => ids,
        }
    }
}

/// `roles.toml`: role -> the real fan or sensor id(s) on this PC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    #[serde(default)]
    roles: BTreeMap<String, Role>,
}

impl Roles {
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let r: Self = toml::from_str(s)?;
        let mut used = HashSet::new();
        for (role, v) in &r.roles {
            if !is_role(role) {
                return invalid(format!("`{role}` is not a role (known: {})", known_roles()));
            }
            match (role == CASE_FANS, v) {
                (true, Role::One(_)) => {
                    return invalid(format!("role `{role}` takes a list of ids"))
                }
                (false, Role::Many(_)) => return invalid(format!("role `{role}` takes one id")),
                _ => {}
            }
            for id in v.ids() {
                if id.is_empty() {
                    return invalid(format!("role `{role}` maps to an empty id"));
                }
                if !used.insert(id.as_str()) {
                    return invalid(format!("`{id}` is mapped to two roles"));
                }
            }
        }
        Ok(r)
    }

    pub fn to_toml(&self) -> String {
        // A map of strings and lists of strings always serialises.
        toml::to_string(self).unwrap_or_default()
    }

    /// The id a single-id role maps to. `case_fans` is read with `members`.
    pub fn get(&self, role: &str) -> Option<&str> {
        match self.roles.get(role) {
            Some(Role::One(id)) => Some(id),
            _ => None,
        }
    }

    /// The fans in `case_fans`, empty when it is unmapped.
    pub fn members(&self) -> &[String] {
        match self.roles.get(CASE_FANS) {
            Some(Role::Many(ids)) => ids,
            _ => &[],
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Role)> {
        self.roles.iter().map(|(r, v)| (r.as_str(), v))
    }

    /// Maps `role` to `id`, taking `id` away from any other role. For `case_fans` it adds a member.
    pub fn assign(&mut self, role: &str, id: &str) -> Result<(), Error> {
        if !is_role(role) || id.is_empty() {
            return invalid(format!("cannot map `{role}` to `{id}`"));
        }
        self.roles.retain(|_, v| match v {
            Role::One(i) => i != id,
            Role::Many(ids) => {
                ids.retain(|i| i != id);
                !ids.is_empty()
            }
        });
        if role == CASE_FANS {
            match self
                .roles
                .entry(role.to_owned())
                .or_insert_with(|| Role::Many(Vec::new()))
            {
                Role::Many(ids) => ids.push(id.to_owned()),
                Role::One(_) => {}
            }
        } else {
            self.roles.insert(role.to_owned(), Role::One(id.to_owned()));
        }
        Ok(())
    }

    /// Roles no one has mapped yet, in a fixed order. An empty group is unmapped.
    pub fn unmapped(&self) -> Vec<&'static str> {
        FAN_ROLES
            .iter()
            .chain(&SENSOR_ROLES)
            .copied()
            .filter(|r| self.roles.get(*r).is_none_or(|v| v.ids().is_empty()))
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

/// The config's alerts with a sensor role replaced by its real id. An alert on a role nothing is
/// mapped to is dropped and named in the second list (SPEC §4.2); any other sensor id is kept.
pub fn resolve_alerts(
    alerts: &[crate::AlertEntry],
    roles: &Roles,
) -> (Vec<crate::AlertEntry>, Vec<String>) {
    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for a in alerts {
        if !SENSOR_ROLES.contains(&a.sensor.as_str()) {
            kept.push(a.clone());
        } else if let Some(id) = roles.get(&a.sensor) {
            kept.push(crate::AlertEntry {
                sensor: id.to_owned(),
                ..a.clone()
            });
        } else {
            skipped.push(format!(
                "alert '{}': role '{}' is not mapped; the alert is skipped",
                a.id, a.sensor
            ));
        }
    }
    (kept, skipped)
}

/// A profile with its roles replaced by real ids, and what had to be left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub profile: Profile,
    /// Roles the profile uses that nothing is mapped to, sorted. Report them with `unmapped_lines`.
    pub unmapped: Vec<String>,
    /// Other things left out (a second control for the same fan). Empty when nothing was.
    pub skipped: Vec<String>,
}

enum Sub {
    Real(String),
    Unmapped(String),
}

/// One line per unmapped role, listing the profiles that use it: `(profile, its unmapped roles)`.
pub fn unmapped_lines(per_profile: &[(String, Vec<String>)]) -> Vec<String> {
    let mut by_role: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (profile, roles) in per_profile {
        for r in roles {
            by_role.entry(r).or_default().push(profile);
        }
    }
    by_role
        .into_iter()
        .map(|(role, profiles)| {
            let what = if SENSOR_ROLES.contains(&role) {
                "the curves and controls that read it are skipped"
            } else {
                "its controls are skipped and the fans stay with the BIOS or driver"
            };
            format!(
                "role '{role}' is not mapped, so {what} (profiles: {}). Run tq identify",
                profiles.join(", ")
            )
        })
        .collect()
}

/// Replaces each role id in `p` with its mapped id. An unmapped role is skipped and reported
/// (SPEC §5.2): a control on it is dropped, a curve on an unmapped sensor is dead and so is
/// every control, curve, custom sensor and alert built on it. A control on `case_fans` becomes
/// one control per member, each floored at `CASE_FANS_MIN_PWM`.
pub fn resolve(p: &Profile, roles: &Roles) -> Resolved {
    let mut out = p.clone();
    let mut skipped = Vec::new();
    let mut unmapped: BTreeSet<String> = BTreeSet::new();
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
                            unmapped.insert(role);
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
                                unmapped.insert(role);
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

    let mut seen = HashSet::new();
    let mut controls = Vec::new();
    for c in std::mem::take(&mut out.controls) {
        if c.fan == CASE_FANS && (c.start_percent.is_some() || c.stop_percent.is_some()) {
            // 0-RPM beats `min_pwm` in the engine, and a member may be a pump.
            skipped.push(format!(
                "profile '{name}': 0-RPM is not allowed on case_fans (a member may be a pump); ignored"
            ));
        }
        let members: Vec<Control> = if c.fan == CASE_FANS {
            roles
                .members()
                .iter()
                .map(|m| Control {
                    fan: m.clone(),
                    min_pwm: c.min_pwm.max(CASE_FANS_MIN_PWM),
                    start_percent: None,
                    stop_percent: None,
                    ..c.clone()
                })
                .collect()
        } else {
            match sub(&c.fan) {
                Some(Sub::Real(real)) => vec![Control {
                    fan: real,
                    ..c.clone()
                }],
                Some(Sub::Unmapped(_)) => Vec::new(),
                None => vec![c.clone()],
            }
        };
        if members.is_empty() {
            unmapped.insert(if c.fan == CASE_FANS {
                CASE_FANS.into()
            } else {
                c.fan
            });
            continue;
        }
        if dead_curves.contains(&c.curve) {
            continue; // the dead sensor's role is already in `unmapped`
        }
        for m in members {
            if seen.insert(m.fan.clone()) {
                controls.push(m);
            } else {
                skipped.push(format!(
                    "profile '{name}': a second control for '{}' is skipped",
                    m.fan
                ));
            }
        }
    }
    out.controls = controls;

    let mut once = HashSet::new();
    skipped.retain(|s| once.insert(s.clone()));
    Resolved {
        profile: out,
        unmapped: unmapped.into_iter().collect(),
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
    /// A probe found that the chip ignores writes to this header. Absent in older files: false.
    #[serde(default)]
    pub rejected_writes: bool,
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
                rejected_writes: false,
            })
            .collect();
        fans.sort_by(|a, b| a.id.cmp(&b.id));
        Self { board, fans }
    }

    /// Accepting today's fans must not forget which headers rejected writes.
    pub fn keep_rejected_from(&mut self, saved: &Inventory) {
        for f in &mut self.fans {
            f.rejected_writes = saved.fans.iter().any(|s| s.id == f.id && s.rejected_writes);
        }
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
            Some(s) if (&s.label, s.kind) != (&f.label, f.kind) => d.changed.push(f.id.clone()),
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
/// header's label, and the one CPU and GPU sensor. A single-id role two fans both fit is left out,
/// so the user is asked. Every other controllable fan of kind `fan` goes in `case_fans`, with or
/// without a tach; one that looks like the CPU fan or a pump but was left out is not swept in.
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
        } else if f.kind == FanKind::Fan {
            (CASE_FANS, "a fan header that is not the CPU fan or a pump")
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
        .into_iter()
        .flat_map(|(role, mut v)| match role {
            CASE_FANS => v,
            _ if v.len() == 1 => vec![v.remove(0)],
            _ => Vec::new(),
        })
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
    let was = |id: &str| before.iter().find(|(i, _)| i == id).map_or(0, |(_, r)| *r);
    let best = during
        .iter()
        .map(|(id, _)| (id, rise(id)))
        // At least RPM_RISE and RPM_RISE_PCT of where the fan was: drift is not a pairing.
        .filter(|(id, r)| {
            *r >= RPM_RISE && u64::from(*r) * 100 >= u64::from(was(id)) * RPM_RISE_PCT
        })
        .max_by_key(|(id, r)| (*r, *id == probed));
    match best {
        None => Pairing::None,
        Some((id, _)) if id == probed => Pairing::Own,
        Some((id, _)) => Pairing::Other(id.clone()),
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
             [[controls]]\nfan = \"cpu_fan\"\ncurve = \"cpu\"\n\
             [[controls]]\nfan = \"gpu_fan\"\ncurve = \"gpu\"\n{extra}"
        ))
        .unwrap()
    }

    fn all_mapped() -> Roles {
        roles(&[
            ("cpu_fan", "lhm_nct6687d_0_fan_1"),
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
        m.roles.remove("cpu_fan");
        let r = resolve(&profile(""), &m);
        assert_eq!(r.profile.controls.len(), 1);
        assert_eq!(r.unmapped, ["cpu_fan"]);
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn an_unmapped_sensor_role_kills_its_curve_and_controls() {
        let mut m = all_mapped();
        m.roles.remove("cpu_temp");
        let r = resolve(&profile(""), &m);
        let fans: Vec<_> = r.profile.controls.iter().map(|c| c.fan.as_str()).collect();
        assert_eq!(fans, ["nvml_GPU-a_fan0"]);
        assert_eq!(r.unmapped, ["cpu_temp"]);
    }

    /// SPEC §4.2: an alert on an unmapped sensor role is dropped and reported; a sensor id that
    /// is not a role passes through.
    #[test]
    fn alerts_resolve_their_sensor_role_and_an_unmapped_one_is_dropped_by_name() {
        let alert = |id: &str, sensor: &str| crate::AlertEntry {
            id: id.into(),
            enabled: true,
            sensor: sensor.into(),
            threshold: 85.0,
            severity: crate::Severity::Warn,
            action: crate::AlertAction::Log,
        };
        let mut m = all_mapped();
        m.roles.remove("gpu_temp");
        let list = [
            alert("a", "cpu_temp"),
            alert("b", "gpu_temp"),
            alert("c", "ssd_1"),
        ];
        let (kept, skipped) = resolve_alerts(&list, &m);
        let got: Vec<_> = kept
            .iter()
            .map(|a| (a.id.as_str(), a.sensor.as_str()))
            .collect();
        assert_eq!(got, [("a", "lhm_cpu"), ("c", "ssd_1")]);
        assert_eq!(skipped.len(), 1);
        assert!(
            skipped[0].contains("'b'") && skipped[0].contains("gpu_temp"),
            "{skipped:?}"
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
             [[controls]]\nfan = \"cpu_fan\"\ncurve = \"s\"\n",
        )
        .unwrap();
        let r = resolve(&p, &roles(&[("cpu_fan", "f1")]));
        assert!(r.profile.controls.is_empty());
        assert_eq!(r.unmapped, ["cpu_temp"]);
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
        let r = resolve(&profile(""), &Roles::default());
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
        let mut m = roles(&[("cpu_fan", "f1")]);
        m.assign("pump", "f1").unwrap();
        assert_eq!(m.get("cpu_fan"), None);
        assert_eq!(m.get("pump"), Some("f1"));
        assert!(m.assign("middle", "f2").is_err());
        assert!(m.unmapped().contains(&"cpu_fan") && !m.unmapped().contains(&"pump"));
        // A group takes members one at a time, and a member leaving it can empty it.
        m.assign("case_fans", "f2").unwrap();
        m.assign("case_fans", "f3").unwrap();
        assert_eq!(m.members(), ["f2", "f3"]);
        m.assign("pump", "f2").unwrap();
        assert_eq!(m.members(), ["f3"]);
        m.assign("cpu_fan", "f3").unwrap();
        assert!(m.members().is_empty() && m.unmapped().contains(&"case_fans"));
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

    /// M16 (c): a header that rejected writes is remembered in the inventory so `tq identify`
    /// does not run it again. It is not a hardware change, an old file reads false, and
    /// accepting today's fans keeps it.
    #[test]
    fn rejected_writes_is_remembered_but_is_never_a_change() {
        let mut saved = inv(None, &[("a", "Fan")]);
        saved.fans[0].rejected_writes = true;
        assert!(diff(&saved, &inv(None, &[("a", "Fan")])).is_empty());

        let mut now = inv(None, &[("a", "Fan"), ("b", "Fan 2")]);
        now.keep_rejected_from(&saved);
        assert!(now.fans[0].rejected_writes && !now.fans[1].rejected_writes);
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
    fn the_first_run_proposes_the_gpu_fan_cpu_fan_pump_sensors_and_the_case_fans() {
        let s = snap();
        let got: Vec<_> = propose(&s, &all_fans(&s))
            .into_iter()
            .map(|p| (p.role, p.id))
            .collect();
        assert_eq!(
            got,
            [
                ("case_fans", "lhm_nct6687d_0_fan_2".to_owned()),
                ("case_fans", "lhm_nct6687d_0_fan_3".into()),
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

    /// M16 (f): a fan that merely drifts is not a pairing. The rise must be at least 150 RPM
    /// and at least 15% of where that fan was.
    #[test]
    fn a_drifting_tach_is_not_named_as_crossed_wiring() {
        let before = rpms(&[("hdr", 0), ("gpu", 2000), ("small", 1000)]);
        // 8% of 2000 is 160 RPM: enough RPM, not enough rise.
        let drift = rpms(&[("hdr", 0), ("gpu", 2160), ("small", 1000)]);
        assert_eq!(pair("hdr", &before, &drift), Pairing::None);
        // 16% of 1000 is 160 RPM: both limits met.
        let rose = rpms(&[("hdr", 0), ("gpu", 2000), ("small", 1160)]);
        assert_eq!(pair("hdr", &before, &rose), Pairing::Other("small".into()));
        // The check's real numbers: 1123 -> 1186.
        let real = rpms(&[("hdr", 0), ("gpu", 1186)]);
        assert_eq!(
            pair("hdr", &rpms(&[("hdr", 0), ("gpu", 1123)]), &real),
            Pairing::None
        );
    }

    // ---- case_fans (design change, 2026-11-06) ----

    fn group(ids: &[&str]) -> Roles {
        let list: Vec<String> = ids.iter().map(|i| format!("\"{i}\"")).collect();
        Roles::from_toml(&format!("[roles]\ncase_fans = [{}]\n", list.join(", "))).unwrap()
    }

    fn case_profile(min_pwm: u8) -> Profile {
        Profile::from_toml(&format!(
            "name = \"p\"\n\
             [[curves]]\nid = \"c\"\ntype = \"flat\"\npwm = 25\n\
             [[controls]]\nfan = \"case_fans\"\ncurve = \"c\"\nstep_down_pct = 7\nmin_pwm = {min_pwm}\n"
        ))
        .unwrap()
    }

    #[test]
    fn roles_toml_takes_case_fans_as_a_list_and_the_other_roles_as_one_id() {
        let r = Roles::from_toml("[roles]\ncase_fans = [\"a\", \"b\"]\ncpu_fan = \"c\"\n").unwrap();
        assert_eq!(Roles::from_toml(&r.to_toml()).unwrap(), r);
        assert_eq!(r.get("cpu_fan"), Some("c"));
        for bad in [
            "[roles]\ncase_fans = \"a\"\n",
            "[roles]\ncpu_fan = [\"a\"]\n",
            "[roles]\ncase_fans = [\"a\", \"a\"]\n",
            "[roles]\ncase_fans = [\"a\", \"\"]\n",
            "[roles]\ncase_fans = [\"a\"]\ncpu_fan = \"a\"\n",
            "[roles]\ncase_front = \"a\"\n",
            "[roles]\ncase_rear = \"a\"\n",
            "[roles]\ncase_top = \"a\"\n",
        ] {
            assert!(Roles::from_toml(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_case_fans_control_becomes_one_control_per_member_floored_at_30() {
        for (asked, floor) in [(0, 30), (20, 30), (30, 30), (45, 45)] {
            let r = resolve(&case_profile(asked), &group(&["a", "b", "c"]));
            assert!(r.skipped.is_empty(), "{:?}", r.skipped);
            let got: Vec<_> = r
                .profile
                .controls
                .iter()
                .map(|c| (c.fan.as_str(), c.curve.as_str(), c.step_down_pct, c.min_pwm))
                .collect();
            assert_eq!(
                got,
                [
                    ("a", "c", 7, floor),
                    ("b", "c", 7, floor),
                    ("c", "c", 7, floor)
                ],
                "min_pwm {asked}"
            );
        }
    }

    #[test]
    fn zero_rpm_on_case_fans_is_stripped_from_every_member_and_reported_once() {
        let p = Profile::from_toml(
            "name = \"p\"\n[[curves]]\nid = \"c\"\ntype = \"flat\"\npwm = 25\n\
             [[controls]]\nfan = \"case_fans\"\ncurve = \"c\"\nstart_percent = 40\nstop_percent = 30\n\
             [[controls]]\nfan = \"cpu_fan\"\ncurve = \"c\"\nstart_percent = 40\nstop_percent = 30\n",
        )
        .unwrap();
        let mut roles = group(&["a", "b"]);
        roles.assign("cpu_fan", "x").unwrap();
        let r = resolve(&p, &roles);
        for c in &r.profile.controls {
            let case = c.fan != "x";
            assert_eq!(c.stop_percent.is_none(), case, "{}", c.fan);
            assert_eq!(c.start_percent.is_none(), case, "{}", c.fan);
        }
        assert_eq!(
            r.skipped,
            ["profile 'p': 0-RPM is not allowed on case_fans (a member may be a pump); ignored"]
        );
        // Without 0-RPM settings there is nothing to report.
        assert!(resolve(&case_profile(0), &roles).skipped.is_empty());
    }

    #[test]
    fn an_empty_or_unmapped_case_fans_group_is_skipped_and_named() {
        for roles in [group(&[]), Roles::default()] {
            let r = resolve(&case_profile(0), &roles);
            assert!(r.profile.controls.is_empty());
            assert_eq!(r.unmapped, ["case_fans"]);
        }
    }

    #[test]
    fn unmapped_roles_are_one_line_each_naming_the_profiles_that_use_them() {
        let per = vec![
            (
                "balanced".to_owned(),
                vec!["case_fans".to_owned(), "cpu_temp".to_owned()],
            ),
            ("gaming".to_owned(), vec!["case_fans".to_owned()]),
        ];
        let lines = unmapped_lines(&per);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("'case_fans'") && lines[0].contains("balanced, gaming"));
        assert!(lines[1].contains("'cpu_temp'") && !lines[1].contains("gaming"));
        assert!(lines.iter().all(|l| l.contains("tq identify")));
        assert!(unmapped_lines(&[]).is_empty());
        assert_eq!(CASE_FANS_MIN_PWM, 30);
    }

    #[test]
    fn the_first_run_proposes_every_other_controllable_fan_as_case_fans() {
        let mut s = snap();
        s.fans[4].rpm = 0; // a header with no tach is still a case fan
        s.fans.push(fan("lhm_x_fan_8", "Aux Pump", FanKind::Pump));
        s.fans
            .push(fan("lhm_x_fan_9", "System Fan #3", FanKind::Fan));
        let mut all = all_fans(&s);
        all.retain(|f| f != "lhm_x_fan_9"); // read-only: never proposed
        let got: Vec<_> = propose(&s, &all)
            .into_iter()
            .filter(|p| p.role == "case_fans")
            .map(|p| p.id)
            .collect();
        assert_eq!(got, ["lhm_nct6687d_0_fan_2", "lhm_nct6687d_0_fan_3"]);
    }

    #[test]
    fn a_fan_that_might_be_the_cpu_fan_or_a_pump_is_never_swept_into_case_fans() {
        let mut s = snap();
        s.fans.push(fan("lhm_x_fan_9", "CPU_OPT", FanKind::Fan)); // two CPU fans: ambiguous
        let all = all_fans(&s);
        let ps = propose(&s, &all);
        assert!(!ps.iter().any(|p| p.role == "cpu_fan"));
        let case: Vec<_> = ps.iter().filter(|p| p.role == "case_fans").collect();
        assert!(
            case.iter()
                .all(|p| p.id != "lhm_x_fan_9" && p.id != "lhm_nct6687d_0_fan_0"),
            "{case:?}"
        );
        assert_eq!(case.len(), 2);
    }

    #[test]
    fn every_bundled_profile_has_one_case_fans_control_and_none_of_the_old_roles() {
        let mut roles = group(&["a", "b"]);
        roles.assign("cpu_temp", "t1").unwrap();
        roles.assign("gpu_temp", "t2").unwrap();
        for (name, text) in crate::bundled_profiles() {
            for old in ["case_front", "case_rear", "case_top"] {
                assert!(!text.contains(old), "{name} still says {old}");
            }
            let p = Profile::from_toml(text).unwrap();
            assert_eq!(
                p.controls.iter().filter(|c| c.fan == "case_fans").count(),
                1,
                "{name}"
            );
            let r = resolve(&p, &roles);
            let case: Vec<_> = r
                .profile
                .controls
                .iter()
                .filter(|c| c.fan == "a" || c.fan == "b")
                .collect();
            assert_eq!(case.len(), 2, "{name}");
            assert!(case.iter().all(|c| c.min_pwm >= 30), "{name}");
        }
    }
}
