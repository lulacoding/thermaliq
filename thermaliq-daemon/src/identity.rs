//! Fan identity wiring (SPEC §5.2): `roles.toml` and `inventory.json` in the data dir, the
//! start-up checks, and the first-run probe. The rules themselves are in `thermaliq_common::identity`.

use crate::runtime::{AgentState, Cache, CommandError, Handle};
use crate::store::write_atomic;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use std::time::Duration;
use thermaliq_common::identity::{
    diff, missing_fans, missing_message, pair, propose, resolve, resolve_alerts, unmapped_lines,
    Inventory, InventoryDiff, Role, Roles, CASE_FANS,
};
use thermaliq_common::{AlertEntry, HardwareSnapshot, Profile};
use thermaliq_hal::HardwareLayer;

/// How long a probe holds a fan at 100% before it reads the tachs.
pub const PROBE_FOR: Duration = Duration::from_secs(5);

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
    /// Owned by the probe task, so it is held until the profile is back even if the request drops.
    probing: std::sync::Arc<tokio::sync::Mutex<()>>,
    probe_for: Duration,
    /// Set at load when the saved inventory is for another board; every role is then unmapped.
    board_reason: Option<String>,
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

/// Fans `inventory.json` says have reported RPM above 0. Empty without an inventory.
pub fn tach_seen(dir: &Path) -> std::collections::HashSet<String> {
    match read_inventory(dir) {
        Ok(Some(inv)) => inv
            .fans
            .into_iter()
            .filter(|f| f.tach_seen)
            .map(|f| f.id)
            .collect(),
        _ => Default::default(),
    }
}

/// Saves that these fans have reported RPM, once and for good (a fan that later dies is still a
/// fan with a tach). Without a saved inventory there is nowhere to keep it: the next hourly
/// pass tries again.
pub fn mark_tach_seen(dir: &Path, fans: &std::collections::HashSet<String>) {
    let Ok(Some(mut inv)) = read_inventory(dir) else {
        return;
    };
    let mut changed = false;
    for f in inv
        .fans
        .iter_mut()
        .filter(|f| fans.contains(&f.id) && !f.tach_seen)
    {
        f.tach_seen = true;
        changed = true;
    }
    if changed {
        if let Err(e) = save_inventory(dir, &inv) {
            eprintln!("thermaliq: could not remember which fans have a tach: {e}");
        }
    }
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
            probing: Default::default(),
            probe_for: PROBE_FOR,
            board_reason: None,
        }
    }

    /// Why every role was left unmapped, if the board changed (SPEC §5.2).
    pub fn board_reason(&self) -> Option<&str> {
        self.board_reason.as_deref()
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
        // The same ids on another board can be other fans, so the saved mapping is not trusted.
        let moved = match read_inventory(dir) {
            Ok(Some(saved)) => {
                diff(
                    &saved,
                    &Inventory::of(&HardwareSnapshot::default(), id.board.clone()),
                )
                .board_changed
            }
            _ => None,
        };
        let roles = if let Some((was, now)) = moved {
            let show = |b: Option<String>| b.unwrap_or_else(|| "none".into());
            let why = format!(
                "the board changed (saved: {}, now: {}); no role is mapped. Run `tq identify`",
                show(was),
                show(now)
            );
            id.problems.push(why.clone());
            id.board_reason = Some(why);
            Roles::default()
        } else {
            read_roles(dir).unwrap_or_else(|e| {
                id.problems
                    .push(format!("roles.toml: {e}; no role is mapped"));
                Roles::default()
            })
        };
        let mut unmapped = Vec::new();
        let resolved: Vec<Profile> = profiles
            .iter()
            .map(|p| {
                let r = resolve(p, &roles);
                id.problems.extend(r.skipped);
                unmapped.push((p.name.clone(), r.unmapped));
                let missing = missing_fans(&r.profile, &id.controllable);
                if !missing.is_empty() {
                    id.problems.push(missing_message(&r.profile.name, &missing));
                }
                r.profile
            })
            .collect();
        id.problems.extend(unmapped_lines(&unmapped));
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
        let rejected = self.rejected();
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
                "rejected_writes": rejected.contains(&f.id),
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
        wanted: &BTreeMap<String, Role>,
        snap: &HardwareSnapshot,
    ) -> Result<(), String> {
        let mut roles = Roles::default();
        let mut used = std::collections::HashSet::new();
        for (role, value) in wanted {
            let ids: Vec<&String> = match (role == CASE_FANS, value) {
                (true, Role::Many(ids)) => ids.iter().collect(),
                (false, Role::One(id)) => vec![id],
                (true, Role::One(_)) => return Err(format!("role '{role}' takes a list of ids")),
                (false, Role::Many(_)) => return Err(format!("role '{role}' takes one id")),
            };
            for id in ids {
                if !used.insert(id) {
                    return Err(format!("'{id}' is wanted twice"));
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
        }
        write_atomic(&self.dir.join("roles.toml"), roles.to_toml().as_bytes())
            .map_err(|e| format!("roles.toml: {e}"))?;
        *self.roles.lock().unwrap_or_else(PoisonError::into_inner) = roles;
        Ok(())
    }

    /// The config's alerts with sensor roles turned into real ids. An alert on a role nothing is
    /// mapped to is dropped, printed, and listed in `problems` (SPEC §4.2).
    pub fn resolve_alerts(&self, alerts: &[AlertEntry]) -> Vec<AlertEntry> {
        let (kept, skipped) = resolve_alerts(alerts, &self.roles());
        for line in &skipped {
            eprintln!("thermaliq: {line}");
        }
        self.late
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(skipped);
        kept
    }

    /// Makes today's fans the saved inventory. An empty snapshot is refused: saving it would
    /// make every fan look new next time.
    pub fn accept_inventory(&self, snap: &HardwareSnapshot) -> Result<(), String> {
        if snap.fans.is_empty() {
            return Err("the snapshot has no fans; not saving an empty inventory".into());
        }
        let mut now = Inventory::of(snap, self.board.clone());
        if let Ok(Some(saved)) = read_inventory(&self.dir) {
            now.keep_flags_from(&saved);
        }
        save_inventory(&self.dir, &now)
    }

    /// Which headers a probe found deaf to writes, so `tq identify` does not run them again.
    fn rejected(&self) -> Vec<String> {
        match read_inventory(&self.dir) {
            Ok(Some(inv)) => inv
                .fans
                .into_iter()
                .filter(|f| f.rejected_writes)
                .map(|f| f.id)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Records what a probe found. Without a saved inventory there is nowhere to keep it.
    fn remember_probe(&self, fan: &str, landed: bool) {
        let Ok(Some(mut inv)) = read_inventory(&self.dir) else {
            return;
        };
        let Some(f) = inv.fans.iter_mut().find(|f| f.id == fan) else {
            return;
        };
        if f.rejected_writes != !landed {
            f.rejected_writes = !landed;
            if let Err(e) = save_inventory(&self.dir, &inv) {
                eprintln!("thermaliq: could not remember the probe of '{fan}': {e}");
            }
        }
    }

    /// Holds `fan` at 100% for `probe_for` through the engine (SPEC §5.2). It only raises a fan,
    /// so a pump is only ever raised.
    ///
    /// The hold is a request of its own that expires 2 s after the hold, not a profile swap: a
    /// profile an app rule, schedule or the user picks meanwhile stays, and ending the probe
    /// withdraws only this request. The hold and its end run in their own task, so if the request
    /// drops (Ctrl-C in `identify`) the engine is still told. The agent is paused for the hold
    /// (and resumed only if this probe paused it).
    pub async fn probe(&self, handle: &Handle, fan: &str) -> Result<Value, ProbeError> {
        if !self.controllable.iter().any(|f| f == fan) {
            return Err(ProbeError::UnknownFan(fan.to_owned()));
        }
        let Ok(one_at_a_time) = self.probing.clone().try_lock_owned() else {
            return Err(ProbeError::Busy);
        };
        let before = handle.cache();
        if before.status.latched {
            return Err(ProbeError::Latched);
        }
        let (task_handle, task_fan, hold) = (handle.clone(), fan.to_owned(), self.probe_for);
        let result = tokio::spawn(async move {
            let _one_at_a_time = one_at_a_time;
            hold_and_release(&task_handle, before, &task_fan, hold).await
        })
        .await
        .unwrap_or_else(|e| {
            Err(ProbeError::NotRestored(format!(
                "the probe task failed ({e}); the hold ends by itself within seconds"
            )))
        });
        if let Ok(v) = &result {
            self.remember_probe(fan, v["landed"] == true);
        }
        result
    }
}

/// The engine ends the hold by itself this long after `hold`, if nobody ends it first.
const PROBE_GRACE: Duration = Duration::from_secs(2);

async fn hold_and_release(
    handle: &Handle,
    before: Cache,
    fan: &str,
    hold: Duration,
) -> Result<Value, ProbeError> {
    // A running agent is paused for the hold; one that was already paused stays paused. A
    // crashed agent cannot be paused, and cannot switch either.
    let paused_here =
        before.status.agent == AgentState::Running && handle.pause_agent().await.is_ok();
    let held = handle.start_probe(fan, hold + PROBE_GRACE).await;
    if held.is_ok() {
        tokio::time::sleep(hold).await;
    }
    let during = handle.cache();
    let released = match &held {
        Ok(id) => handle.end_probe(fan, *id).await,
        Err(_) => Ok(()),
    };
    if paused_here {
        let _ = handle.resume_agent().await;
    }
    held.map_err(ProbeError::Engine)?;
    released.map_err(|e| {
        ProbeError::NotRestored(format!(
            "could not end the hold on fan '{fan}' ({e:?}); it ends by itself within seconds"
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
    // What the engine last wrote to the fan and the chip accepted. A header that rejects writes
    // (a channel with no header behind it, say) never gets there, whatever the engine asked.
    let landed = during
        .fan_info
        .get(fan)
        .is_some_and(|i| i.commanded == Some(100) && !i.write_failing);
    let duty_after = during
        .snapshot
        .fans
        .iter()
        .find(|f| f.id == fan)
        .map(|f| f.duty_percent);
    Ok(json!({
        "fan": fan,
        "label": label,
        "rpm_before": was,
        "rpm_after": now,
        "pairing": pair(fan, &was, &now),
        "landed": landed,
        "duty_after": duty_after,
    }))
}

fn rpms(c: &Cache) -> Vec<(String, u32)> {
    c.snapshot
        .fans
        .iter()
        .map(|f| (f.id.clone(), f.rpm))
        .collect()
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
        [[controls]]\nfan = \"case_fans\"\ncurve = \"c\"\n";

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
            "[roles]\ncase_fans = [\"f1\", \"f2\"]\ncpu_temp = \"cpu\"\n",
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
        fs::write(d.path().join("roles.toml"), "[roles]\ncpu_temp = \"cpu\"\n").unwrap();
        let (id, profiles) = Identity::load(d.path(), &*hal(&["f1"]), bal());
        assert!(profiles[0].controls.is_empty());
        assert!(
            id.problems.iter().any(|p| p.contains("'case_fans'")),
            "{:?}",
            id.problems
        );
    }

    #[test]
    fn an_unmapped_role_is_one_line_listing_every_profile_that_uses_it() {
        let d = dir();
        let mk = |name: &str| {
            Profile::from_toml(&format!(
                "name = \"{name}\"\n[[curves]]\nid = \"c\"\ntype = \"linear\"\nsource = \"cpu_temp\"\nmin_temp = 30\nmax_temp = 90\nmin_pwm = 20\nmax_pwm = 100\n\
                 [[controls]]\nfan = \"case_fans\"\ncurve = \"c\"\n"
            ))
            .unwrap()
        };
        let (id, _) = Identity::load(d.path(), &*hal(&["f1"]), vec![mk("balanced"), mk("gaming")]);
        for role in ["case_fans", "cpu_temp"] {
            let lines: Vec<_> = id.problems.iter().filter(|p| p.contains(role)).collect();
            assert_eq!(lines.len(), 1, "{role}: {:?}", id.problems);
            assert!(
                lines[0].contains("balanced") && lines[0].contains("gaming"),
                "{}",
                lines[0]
            );
        }
    }

    #[test]
    fn a_mapped_fan_that_no_backend_lists_is_named_at_load() {
        let d = dir();
        fs::write(
            d.path().join("roles.toml"),
            "[roles]\ncase_fans = [\"f1\", \"gone\"]\ncpu_temp = \"cpu\"\n",
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
        let want = |pairs: &[(&str, &[&str])]| -> BTreeMap<String, Role> {
            pairs
                .iter()
                .map(|(role, ids)| {
                    let ids: Vec<String> = ids.iter().map(|i| (*i).into()).collect();
                    let v = if *role == "case_fans" {
                        Role::Many(ids)
                    } else {
                        Role::One(ids[0].clone())
                    };
                    ((*role).into(), v)
                })
                .collect()
        };
        for bad in [
            want(&[("case_fans", &["f1", "nope"])]),
            want(&[("cpu_temp", &["f1"])]),
            want(&[("middle", &["f1"])]),
            want(&[("case_front", &["f1"])]),
            want(&[("case_fans", &["f1"]), ("cpu_fan", &["f1"])]),
            want(&[("case_fans", &["f1", "f1"])]),
        ] {
            assert!(id.set_roles(&bad, &snap).is_err(), "{bad:?}");
        }
        // The wrong shape for a role.
        let one = BTreeMap::from([("case_fans".to_owned(), Role::One("f1".into()))]);
        let many = BTreeMap::from([("cpu_fan".to_owned(), Role::Many(vec!["f1".into()]))]);
        assert!(id.set_roles(&one, &snap).is_err() && id.set_roles(&many, &snap).is_err());
        assert!(
            !d.path().join("roles.toml").exists(),
            "a refused set writes nothing"
        );
        id.set_roles(
            &want(&[("case_fans", &["f1", "f2"]), ("cpu_temp", &["cpu"])]),
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
            .any(|r| r == "case_fans"));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn reviewer_probe_completion_does_not_restore_a_stale_profile() {
        let (h, d, profiles) = running(&["f1", "f2"]);
        let id = Arc::new(identity_for(&["f1", "f2"]));
        let run = tokio::spawn({
            let (id, handle) = (id.clone(), d.handle());
            async move { id.probe(&handle, "f2").await }
        });
        wait_until(|| {
            h.writes()
                .iter()
                .any(|(fan, duty)| fan == "f2" && *duty == 100)
        });
        let mut selected = profiles[0].clone();
        selected.name = "new-selection".into();
        d.handle().set_profile(selected).await.unwrap();
        run.await.unwrap().unwrap();
        assert_eq!(
            d.cache().active_profile,
            "new-selection",
            "probe completion overwrote the newer selection"
        );
    }
    #[tokio::test]
    async fn a_probe_raises_the_fan_pairs_its_tach_and_puts_the_profile_back() {
        let (h, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        let r = id.probe(&d.handle(), "f2").await.unwrap();
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

    // Multi-thread: `wait_until` blocks, and the probe's own task has to keep running meanwhile.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_probe_dropped_mid_hold_still_puts_the_profile_back() {
        let (h, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        // The client leaves (Ctrl-C in identify) after the 100% write; axum drops the future.
        let r = tokio::time::timeout(Duration::from_millis(60), id.probe(&d.handle(), "f2")).await;
        assert!(r.is_err(), "the probe should still be holding");
        wait_until(|| h.writes().iter().any(|(f, duty)| f == "f2" && *duty == 100));
        // `flat` never controls f2, so the original profile back means f2 is handed to the BIOS.
        wait_until(|| h.restores().contains(&"f2".to_owned()));
        // The task still holds `probing` for a moment after the restore (it resumes the agent and
        // builds its result), so a new probe may be Busy until then; once it is not, it must work.
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            match id.probe(&d.handle(), "f1").await {
                Err(ProbeError::Busy) if Instant::now() < end => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                r => {
                    assert!(r.is_ok(), "{r:?}");
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn the_agent_is_paused_for_a_probe_and_resumed_only_if_it_was_running() {
        let (_, d, _) = running(&["f1", "f2"]);
        let id = std::sync::Arc::new(identity_for(&["f1", "f2"]));
        for was_paused in [false, true] {
            if was_paused {
                d.handle().pause_agent().await.unwrap();
            }
            let run = tokio::spawn({
                let (id, h) = (id.clone(), d.handle());
                async move { id.probe(&h, "f2").await }
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(d.cache().status.agent, crate::runtime::AgentState::Paused);
            run.await.unwrap().unwrap();
            let after = d.cache().status.agent;
            assert_eq!(after == crate::runtime::AgentState::Paused, was_paused);
        }
    }

    /// Wraps a mock with a board name.
    struct Boarded(Arc<MockHardwareLayer>, &'static str);
    impl HardwareLayer for Boarded {
        fn snapshot(&self) -> Result<HardwareSnapshot, thermaliq_hal::HalError> {
            self.0.snapshot()
        }
        fn set_fan_duty(&self, f: &str, d: u8) -> Result<(), thermaliq_hal::HalError> {
            self.0.set_fan_duty(f, d)
        }
        fn restore_auto(&self, f: &str) -> Result<(), thermaliq_hal::HalError> {
            self.0.restore_auto(f)
        }
        fn capabilities(&self) -> thermaliq_hal::Capabilities {
            self.0.capabilities()
        }
        fn board(&self) -> Option<String> {
            Some(self.1.into())
        }
    }

    #[test]
    fn a_changed_board_loads_every_role_unmapped_and_says_what_to_run() {
        let d = dir();
        let roles = "[roles]\ncase_fans = [\"f1\", \"f2\"]\ncpu_temp = \"cpu\"\n";
        fs::write(d.path().join("roles.toml"), roles).unwrap();
        let old = Boarded(hal(&["f1", "f2"]), "OLD BOARD");
        let (first, mapped) = Identity::load(d.path(), &old, bal());
        assert_eq!(mapped[0].controls.len(), 2);
        first.check_inventory(&old.snapshot().unwrap());

        let new = Boarded(hal(&["f1", "f2"]), "NEW BOARD");
        let (id, profiles) = Identity::load(d.path(), &new, bal());
        assert!(profiles[0].controls.is_empty(), "same ids, other board");
        assert!(id.roles().iter().next().is_none());
        let why = id.board_reason().unwrap();
        assert!(
            why.contains("NEW BOARD") && why.contains("tq identify"),
            "{why}"
        );

        // Still so on the next start; accepting makes it the saved board, so roles load again.
        let (again, _) = Identity::load(d.path(), &new, bal());
        assert!(again.board_reason().is_some());
        again.accept_inventory(&new.snapshot().unwrap()).unwrap();
        let (ok, profiles) = Identity::load(d.path(), &new, bal());
        assert!(ok.board_reason().is_none());
        assert_eq!(profiles[0].controls.len(), 2);
    }

    #[tokio::test]
    async fn a_probe_says_whether_the_write_landed_and_where_the_chip_ended_up() {
        let (h, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        let ok = id.probe(&d.handle(), "f2").await.unwrap();
        assert_eq!(ok["landed"], true, "{ok}");
        assert_eq!(ok["duty_after"], 100, "{ok}");

        // A header whose writes the HAL refuses: commanded never reaches 100.
        h.fail_writes("f1");
        let bad = id.probe(&d.handle(), "f1").await.unwrap();
        assert_eq!(bad["landed"], false, "{bad}");
        assert!(
            bad["duty_after"].is_u64(),
            "the chip's own reading is reported: {bad}"
        );
    }

    /// M16 (a): the engine's last accepted duty is 100 from an earlier hold. A write that fails
    /// now must not read as landed.
    #[tokio::test]
    async fn a_stale_commanded_100_does_not_hide_a_write_that_now_fails() {
        let (h, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        assert_eq!(id.probe(&d.handle(), "f2").await.unwrap()["landed"], true);
        assert_eq!(d.handle().cache().fan_info["f2"].commanded, Some(100));

        h.fail_writes("f2");
        let bad = id.probe(&d.handle(), "f2").await.unwrap();
        assert_eq!(bad["landed"], false, "{bad}");

        h.heal_writes("f2");
        assert_eq!(id.probe(&d.handle(), "f2").await.unwrap()["landed"], true);
    }

    fn saved_flag(dir: &Path, fan: &str) -> bool {
        read_inventory(dir)
            .unwrap()
            .unwrap()
            .fans
            .iter()
            .any(|f| f.id == fan && f.rejected_writes)
    }

    /// M16 (c): a header that rejected writes is remembered, shown, kept by accepting, and not
    /// a change; a later probe that lands clears it.
    #[tokio::test]
    async fn a_probe_that_does_not_land_is_remembered_in_the_inventory() {
        let dir = dir();
        let (h, d, _) = running(&["f1", "f2"]);
        let mut id =
            Identity::empty(dir.path().to_owned()).with_probe_time(Duration::from_millis(150));
        id.controllable = vec!["f1".into(), "f2".into()];
        let snap = d.cache().snapshot;
        id.check_inventory(&snap);

        h.fail_writes("f2");
        id.probe(&d.handle(), "f2").await.unwrap();
        assert!(saved_flag(dir.path(), "f2") && !saved_flag(dir.path(), "f1"));
        let fans = id.describe(&snap)["fans"].clone();
        assert_eq!(fans[1]["rejected_writes"], true, "{fans}");
        assert_eq!(fans[0]["rejected_writes"], false, "{fans}");

        id.accept_inventory(&snap).unwrap();
        assert!(saved_flag(dir.path(), "f2"), "accepting forgot it");
        id.check_inventory(&snap);
        assert_eq!(id.describe(&snap)["inventory"]["new"], json!([]));
        assert_eq!(id.describe(&snap)["inventory"]["changed"], json!([]));

        h.heal_writes("f2");
        id.probe(&d.handle(), "f2").await.unwrap();
        assert!(
            !saved_flag(dir.path(), "f2"),
            "a write that landed clears it"
        );
    }

    fn two_fans() -> HardwareSnapshot {
        let fan = |id: &str| thermaliq_common::FanReading {
            id: id.into(),
            label: id.into(),
            rpm: 0,
            duty_percent: 0,
            kind: thermaliq_common::FanKind::Fan,
        };
        HardwareSnapshot {
            fans: vec![fan("f1"), fan("f2")],
            ..Default::default()
        }
    }

    /// M16 review: "has this fan ever reported RPM" lives in `inventory.json`, so a fan that
    /// died is still a fan with a tach after a restart, and accepting the inventory keeps it.
    #[test]
    fn tach_seen_is_saved_in_the_inventory_once_and_survives_accepting_it() {
        let dir = dir();
        let id = Identity::empty(dir.path().to_owned());
        assert!(tach_seen(dir.path()).is_empty(), "no inventory yet");
        mark_tach_seen(dir.path(), &["f1".to_owned()].into()); // nowhere to keep it: no panic
        assert!(!dir.path().join("inventory.json").exists());

        id.check_inventory(&two_fans());
        assert!(tach_seen(dir.path()).is_empty());
        mark_tach_seen(dir.path(), &["f1".to_owned(), "gone".to_owned()].into());
        assert_eq!(tach_seen(dir.path()), ["f1".to_owned()].into());

        id.accept_inventory(&two_fans()).unwrap();
        assert_eq!(
            tach_seen(dir.path()),
            ["f1".to_owned()].into(),
            "accept kept it"
        );
        id.check_inventory(&two_fans());
        assert_eq!(id.describe(&two_fans())["inventory"]["changed"], json!([]));
    }

    #[test]
    fn an_inventory_file_from_before_rejected_writes_still_loads() {
        let dir = dir();
        fs::write(
            dir.path().join("inventory.json"),
            r#"{"board":null,"fans":[{"id":"f1","label":"f1","kind":"fan"}]}"#,
        )
        .unwrap();
        assert!(!saved_flag(dir.path(), "f1"));
        assert_eq!(read_inventory(dir.path()).unwrap().unwrap().fans.len(), 1);
    }

    #[tokio::test]
    async fn a_probe_of_a_fan_no_backend_lists_is_refused_and_touches_nothing() {
        let (h, d, _) = running(&["f1"]);
        let id = identity_for(&["f1"]);
        let before = h.writes().len();
        let err = id.probe(&d.handle(), "nope").await.unwrap_err();
        assert!(matches!(err, ProbeError::UnknownFan(f) if f == "nope"));
        assert_eq!(h.writes().len(), before);
    }

    #[tokio::test]
    async fn two_probes_at_once_are_refused_not_stacked() {
        let (_, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        let handle = d.handle();
        let (a, b) = tokio::join!(id.probe(&handle, "f1"), id.probe(&handle, "f2"));
        assert!(a.is_ok() ^ b.is_ok(), "{a:?} {b:?}");
        assert!(matches!(a.err().or(b.err()), Some(ProbeError::Busy)));
    }

    #[tokio::test]
    async fn a_probe_refuses_to_start_while_the_latch_holds() {
        let h = Arc::new(MockHardwareLayer::new(
            &["f1"],
            vec![Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 97.0)])],
        ));
        let flat = Profile::from_toml(
            "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\n",
        )
        .unwrap();
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        let d = Daemon::start(h, &flat, vec![], db, fast());
        wait_until(|| d.cache().status.latched);
        let err = identity_for(&["f1"])
            .probe(&d.handle(), "f1")
            .await
            .unwrap_err();
        assert!(matches!(err, ProbeError::Latched), "{err:?}");
    }

    #[tokio::test]
    async fn a_probe_is_refused_on_a_fan_with_a_live_override() {
        let (h, d, _) = running(&["f1", "f2"]);
        let id = identity_for(&["f1", "f2"]);
        d.handle().set_override("f1", 80, 60).await.unwrap();
        let err = id.probe(&d.handle(), "f1").await.unwrap_err();
        assert!(
            matches!(&err, ProbeError::Engine(CommandError::Rejected(m)) if m.contains("live override")),
            "{err:?}"
        );
        assert!(!h.writes().iter().any(|(f, duty)| f == "f1" && *duty == 100));
        assert_eq!(
            d.cache().status.agent,
            crate::runtime::AgentState::Running,
            "resumed"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_probe_keeps_holding_and_leaves_a_concurrent_automatic_switch_alone() {
        let (h, d, profiles) = running(&["f1", "f2"]);
        let id = Arc::new(identity_for(&["f1", "f2"]));
        let run = tokio::spawn({
            let (id, handle) = (id.clone(), d.handle());
            async move { id.probe(&handle, "f2").await }
        });
        wait_until(|| {
            h.writes()
                .iter()
                .any(|(fan, duty)| fan == "f2" && *duty == 100)
        });
        // What the supervisor does when an app rule or schedule wins mid-hold.
        let mut automatic = profiles[0].clone();
        automatic.name = "automatic".into();
        d.handle().apply_profile(automatic).await.unwrap();
        let r = run.await.unwrap().unwrap();
        assert_eq!(r["landed"], true, "{r}");
        assert_eq!(d.cache().active_profile, "automatic");
    }
}
