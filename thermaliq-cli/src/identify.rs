//! `tq identify`: the first-run walk through the fans (SPEC §5.2). It talks to the running
//! daemon over the API, never to hardware: a probe is a request the engine carries out.

use crate::client::Client;
use crate::CliError;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use thermaliq_common::identity::{Role, CASE_FANS};

pub trait Api {
    fn identity(&self) -> Result<Value, CliError>;
    fn probe(&self, fan: &str) -> Result<Value, CliError>;
    fn put_roles(&self, roles: &Value) -> Result<(), CliError>;
    fn accept_inventory(&self) -> Result<(), CliError>;
}

impl Api for Client {
    fn identity(&self) -> Result<Value, CliError> {
        self.get("/identity")
    }
    fn probe(&self, fan: &str) -> Result<Value, CliError> {
        self.post("/identify/probe", Some(json!({ "fan": fan })))
    }
    fn put_roles(&self, roles: &Value) -> Result<(), CliError> {
        self.put("/roles", json!({ "roles": roles })).map(drop)
    }
    fn accept_inventory(&self) -> Result<(), CliError> {
        self.post("/inventory/accept", None).map(drop)
    }
}

fn label_of(v: &Value, id: &str) -> String {
    v["fans"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["id"] == id)
        .and_then(|f| f["label"].as_str())
        .unwrap_or(id)
        .to_owned()
}

/// What the chip read at the end of a probe, for a message.
fn chip(r: &Value) -> String {
    r["duty_after"]
        .as_u64()
        .map_or_else(|| "an unknown duty".into(), |d| format!("{d}%"))
}

struct Term<'a> {
    input: &'a mut dyn BufRead,
    out: &'a mut dyn Write,
}

impl Term<'_> {
    fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
    }

    /// The answer, lower case and trimmed. `None` at the end of input or on `q`, which both
    /// mean stop without saving.
    fn ask(&mut self, prompt: &str) -> Option<String> {
        let _ = write!(self.out, "{prompt} ");
        let _ = self.out.flush();
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                let a = line.trim().to_lowercase();
                (a != "q" && a != "quit").then_some(a)
            }
        }
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn rpm_of(list: &Value, fan: &str) -> Option<u64> {
    list.as_array()?
        .iter()
        .find(|e| e[0] == fan)
        .and_then(|e| e[1].as_u64())
}

fn pairing_line(v: &Value, fan: &str) -> String {
    match v["pairing"]["result"].as_str() {
        Some("own") => format!(
            "its own tach rose ({} -> {} RPM)",
            rpm_of(&v["rpm_before"], fan).unwrap_or(0),
            rpm_of(&v["rpm_after"], fan).unwrap_or(0)
        ),
        Some("other") => format!(
            "the tach of '{}' rose instead, so the wiring may be crossed. Nothing is guessed",
            v["pairing"]["fan"].as_str().unwrap_or("?")
        ),
        _ => "no tach moved: it has none, is unplugged, or was already at full speed".into(),
    }
}

/// Shows what the daemon would map, then asks "Save these? [Y/n]". `y` or a bare Enter saves
/// `roles.toml` and accepts the inventory; `n`, `q` or the end of input saves nothing. It never
/// probes a fan and never asks where one is (SPEC §5.2).
pub fn run(api: &dyn Api, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), CliError> {
    let mut t = Term { input, out };
    let v = api.identity()?;
    if let Some(b) = v["board"].as_str() {
        t.say(&format!("Board: {b}"));
    }
    for p in strings(&v["problems"]) {
        t.say(&format!("Problem: {p}"));
    }
    let mut roles: BTreeMap<String, Role> =
        serde_json::from_value(v["roles"].clone()).unwrap_or_default();
    let before = roles.clone();
    let used = |roles: &BTreeMap<String, Role>, id: &str| {
        roles.values().any(|r| match r {
            Role::One(i) => i == id,
            Role::Many(m) => m.iter().any(|i| i == id),
        })
    };

    let mut candidates: Vec<String> = Vec::new();
    for p in v["proposals"].as_array().into_iter().flatten() {
        let (Some(role), Some(id)) = (p["role"].as_str(), p["id"].as_str()) else {
            continue;
        };
        if used(&roles, id) {
            continue;
        }
        if role == CASE_FANS {
            if let Role::Many(m) = roles
                .entry(role.to_owned())
                .or_insert_with(|| Role::Many(Vec::new()))
            {
                m.push(id.to_owned());
                candidates.push(id.to_owned());
            }
        } else if roles.contains_key(role) {
            continue;
        } else {
            roles.insert(role.to_owned(), Role::One(id.to_owned()));
        }
        t.say(&format!(
            "  {role} <- {id}  ({})",
            p["why"].as_str().unwrap_or("")
        ));
    }

    // A header can exist as a channel with no header behind it: the chip then ignores writes.
    // Each new candidate runs at 100% once (raise-only, agent paused by the daemon); one whose
    // write does not land is not worth controlling.
    if !candidates.is_empty() {
        t.say(&format!(
            "\nChecking {} case fan{}: each runs at 100% for 5 s, one at a time.",
            candidates.len(),
            if candidates.len() == 1 { "" } else { "s" }
        ));
    }
    for id in candidates {
        let label = label_of(&v, &id);
        let left_out = match api.probe(&id) {
            Ok(r) if r["landed"] == true => {
                t.say(&format!("  {label}: the write landed"));
                None
            }
            Ok(r) => Some(format!(
                "{label} rejects writes (chip stays at {}): left out",
                chip(&r)
            )),
            Err(e) => Some(format!("{label} could not be checked ({e}): left out")),
        };
        if let Some(why) = left_out {
            t.say(&format!("  {why}"));
            if let Some(Role::Many(m)) = roles.get_mut(CASE_FANS) {
                m.retain(|i| *i != id);
            }
        }
    }
    if roles.get(CASE_FANS) == Some(&Role::Many(Vec::new())) {
        roles.remove(CASE_FANS);
    }

    let unmapped: Vec<_> = strings(&v["unmapped_roles"])
        .into_iter()
        .filter(|r| !roles.contains_key(r))
        .collect();
    if !unmapped.is_empty() {
        t.say(&format!(
            "\nNot mapped: {}. Profiles skip what uses them and the log says so.",
            unmapped.join(", ")
        ));
    }

    if roles != before {
        t.say("\nRoles to save:");
        for (r, i) in &roles {
            let ids = match i {
                Role::One(id) => id.clone(),
                Role::Many(m) => m.join(", "),
            };
            t.say(&format!("  {r} = {ids}"));
        }
        if t.ask("Save these? [Y/n]")
            .is_none_or(|a| a.starts_with('n'))
        {
            t.say("Nothing was saved.");
            return Ok(());
        }
        api.put_roles(&json!(roles))?;
        api.accept_inventory()?;
        t.say("Saved. Restart the ThermalIQ service so profiles use these roles.");
        return Ok(());
    }
    let changed = !v["inventory"]["new"].as_array().is_none_or(Vec::is_empty)
        || !v["inventory"]["missing"]
            .as_array()
            .is_none_or(Vec::is_empty)
        || !v["inventory"]["changed"]
            .as_array()
            .is_none_or(Vec::is_empty)
        || !v["inventory"]["board_changed"].is_null();
    if changed
        && t.ask("The fans differ from the saved inventory. Accept them as it is? [y/N]")
            .is_some_and(|a| a.starts_with('y'))
    {
        api.accept_inventory()?;
        t.say("Inventory saved.");
    } else {
        t.say("Nothing changed.");
    }
    Ok(())
}

/// `tq identify --probe <fan>`: runs that one fan at 100% for 5 s and says which tach rose.
/// Opt-in, and it saves nothing.
pub fn run_probe(api: &dyn Api, fan: &str, out: &mut dyn Write) -> Result<(), CliError> {
    let v = api.identity()?;
    let label = v["fans"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["id"] == fan)
        .and_then(|f| f["label"].as_str())
        .unwrap_or(fan);
    let _ = writeln!(out, "Running '{label}' ({fan}) at 100% for 5 s...");
    let r = api.probe(fan)?;
    let landed = if r["landed"] == true {
        format!("the write landed (the chip reads {})", chip(&r))
    } else {
        format!("it rejects writes (chip stays at {})", chip(&r))
    };
    let _ = writeln!(
        out,
        "  {}\n  {landed}\nNothing was saved.",
        pairing_line(&r, fan)
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        identity: Value,
        probe_result: Value,
        probe_error: Option<String>,
        per_fan: std::collections::HashMap<String, Result<Value, CliError>>,
        probed: RefCell<Vec<String>>,
        saved: RefCell<Option<Value>>,
        accepted: RefCell<u32>,
    }

    impl Api for Fake {
        fn identity(&self) -> Result<Value, CliError> {
            Ok(self.identity.clone())
        }
        fn probe(&self, fan: &str) -> Result<Value, CliError> {
            self.probed.borrow_mut().push(fan.into());
            if let Some(r) = self.per_fan.get(fan) {
                return r.clone();
            }
            match &self.probe_error {
                Some(e) => Err(e.as_str().into()),
                None => Ok(self.probe_result.clone()),
            }
        }
        fn put_roles(&self, roles: &Value) -> Result<(), CliError> {
            *self.saved.borrow_mut() = Some(roles.clone());
            Ok(())
        }
        fn accept_inventory(&self) -> Result<(), CliError> {
            *self.accepted.borrow_mut() += 1;
            Ok(())
        }
    }

    fn fan(id: &str, label: &str, kind: &str, rpm: u32) -> Value {
        json!({"id": id, "label": label, "kind": kind, "rpm": rpm, "controllable": true})
    }

    fn proposal(role: &str, id: &str, why: &str) -> Value {
        json!({"role": role, "id": id, "why": why})
    }

    /// This PC: System Fan #1 and #2 (no tach), a CPU fan, a pump and the GPU fan.
    fn pc() -> Fake {
        Fake {
            identity: json!({
                "board": "TEST BOARD",
                "problems": [],
                "roles": {},
                "unmapped_roles": ["cpu_fan", "pump", "gpu_fan", "case_fans", "cpu_temp", "gpu_temp"],
                "inventory": {"new": [], "missing": [], "changed": [], "board_changed": null},
                "proposals": [
                    proposal("gpu_fan", "nvml_GPU-a_fan0", "an NVIDIA GPU fan"),
                    proposal("cpu_fan", "h0", "its label says CPU"),
                    proposal("case_fans", "h1", "a fan header"),
                    proposal("case_fans", "h2", "a fan header"),
                    proposal("pump", "p0", "a pump by its kind or label"),
                ],
                "fans": [
                    fan("nvml_GPU-a_fan0", "GPU 0 fan 0", "fan", 900),
                    fan("h0", "CPU Fan", "fan", 800),
                    fan("h1", "System Fan #1", "fan", 0),
                    fan("h2", "System Fan #2", "fan", 0),
                    fan("p0", "Pump Fan", "pump", 2000),
                ],
            }),
            probe_result: json!({
                "rpm_before": [["h1", 500]], "rpm_after": [["h1", 1300]],
                "pairing": {"result": "own"}, "landed": true, "duty_after": 100,
            }),
            ..Fake::default()
        }
    }

    fn go(api: &Fake, answers: &str) -> String {
        let mut out = Vec::new();
        run(api, &mut answers.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn mapped(api: &mut Fake) {
        api.identity["roles"] = json!({"gpu_fan": "nvml_GPU-a_fan0", "cpu_fan": "h0", "pump": "p0", "case_fans": ["h1", "h2"]});
    }

    #[test]
    fn it_shows_the_proposals_with_the_case_fans_list_and_enter_saves_them() {
        let api = pc();
        let out = go(&api, "\n");
        let saved = api.saved.borrow().clone().unwrap();
        assert_eq!(
            saved,
            json!({"gpu_fan": "nvml_GPU-a_fan0", "cpu_fan": "h0", "pump": "p0", "case_fans": ["h1", "h2"]})
        );
        assert_eq!(*api.accepted.borrow(), 1);
        for want in [
            "case_fans",
            "h1",
            "h2",
            "Save these? [Y/n]",
            "Restart the ThermalIQ service",
        ] {
            assert!(out.contains(want), "{want}: {out}");
        }
    }

    fn rejects(api: &mut Fake, fan: &str, chip: u8) {
        api.per_fan.insert(
            fan.into(),
            Ok(
                json!({"rpm_before": [], "rpm_after": [], "pairing": {"result": "none"},
                      "landed": false, "duty_after": chip}),
            ),
        );
    }

    #[test]
    fn each_new_case_fans_candidate_is_probed_once_and_never_a_pump_or_another_role() {
        let api = pc();
        let out = go(&api, "\n");
        assert_eq!(*api.probed.borrow(), ["h1", "h2"]);
        assert!(
            out.contains("100% for 5 s"),
            "tells you it runs the fans: {out}"
        );
        for no in ["louder", "front/rear"] {
            assert!(!out.contains(no), "{no}: {out}");
        }
    }

    #[test]
    fn a_candidate_that_rejects_writes_is_left_out_and_listed_with_the_chip_duty() {
        let mut api = pc();
        rejects(&mut api, "h2", 50);
        let out = go(&api, "\n");
        assert!(
            out.contains("System Fan #2 rejects writes (chip stays at 50%): left out"),
            "{out}"
        );
        let saved = api.saved.borrow().clone().unwrap();
        assert_eq!(saved["case_fans"], json!(["h1"]));
        let summary = out.split("Roles to save:").nth(1).unwrap();
        assert!(!summary.contains("h2"), "{out}");
    }

    #[test]
    fn when_no_candidate_lands_there_is_no_case_fans_role() {
        let mut api = pc();
        rejects(&mut api, "h1", 50);
        rejects(&mut api, "h2", 50);
        go(&api, "\n");
        let saved = api.saved.borrow().clone().unwrap();
        assert!(saved.get("case_fans").is_none(), "{saved}");
        assert_eq!(saved["cpu_fan"], "h0");
    }

    #[test]
    fn a_candidate_that_cannot_be_checked_is_left_out_with_the_reason() {
        let mut api = pc();
        api.per_fan.insert(
            "h1".into(),
            Err("the safety latch holds every fan at 100%".into()),
        );
        let out = go(&api, "\n");
        assert!(
            out.contains("System Fan #1 could not be checked (the safety latch holds every fan at 100%): left out"),
            "{out}"
        );
        assert_eq!(
            api.saved.borrow().clone().unwrap()["case_fans"],
            json!(["h2"])
        );
    }

    #[test]
    fn y_and_a_bare_enter_save_and_n_q_or_the_end_of_input_save_nothing() {
        for yes in ["\n", "y\n", "Y\n"] {
            let api = pc();
            go(&api, yes);
            assert!(api.saved.borrow().is_some(), "{yes:?}");
        }
        for no in ["n\n", "q\n", ""] {
            let api = pc();
            go(&api, no);
            assert!(api.saved.borrow().is_none(), "{no:?}");
            assert_eq!(*api.accepted.borrow(), 0, "{no:?}");
        }
    }

    #[test]
    fn a_fan_already_in_another_role_is_not_added_and_an_existing_group_grows() {
        let mut api = pc();
        api.identity["roles"] = json!({"cpu_fan": "h1", "case_fans": ["h2"]});
        api.identity["proposals"] = json!([
            proposal("case_fans", "h1", "a fan header"),
            proposal("case_fans", "h2", "a fan header"),
            proposal("case_fans", "h3", "a fan header"),
        ]);
        go(&api, "\n");
        let saved = api.saved.borrow().clone().unwrap();
        assert_eq!(saved["cpu_fan"], "h1");
        assert_eq!(saved["case_fans"], json!(["h2", "h3"]));
        assert_eq!(
            *api.probed.borrow(),
            ["h3"],
            "only the new candidate is probed"
        );
    }

    #[test]
    fn what_load_found_is_printed_once_per_line() {
        let mut api = pc();
        api.identity["problems"] =
            json!(["role 'cpu_temp' is not mapped (profiles: balanced, gaming)"]);
        let out = go(&api, "n\n");
        assert_eq!(out.matches("Problem: role 'cpu_temp'").count(), 1, "{out}");
    }

    #[test]
    fn nothing_to_do_changes_nothing() {
        let mut api = pc();
        mapped(&mut api);
        let out = go(&api, "");
        assert!(out.contains("Nothing changed"), "{out}");
        assert!(api.probed.borrow().is_empty() && api.saved.borrow().is_none());
    }

    #[test]
    fn a_differing_inventory_can_be_accepted_on_its_own() {
        let mut api = pc();
        mapped(&mut api);
        api.identity["inventory"]["missing"] = json!(["gone"]);
        go(&api, "y\n");
        assert_eq!(*api.accepted.borrow(), 1);
        assert!(api.saved.borrow().is_none());
    }

    fn probe_out(api: &Fake, fan: &str) -> Result<String, CliError> {
        let mut out = Vec::new();
        run_probe(api, fan, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn probe_reports_the_pairing_for_one_fan_and_saves_nothing() {
        let api = pc();
        let out = probe_out(&api, "h1").unwrap();
        assert_eq!(*api.probed.borrow(), ["h1"]);
        assert!(out.contains("its own tach rose (500 -> 1300 RPM)"), "{out}");
        assert!(out.contains("System Fan #1"), "{out}");
        assert!(api.saved.borrow().is_none() && *api.accepted.borrow() == 0);
    }

    #[test]
    fn probe_also_says_whether_the_write_landed() {
        let mut api = pc();
        assert!(probe_out(&api, "h1")
            .unwrap()
            .contains("the write landed (the chip reads 100%)"));
        rejects(&mut api, "h1", 50);
        let out = probe_out(&api, "h1").unwrap();
        assert!(out.contains("rejects writes (chip stays at 50%)"), "{out}");
    }

    #[test]
    fn probe_says_when_another_tach_rose_or_none_did_and_passes_an_error_on() {
        let mut api = pc();
        api.probe_result =
            json!({"rpm_before": [], "rpm_after": [], "pairing": {"result": "other", "fan": "h2"}});
        assert!(probe_out(&api, "h1")
            .unwrap()
            .contains("tach of 'h2' rose instead"));
        api.probe_result =
            json!({"rpm_before": [], "rpm_after": [], "pairing": {"result": "none"}});
        assert!(probe_out(&api, "h1").unwrap().contains("no tach moved"));
        api.probe_error = Some("the safety latch holds every fan at 100%".into());
        assert!(probe_out(&api, "h1")
            .unwrap_err()
            .msg
            .contains("safety latch"));
    }
}
