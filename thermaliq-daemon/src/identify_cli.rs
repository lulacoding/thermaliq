//! `thermaliq-daemon identify`: the first-run walk through the fans (SPEC §5.2). It talks to the
//! running daemon over the API, never to hardware: a probe is a request the engine carries out.

use crate::store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::time::Duration;
use thermaliq_common::identity::Heard;

/// A probe holds a fan for 5 s before it answers.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub trait Api {
    fn identity(&self) -> Result<Value, String>;
    fn probe(&self, fan: &str) -> Result<Value, String>;
    fn put_roles(&self, roles: &BTreeMap<String, String>) -> Result<(), String>;
    fn accept_inventory(&self) -> Result<(), String>;
}

pub struct HttpApi {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl HttpApi {
    pub fn new(port: u16, token: String) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            token,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(CALL_TIMEOUT))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into(),
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let auth = format!("Bearer {}", self.token);
        let sent = match (method, body) {
            ("GET", _) => self.agent.get(&url).call(),
            ("POST", None) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .send_empty(),
            ("POST", Some(b)) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b.to_string()),
            (_, b) => self
                .agent
                .put(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b.unwrap_or(Value::Null).to_string()),
        };
        let mut res = sent.map_err(|e| {
            format!(
                "cannot reach the daemon at {}: {e}. Is the ThermalIQ service running?",
                self.base
            )
        })?;
        let status = res.status().as_u16();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status >= 400 {
            return Err(value["error"]
                .as_str()
                .map_or_else(|| format!("HTTP {status}"), str::to_owned));
        }
        Ok(value)
    }
}

impl Api for HttpApi {
    fn identity(&self) -> Result<Value, String> {
        self.call("GET", "/identity", None)
    }
    fn probe(&self, fan: &str) -> Result<Value, String> {
        self.call("POST", "/identify/probe", Some(json!({ "fan": fan })))
    }
    fn put_roles(&self, roles: &BTreeMap<String, String>) -> Result<(), String> {
        self.call("PUT", "/roles", Some(json!({ "roles": roles })))
            .map(drop)
    }
    fn accept_inventory(&self) -> Result<(), String> {
        self.call("POST", "/inventory/accept", None).map(drop)
    }
}

/// `thermaliq-daemon identify`.
pub fn run_interactive() -> Result<(), String> {
    let dir = store::data_dir().map_err(|e| e.to_string())?;
    let port = store::load_config(&dir)
        .map_err(|e| format!("config.toml: {e}"))?
        .api_port;
    let token = store::load_token(&dir)
        .map_err(|e| format!("token.txt: {e}"))?
        .ok_or("token.txt is missing: run install-service first")?;
    run(
        &HttpApi::new(port, token),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
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

fn heard(answer: &str) -> Option<Heard> {
    match answer {
        "front" | "f" => Some(Heard::Front),
        "rear" | "r" | "back" => Some(Heard::Rear),
        "top" | "t" => Some(Heard::Top),
        "none" | "n" | "" => Some(Heard::None),
        _ => None,
    }
}

/// Walks the user through the fans. Saves only after a yes; the end of input or `q` saves nothing.
pub fn run(api: &dyn Api, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), String> {
    let mut t = Term { input, out };
    let v = api.identity()?;
    if let Some(b) = v["board"].as_str() {
        t.say(&format!("Board: {b}"));
    }
    for p in strings(&v["problems"]) {
        t.say(&format!("Problem: {p}"));
    }
    let mut roles: BTreeMap<String, String> = v["roles"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(r, i)| Some((r.clone(), i.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let before = roles.clone();

    for p in v["proposals"].as_array().into_iter().flatten() {
        let (Some(role), Some(id)) = (p["role"].as_str(), p["id"].as_str()) else {
            continue;
        };
        if !roles.contains_key(role) && !roles.values().any(|i| i == id) {
            t.say(&format!(
                "  {role} <- {id}  ({})",
                p["why"].as_str().unwrap_or("")
            ));
            roles.insert(role.to_owned(), id.to_owned());
        }
    }

    // A pump is never probed here: a probe only raises a fan, but a pump's role comes from its
    // kind or label, and what it cools is M20d's.
    for f in v["fans"].as_array().into_iter().flatten() {
        let (Some(id), Some(label)) = (f["id"].as_str(), f["label"].as_str()) else {
            continue;
        };
        if f["controllable"] != true || f["kind"] == "pump" || roles.values().any(|i| i == id) {
            continue;
        }
        t.say(&format!("\nFan '{label}' ({id}) has no role."));
        let Some(go) = t.ask("Run it at 100% for 5 s to find out what it is? [Y/n, q to stop]")
        else {
            t.say("Stopped. Nothing was saved.");
            return Ok(());
        };
        if go.starts_with('n') {
            continue;
        }
        match api.probe(id) {
            Ok(r) => t.say(&format!("  {}", pairing_line(&r, id))),
            Err(e) => {
                t.say(&format!("  could not probe it: {e}"));
                continue;
            }
        }
        let mut answer = None;
        for _ in 0..3 {
            let Some(a) = t.ask("Which fan got louder? [front/rear/top/none, q to stop]") else {
                t.say("Stopped. Nothing was saved.");
                return Ok(());
            };
            if let Some(h) = heard(&a) {
                answer = Some(h);
                break;
            }
            t.say("  please answer front, rear, top or none");
        }
        if let Some(role) = answer.and_then(Heard::role) {
            if let Some(old) = roles.insert(role.to_owned(), id.to_owned()) {
                t.say(&format!("  {role} was {old}; it is {id} now"));
            }
        }
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
            t.say(&format!("  {r} = {i}"));
        }
        if t.ask("Save these? [Y/n]")
            .is_none_or(|a| a.starts_with('n'))
        {
            t.say("Nothing was saved.");
            return Ok(());
        }
        api.put_roles(&roles)?;
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
        probed: RefCell<Vec<String>>,
        saved: RefCell<Option<BTreeMap<String, String>>>,
        accepted: RefCell<u32>,
    }

    impl Api for Fake {
        fn identity(&self) -> Result<Value, String> {
            Ok(self.identity.clone())
        }
        fn probe(&self, fan: &str) -> Result<Value, String> {
            self.probed.borrow_mut().push(fan.into());
            match &self.probe_error {
                Some(e) => Err(e.clone()),
                None => Ok(self.probe_result.clone()),
            }
        }
        fn put_roles(&self, roles: &BTreeMap<String, String>) -> Result<(), String> {
            *self.saved.borrow_mut() = Some(roles.clone());
            Ok(())
        }
        fn accept_inventory(&self) -> Result<(), String> {
            *self.accepted.borrow_mut() += 1;
            Ok(())
        }
    }

    fn fan(id: &str, label: &str, kind: &str) -> Value {
        json!({"id": id, "label": label, "kind": kind, "rpm": 800, "controllable": true})
    }

    fn pc() -> Fake {
        Fake {
            identity: json!({
                "board": "TEST BOARD",
                "problems": [],
                "roles": {},
                "unmapped_roles": ["cpu_fan", "pump", "case_front", "case_rear", "case_top", "gpu_fan", "cpu_temp", "gpu_temp"],
                "inventory": {"new": [], "missing": [], "changed": [], "board_changed": null},
                "proposals": [
                    {"role": "gpu_fan", "id": "nvml_GPU-a_fan0", "why": "an NVIDIA GPU fan"},
                    {"role": "cpu_fan", "id": "h0", "why": "its label says CPU"},
                ],
                "fans": [
                    fan("nvml_GPU-a_fan0", "GPU 0 fan 0", "fan"),
                    fan("h0", "CPU Fan", "fan"),
                    fan("h1", "System Fan #1", "fan"),
                    fan("h2", "System Fan #2", "fan"),
                    fan("p0", "Pump Fan", "pump"),
                ],
            }),
            probe_result: json!({
                "rpm_before": [["h1", 500]], "rpm_after": [["h1", 1300]],
                "pairing": {"result": "own"},
            }),
            ..Fake::default()
        }
    }

    fn go(api: &Fake, answers: &str) -> String {
        let mut out = Vec::new();
        run(api, &mut answers.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn it_proposes_what_it_is_sure_of_asks_about_the_rest_and_saves_after_a_yes() {
        let api = pc();
        let out = go(&api, "y\nfront\ny\nrear\ny\n");
        assert_eq!(*api.probed.borrow(), ["h1", "h2"]);
        let saved = api.saved.borrow().clone().unwrap();
        assert_eq!(saved["gpu_fan"], "nvml_GPU-a_fan0");
        assert_eq!(saved["cpu_fan"], "h0");
        assert_eq!(saved["case_front"], "h1");
        assert_eq!(saved["case_rear"], "h2");
        assert_eq!(*api.accepted.borrow(), 1);
        assert!(out.contains("its own tach rose (500 -> 1300 RPM)"), "{out}");
        assert!(out.contains("Restart the ThermalIQ service"), "{out}");
    }

    #[test]
    fn a_pump_and_a_fan_that_already_has_a_role_are_never_probed() {
        let api = pc();
        go(&api, "n\nn\n");
        assert!(!api.probed.borrow().iter().any(|f| f == "p0" || f == "h0"));
    }

    #[test]
    fn answering_none_leaves_the_fan_unmapped() {
        let api = pc();
        go(&api, "y\nnone\nn\ny\n");
        let saved = api.saved.borrow().clone().unwrap();
        assert!(!saved.values().any(|i| i == "h1"));
    }

    #[test]
    fn the_end_of_input_or_q_saves_nothing() {
        for answers in ["", "q\n", "y\nq\n", "y\nfront\ny\nq\n", "y\nfront\nn\n"] {
            let api = pc();
            go(&api, answers);
            assert!(api.saved.borrow().is_none(), "{answers:?}");
            assert_eq!(*api.accepted.borrow(), 0, "{answers:?}");
        }
    }

    #[test]
    fn declining_the_save_saves_nothing() {
        let api = pc();
        go(&api, "n\nn\nn\n");
        assert!(api.saved.borrow().is_none());
        assert_eq!(*api.accepted.borrow(), 0);
    }

    #[test]
    fn a_failed_probe_is_reported_and_the_fan_skipped() {
        let mut api = pc();
        api.probe_error = Some("the safety latch holds every fan at 100%".into());
        let out = go(&api, "y\ny\ny\n");
        assert!(
            out.contains("could not probe it: the safety latch"),
            "{out}"
        );
        let saved = api.saved.borrow().clone().unwrap();
        assert!(!saved.contains_key("case_front"));
    }

    #[test]
    fn a_tach_that_rose_on_another_fan_is_said_not_guessed() {
        let mut api = pc();
        api.probe_result =
            json!({"rpm_before": [], "rpm_after": [], "pairing": {"result": "other", "fan": "h2"}});
        let out = go(&api, "y\nn\nn\nn\n");
        assert!(out.contains("tach of 'h2' rose instead"), "{out}");
    }

    #[test]
    fn nothing_to_do_changes_nothing() {
        let mut api = pc();
        api.identity["roles"] = json!({"gpu_fan": "nvml_GPU-a_fan0", "cpu_fan": "h0", "case_front": "h1", "case_rear": "h2"});
        let out = go(&api, "");
        assert!(out.contains("Nothing changed"), "{out}");
        assert!(api.probed.borrow().is_empty() && api.saved.borrow().is_none());
    }

    #[test]
    fn a_differing_inventory_can_be_accepted_on_its_own() {
        let mut api = pc();
        api.identity["roles"] = json!({"gpu_fan": "nvml_GPU-a_fan0", "cpu_fan": "h0", "case_front": "h1", "case_rear": "h2"});
        api.identity["inventory"]["missing"] = json!(["gone"]);
        go(&api, "y\n");
        assert_eq!(*api.accepted.borrow(), 1);
        assert!(api.saved.borrow().is_none());
    }

    #[test]
    fn an_unusable_answer_is_asked_again_then_taken_as_none() {
        let api = pc();
        let out = go(&api, "y\nmiddle\nlounge\nattic\nn\ny\n");
        assert_eq!(
            out.matches("please answer front, rear, top or none")
                .count(),
            3
        );
        assert!(!api
            .saved
            .borrow()
            .clone()
            .unwrap()
            .contains_key("case_front"));
    }
}
