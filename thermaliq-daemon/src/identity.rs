//! Fan identity wiring (SPEC §5.2): `roles.toml` and `inventory.json` in the data dir, the
//! start-up checks, and the first-run probe. The rules themselves are in `thermaliq_common::identity`.

use crate::runtime::{Cache, CommandError, Handle};
use crate::store::write_atomic;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use std::time::Duration;
use thermaliq_common::identity::{
    diff, missing_fans, missing_message, pair, propose, resolve, Inventory, InventoryDiff, Roles,
};
use thermaliq_common::{Control, Curve, HardwareSnapshot, Profile};
use thermaliq_hal::HardwareLayer;

/// How long a probe holds a fan at 100% before it reads the tachs.
pub const PROBE_FOR: Duration = Duration::from_secs(5);
const PROBE_CURVE: &str = "identify_probe";

pub struct Identity {
    dir: PathBuf,
    board: Option<String>,
    /// Every fan a backend can write to, as at start (SPEC §5.2).
    controllable: Vec<String>,
    roles: Mutex<Roles>,
    /// Filled by `check_inventory`.
    diff: Mutex<InventoryDiff>,
    /// Problems found at load, already printed: the profile and the role or fan.
    problems: Vec<String>,
    /// Problems the inventory check found.
    late: Mutex<Vec<String>>,
    probing: tokio::sync::Mutex<()>,
    probe_for: Duration,
}

#[derive(Debug)]
pub enum ProbeError {
    UnknownFan(String),
    Busy,
    /// S2 holds every fan at 100% (during or before), so the probe would learn nothing.
    Latched,
    Engine(CommandError),
    /// The profile could not be put back. Said loudly: the probed fan may stay at 100%.
    NotRestored(String),
}

fn read_roles(dir: &Path) -> Result<Roles, String> {
    match fs::read_to_string(dir.join("roles.toml")) {
        Ok(s) => Roles::from_toml(&s).map_err(|e| e.to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Roles::default()),
        Err(e) => Err(e.to_string()),
    }
}

fn read_inventory(dir: &Path) -> Result<Option<Inventory>, String> {
    match fs::read_to_string(dir.join("inventory.json")) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn save_inventory(dir: &Path, inv: &Inventory) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(inv).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("inventory.json"), &json).map_err(|e| format!("inventory.json: {e}"))
}

impl Identity {
    /// No roles, no inventory: what the API tests and an unconfigured state use.
    pub fn empty(dir: PathBuf) -> Self {
        Self {
            dir,
            board: None,
            controllable: Vec::new(),
            roles: Mutex::default(),
            diff: Mutex::default(),
            problems: Vec::new(),
            late: Mutex::default(),
            probing: tokio::sync::Mutex::new(()),
            probe_for: PROBE_FOR,
        }
    }

    pub fn with_probe_time(mut self, d: Duration) -> Self {
        self.probe_for = d;
        self
    }

    /// Reads `roles.toml`, turns each profile's roles into real ids, and checks the inventory.
    /// Everything it finds is printed (it ends up in `service.log`). A bad file costs the
    /// mapping, not the daemon: the other fans and S2–S4 must keep working.
    pub fn load(
        dir: &Path,
        hal: &dyn HardwareLayer,
        profiles: Vec<Profile>,
    ) -> (Self, Vec<Profile>) {
        let mut id = Self::empty(dir.to_owned());
        id.controllable = hal.capabilities().controllable_fans;
        id.board = hal.board();
        let roles = read_roles(dir).unwrap_or_else(|e| {
            id.problems
                .push(format!("roles.toml: {e}; no role is mapped"));
            Roles::default()
        });
        let resolved: Vec<Profile> = profiles
            .iter()
            .map(|p| {
                let r = resolve(p, &roles);
                id.problems.extend(r.skipped);
                let missing = missing_fans(&r.profile, &id.controllable);
                if !missing.is_empty() {
                    id.problems.push(missing_message(&r.profile.name, &missing));
                }
                r.profile
            })
            .collect();
        *id.roles.lock().unwrap_or_else(PoisonError::into_inner) = roles;
        for line in &id.problems {
            eprintln!("thermaliq: {line}");
        }
        (id, resolved)
    }

    /// Compares the first fresh snapshot with `inventory.json`, and saves it if there is none
    /// yet. Not part of `load`: a hung backend must not stop the daemon starting.
    pub fn check_inventory(&self, snap: &HardwareSnapshot) {
        let now = Inventory::of(snap, self.board.clone());
        let mut late = Vec::new();
        let mut found = InventoryDiff::default();
        match read_inventory(&self.dir) {
            Ok(None) if now.fans.is_empty() => {
                late.push(
                    "no fans in the first snapshot; the inventory baseline was not saved".into(),
                );
            }
            Ok(None) => match save_inventory(&self.dir, &now) {
                Ok(()) => eprintln!(
                    "thermaliq: first run: saved the fan inventory ({} fans)",
                    now.fans.len()
                ),
                Err(e) => late.push(format!("cannot save the inventory: {e}")),
            },
            Ok(Some(saved)) => found = diff(&saved, &now),
            Err(e) => late.push(format!("inventory.json: {e}; not checked")),
        }
        for line in found.lines().iter().chain(&late) {
            eprintln!("thermaliq: {line}");
        }
        *self.diff.lock().unwrap_or_else(PoisonError::into_inner) = found;
        *self.late.lock().unwrap_or_else(PoisonError::into_inner) = late;
    }

    /// Runs `check_inventory` on the first fresh snapshot the engine caches. Gives up after a
    /// minute and says so: the check then shows as not done, never as a clean inventory.
    pub fn check_when_ready(self: &std::sync::Arc<Self>, handle: Handle) {
        let me = self.clone();
        std::thread::spawn(move || {
            let end = std::time::Instant::now() + Duration::from_secs(60);
            while std::time::Instant::now() < end {
                let c = handle.cache();
                if c.ticks > 0 && c.snapshot_age == 0 {
                    return me.check_inventory(&c.snapshot);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let why = "inventory not checked: no snapshot within 60 s of the start".to_owned();
            eprintln!("thermaliq: {why}");
            *me.late.lock().unwrap_or_else(PoisonError::into_inner) = vec![why];
        });
    }

    fn roles(&self) -> Roles {
        self.roles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What `GET /identity` answers.
    pub fn describe(&self, snap: &HardwareSnapshot) -> Value {
        let roles = self.roles();
        json!({
            "board": self.board,
            "roles": roles.iter().collect::<BTreeMap<_, _>>(),
            "unmapped_roles": roles.unmapped(),
            "inventory": *self.diff.lock().unwrap_or_else(PoisonError::into_inner),
            "problems": self
                .problems
                .iter()
                .chain(self.late.lock().unwrap_or_else(PoisonError::into_inner).iter())
                .collect::<Vec<_>>(),
            "proposals": propose(snap, &self.controllable),
            "fans": snap.fans.iter().map(|f| json!({
                "id": f.id, "label": f.label, "kind": f.kind, "rpm": f.rpm,
                "controllable": self.controllable.contains(&f.id),
            })).collect::<Vec<_>>(),
            "sensors": snap.sensors.iter()
                .map(|s| json!({"id": s.id, "label": s.label, "kind": s.kind}))
                .collect::<Vec<_>>(),
        })
    }

    /// Replaces `roles.toml`. A fan role must name a fan a backend can write to and a sensor
    /// role a sensor in the latest snapshot. Profiles use it after the next service start.
    pub fn set_roles(
        &self,
        wanted: &BTreeMap<String, String>,
        snap: &HardwareSnapshot,
    ) -> Result<(), String> {
        let mut roles = Roles::default();
        let mut used = std::collections::HashSet::new();
        for (role, id) in wanted {
            if !used.insert(id) {
                return Err(format!("'{id}' is wanted by two roles"));
            }
            let sensor = thermaliq_common::identity::SENSOR_ROLES.contains(&role.as_str());
            let exists = if sensor {
                snap.sensors.iter().any(|s| &s.id == id)
            } else {
                self.controllable.contains(id)
            };
            if !exists {
                return Err(format!(
                    "role '{role}' maps to '{id}', which is not a {} on this PC",
                    if sensor { "sensor" } else { "controllable fan" }
                ));
            }
            roles.assign(role, id).map_err(|e| e.to_string())?;
        }
        write_atomic(&self.dir.join("roles.toml"), roles.to_toml().as_bytes())
            .map_err(|e| format!("roles.toml: {e}"))?;
        *self.roles.lock().unwrap_or_else(PoisonError::into_inner) = roles;
        Ok(())
    }

    /// Makes today's fans the saved inventory. An empty snapshot is refused: saving it would
    /// make every fan look new next time.
    pub fn accept_inventory(&self, snap: &HardwareSnapshot) -> Result<(), String> {
        if snap.fans.is_empty() {
            return Err("the snapshot has no fans; not saving an empty inventory".into());
        }
        save_inventory(&self.dir, &Inventory::of(snap, self.board.clone()))
    }

    /// Holds `fan` at 100% for `probe_for` through the engine, then puts the profile back
    /// (SPEC §5.2). It only raises a fan, so a pump is only ever raised.
    pub async fn probe(
        &self,
        handle: &Handle,
        profiles: &[Profile],
        fan: &str,
    ) -> Result<Value, ProbeError> {
        if !self.controllable.iter().any(|f| f == fan) {
            return Err(ProbeError::UnknownFan(fan.to_owned()));
        }
        let Ok(_one_at_a_time) = self.probing.try_lock() else {
            return Err(ProbeError::Busy);
        };
        let before = handle.cache();
        if before.status.latched {
            return Err(ProbeError::Latched);
        }
        let Some(original) = profiles.iter().find(|p| p.name == before.active_profile) else {
            return Err(ProbeError::Engine(CommandError::Rejected(format!(
                "the active profile '{}' is not a known profile",
                before.active_profile
            ))));
        };
        handle
            .set_profile(with_probe(original, fan))
            .await
            .map_err(ProbeError::Engine)?;
        tokio::time::sleep(self.probe_for).await;
        let during = handle.cache();
        handle.set_profile(original.clone()).await.map_err(|e| {
            ProbeError::NotRestored(format!(
                "could not put profile '{}' back ({e:?}); fan '{fan}' may stay at 100% until you \
                 switch profile",
                original.name
            ))
        })?;
        if during.status.latched {
            return Err(ProbeError::Latched);
        }
        let (was, now) = (rpms(&before), rpms(&during));
        let label = during
            .snapshot
            .fans
            .iter()
            .find(|f| f.id == fan)
            .map(|f| f.label.clone());
        Ok(json!({
            "fan": fan,
            "label": label,
            "rpm_before": was,
            "rpm_after": now,
            "pairing": pair(fan, &was, &now),
        }))
    }
}

fn rpms(c: &Cache) -> Vec<(String, u32)> {
    c.snapshot
        .fans
        .iter()
        .map(|f| (f.id.clone(), f.rpm))
        .collect()
}

/// `p` with `fan` on a flat 100% curve and its step-up at once, in place of any control it had.
fn with_probe(p: &Profile, fan: &str) -> Profile {
    let mut p = p.clone();
    p.controls.retain(|c| c.fan != fan);
    p.curves.retain(|c| c.id() != PROBE_CURVE);
    p.curves.push(Curve::Flat {
        id: PROBE_CURVE.into(),
        pwm: 100,
    });
    p.controls.push(Control {
        fan: fan.to_owned(),
        curve: PROBE_CURVE.into(),
        step_up_pct: 100,
        step_down_pct: 100,
        min_pwm: 0,
        start_percent: None,
        stop_percent: None,
    });
    p
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::history::HistoryDb;
    use crate::runtime::{Daemon, Options};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use thermaliq_common::SensorKind;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    const BAL: &str = "name = \"bal\"\n\
        [[curves]]\nid = \"c\"\ntype = \"linear\"\nsource = \"cpu_temp\"\nmin_temp = 30\nmax_temp = 90\nmin_pwm = 20\nmax_pwm = 100\n\
        [[controls]]\nfan = \"case_front\"\ncurve = \"c\"\n\
        [[controls]]\nfan = \"case_rear\"\ncurve = \"c\"\n\
        [[alerts]]\nsensor = \"cpu_temp\"\nthreshold = 85\nseverity = \"warn\"\naction = { type = \"log\" }\n";

    fn at() -> Tick {
        Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)])
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn hal(fans: &[&str]) -> Arc<MockHardwareLayer> {
        Arc::new(MockHardwareLayer::new(fans, vec![at()]))
    }

    fn bal() -> Vec<Profile> {
        vec![Profile::from_toml(BAL).unwrap()]
    }

    fn fast() -> Options {
        Options {
            tick: Duration::from_millis(5),
            prune_every: Duration::from_secs(3600),
        }
    }

    fn wait_until(mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn roles_in_roles_toml_become_real_ids_at_load() {
        let d = dir();
        fs::write(
            d.path().join("roles.toml"),
            "[roles]\ncase_front = \"f1\"\ncase_rear = \"f2\"\ncpu_temp = \"cpu\"\n",
        )
        .unwrap();
        let (id, profiles) = Identity::load(d.path(), &*hal(&["f1", "f2"]), bal());
        let fans: Vec<_> = profiles[0]
            .controls
            .iter()
            .map(|c| c.fan.as_str())
            .collect();
        assert_eq!(fans, ["f1", "f2"]);
        assert!(id.problems.is_empty(), "{:?}", id.problems);
    }

    #[test]
    fn an_unmapped_role_is_reported_by_name_and_its_control_skipped() {
        let d = dir();
        fs::write(
            d.path().join("roles.toml"),
            "[roles]\ncase_front = \"f1\"\ncpu_temp = \"cpu\"\n",
        )
        .unwrap();
        let (id, profiles) = Identity::load(d.path(), &*hal(&["f1"]), bal());
        assert_eq!(profiles[0].controls.len(), 1);
        assert!(
            id.problems.iter().any(|p| p.contains("'case_rear'")),
            "{:?}",
            id.problems
        );
    }

    #[test]
    fn a_mapped_fan_that_no_backend_lists_is_named_at_load() {
        let d = dir();
        fs::write(
            d.path().join("roles.toml"),
            "[roles]\ncase_front = \"f1\"\ncase_rear = \"gone\"\ncpu_temp = \"cpu\"\n",
        )
        .unwrap();
        let (id, _) = Identity::load(d.path(), &*hal(&["f1"]), bal());
        assert!(
            id.problems
                .iter()
                .any(|p| p.contains("'gone'") && p.contains("profile 'bal'")),
            "{:?}",
            id.problems
        );
    }

    #[test]
    fn a_broken_roles_file_costs_the_mapping_not_the_start() {
        let d = dir();
        fs::write(d.path().join("roles.toml"), "[roles]\nmiddle = \"f1\"\n").unwrap();
        let (id, profiles) = Identity::load(d.path(), &*hal(&["f1"]), bal());
        assert!(profiles[0].controls.is_empty());
        assert!(
            id.problems.iter().any(|p| p.contains("roles.toml")),
            "{:?}",
            id.problems
        );
    }

    /// A start with these fans, including the inventory check.
    fn checked(d: &Path, fans: &[&str]) -> Identity {
        let h = hal(fans);
        let (id, _) = Identity::load(d, &*h, vec![]);
        id.check_inventory(&h.snapshot().unwrap());
        id
    }

    fn found(id: &Identity) -> InventoryDiff {
        id.diff.lock().unwrap().clone()
    }

    #[test]
    fn the_first_start_saves_the_inventory_and_later_starts_report_the_difference() {
        let d = dir();
        assert!(found(&checked(d.path(), &["f1", "f2"])).is_empty());
        assert!(d.path().join("inventory.json").exists());
        let again = found(&checked(d.path(), &["f1", "f3"]));
        assert_eq!(again.missing, ["f2"]);
        assert_eq!(again.new, ["f3"]);
        // Reporting does not rewrite it: a fan that stays missing is reported again.
        assert_eq!(found(&checked(d.path(), &["f1", "f3"])).missing, ["f2"]);
    }

    #[test]
    fn accepting_saves_today_s_fans_and_refuses_an_empty_snapshot() {
        let d = dir();
        checked(d.path(), &["f1", "f2"]);
        let id = checked(d.path(), &["f1"]);
        id.accept_inventory(&hal(&["f1"]).snapshot().unwrap())
            .unwrap();
        assert!(found(&checked(d.path(), &["f1"])).is_empty());
        assert!(id.accept_inventory(&HardwareSnapshot::default()).is_err());
    }

    #[test]
    fn an_empty_first_snapshot_saves_no_baseline_and_says_so() {
        let d = dir();
        let id = checked(d.path(), &[]);
        assert!(!d.path().join("inventory.json").exists());
        assert!(id.late.lock().unwrap()[0].contains("baseline"));
    }

    #[test]
    fn a_corrupt_inventory_is_reported_and_left_alone() {
        let d = dir();
        fs::write(d.path().join("inventory.json"), "{ nope").unwrap();
        let id = checked(d.path(), &["f1"]);
        assert!(id.late.lock().unwrap()[0].contains("inventory.json"));
        assert_eq!(
            fs::read_to_string(d.path().join("inventory.json")).unwrap(),
            "{ nope"
        );
    }

    #[test]
    fn a_start_does_not_wait_on_a_backend_that_never_answers() {
        // `load` only reads what the backends already know; it must return at once.
        struct Wedged;
        impl HardwareLayer for Wedged {
            fn snapshot(&self) -> Result<HardwareSnapshot, thermaliq_hal::HalError> {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), thermaliq_hal::HalError> {
                Ok(())
            }
            fn restore_auto(&self, _: &str) -> Result<(), thermaliq_hal::HalError> {
                Ok(())
            }
            fn capabilities(&self) -> thermaliq_hal::Capabilities {
                thermaliq_hal::Capabilities::default()
            }
        }
        let d = dir();
        let t0 = Instant::now();
        Identity::load(d.path(), &Wedged, bal());
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn set_roles_checks_the_ids_exist_and_writes_a_file_load_reads_back() {
        let d = dir();
        let h = hal(&["f1", "f2"]);
        let (id, _) = Identity::load(d.path(), &*h, bal());
        let snap = h.snapshot().unwrap();
        let want = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(a, b)| ((*a).into(), (*b).into()))
                .collect()
        };
        for bad in [
            want(&[("case_front", "nope")]),
            want(&[("cpu_temp", "f1")]),
            want(&[("middle", "f1")]),
            want(&[("case_front", "f1"), ("case_rear", "f1")]),
        ] {
            assert!(id.set_roles(&bad, &snap).is_err(), "{bad:?}");
        }
        assert!(
            !d.path().join("roles.toml").exists(),
            "a refused set writes nothing"
        );
        id.set_roles(
            &want(&[
                ("case_front", "f1"),
                ("case_rear", "f2"),
                ("cpu_temp", "cpu"),
            ]),
            &snap,
        )
        .unwrap();
        let (_, profiles) = Identity::load(d.path(), &*h, bal());
        assert_eq!(profiles[0].controls.len(), 2);
    }

    #[test]
    fn describe_lists_roles_unmapped_roles_proposals_and_fans() {
        let d = dir();
        let h = hal(&["f1"]);
        let (id, _) = Identity::load(d.path(), &*h, bal());
        let v = id.describe(&h.snapshot().unwrap());
        assert_eq!(v["fans"][0]["id"], "f1");
        assert!(v["unmapped_roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "case_front"));
        assert!(v["proposals"].is_array() && v["roles"].is_object());
    }

    fn running(fans: &[&str]) -> (Arc<MockHardwareLayer>, Daemon, Vec<Profile>) {
        let h = hal(fans);
        let profiles = vec![Profile::from_toml(
            "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap()];
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        let d = Daemon::start(h.clone(), &profiles[0], vec![], db, fast());
        wait_until(|| d.cache().ticks > 2);
        (h, d, profiles)
    }

    fn identity_for(fans: &[&str]) -> Identity {
        let mut id = Identity::empty(PathBuf::new()).with_probe_time(Duration::from_millis(150));
        id.controllable = fans.iter().map(|f| (*f).to_owned()).collect();
        id
    }

    #[tokio::test]
    async fn a_probe_raises_the_fan_pairs_its_tach_and_puts_the_profile_back() {
        let (h, d, profiles) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        let r = id.probe(&d.handle(), &profiles, "f2").await.unwrap();
        assert_eq!(r["pairing"]["result"], "own", "{r}");
        assert!(h.writes().iter().any(|(f, duty)| f == "f2" && *duty == 100));
        assert_eq!(d.cache().active_profile, "flat");
        wait_until(|| h.restores().contains(&"f2".to_owned()));
        // The profile's own fan was never lowered by the probe.
        assert!(h
            .writes()
            .iter()
            .filter(|(f, _)| f == "f1")
            .all(|(_, duty)| *duty == 30));
    }

    #[tokio::test]
    async fn a_probe_of_a_fan_no_backend_lists_is_refused_and_touches_nothing() {
        let (h, d, profiles) = running(&["f1"]);
        let id = identity_for(&["f1"]);
        let before = h.writes().len();
        let err = id.probe(&d.handle(), &profiles, "nope").await.unwrap_err();
        assert!(matches!(err, ProbeError::UnknownFan(f) if f == "nope"));
        assert_eq!(h.writes().len(), before);
    }

    #[tokio::test]
    async fn two_probes_at_once_are_refused_not_stacked() {
        let (_, d, profiles) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        let handle = d.handle();
        let (a, b) = tokio::join!(
            id.probe(&handle, &profiles, "f1"),
            id.probe(&handle, &profiles, "f2")
        );
        assert!(a.is_ok() ^ b.is_ok(), "{a:?} {b:?}");
        assert!(matches!(a.err().or(b.err()), Some(ProbeError::Busy)));
    }

    #[tokio::test]
    async fn a_probe_refuses_to_start_while_the_latch_holds() {
        let h = Arc::new(MockHardwareLayer::new(
            &["f1"],
            vec![Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 97.0)])],
        ));
        let profiles = vec![Profile::from_toml(
            "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\n",
        )
        .unwrap()];
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        let d = Daemon::start(h, &profiles[0], vec![], db, fast());
        wait_until(|| d.cache().status.latched);
        let err = identity_for(&["f1"])
            .probe(&d.handle(), &profiles, "f1")
            .await
            .unwrap_err();
        assert!(matches!(err, ProbeError::Latched), "{err:?}");
    }

    #[test]
    fn the_probe_profile_replaces_the_fans_control_and_nothing_else() {
        let p = Profile::from_toml(
            "name = \"p\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"a\"\ncurve = \"c\"\n[[controls]]\nfan = \"b\"\ncurve = \"c\"\n",
        )
        .unwrap();
        let q = with_probe(&p, "b");
        assert_eq!(q.controls.len(), 2);
        assert_eq!(q.controls[0], p.controls[0]);
        let probe = q.controls.iter().find(|c| c.fan == "b").unwrap();
        assert_eq!(
            (probe.curve.as_str(), probe.step_up_pct),
            (PROBE_CURVE, 100)
        );
        assert!(q
            .curves
            .iter()
            .any(|c| matches!(c, Curve::Flat { id, pwm: 100 } if id == PROBE_CURVE)));
        assert_eq!(with_probe(&q, "b"), q, "probing twice adds nothing");
    }
}
