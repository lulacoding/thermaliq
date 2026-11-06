//! The daemon's HTTP API, loopback only. Reads send no token; every write does.

use crate::{CliError, Exit};
use serde_json::Value;
use std::time::Duration;

/// A probe holds a fan for 5 s before the daemon answers.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

type Token = Box<dyn Fn() -> Result<String, CliError>>;

pub struct Client {
    base: String,
    agent: ureq::Agent,
    token: Token,
}

/// Escapes everything but unreserved characters, so an id can never change the path.
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl Client {
    /// `token` runs only when a call needs one, so reads work with no token file.
    pub fn new(port: u16, token: impl Fn() -> Result<String, CliError> + 'static) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(CALL_TIMEOUT))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into(),
            token: Box::new(token),
        }
    }

    pub fn with_token(port: u16, token: String) -> Self {
        Self::new(port, move || Ok(token.clone()))
    }

    pub fn token(&self) -> Result<String, CliError> {
        (self.token)()
    }

    pub fn get(&self, path: &str) -> Result<Value, CliError> {
        self.send("GET", path, None, false)
    }

    /// A read the daemon keeps behind the token.
    pub fn get_authed(&self, path: &str) -> Result<Value, CliError> {
        self.send("GET", path, None, true)
    }

    pub fn post(&self, path: &str, body: Option<Value>) -> Result<Value, CliError> {
        self.send("POST", path, body, true)
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value, CliError> {
        self.send("PUT", path, Some(body), true)
    }

    pub fn delete(&self, path: &str) -> Result<Value, CliError> {
        self.send("DELETE", path, None, true)
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        authed: bool,
    ) -> Result<Value, CliError> {
        let url = format!("{}{path}", self.base);
        let auth = authed
            .then(|| self.token().map(|t| format!("Bearer {t}")))
            .transpose()?;
        let sent = if matches!(method, "GET" | "DELETE") {
            let mut r = if method == "GET" {
                self.agent.get(&url)
            } else {
                self.agent.delete(&url)
            };
            if let Some(a) = &auth {
                r = r.header("Authorization", a);
            }
            r.call()
        } else {
            let mut r = if method == "POST" {
                self.agent.post(&url)
            } else {
                self.agent.put(&url)
            };
            if let Some(a) = &auth {
                r = r.header("Authorization", a);
            }
            match body {
                Some(b) => r
                    .header("Content-Type", "application/json")
                    .send(b.to_string()),
                None => r.send_empty(),
            }
        };
        let mut res = sent.map_err(|e| {
            CliError::new(
                Exit::Unreachable,
                format!(
                    "cannot reach the daemon at {}: {e}. Is the ThermalIQ service running?",
                    self.base
                ),
            )
        })?;
        let status = res.status().as_u16();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status < 400 {
            return Ok(value);
        }
        let said = value["error"]
            .as_str()
            .map_or_else(|| format!("HTTP {status}"), str::to_owned);
        Err(match status {
            401 => CliError::new(
                Exit::Token,
                format!("{said} (token.txt may be stale: run tq elevated or as the installing user, or set THERMALIQ_TOKEN)"),
            ),
            400..=499 => CliError::new(Exit::Rejected, said),
            _ => CliError::new(Exit::Error, said),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::enc;

    #[test]
    fn enc_keeps_unreserved_characters_and_escapes_the_rest() {
        assert_eq!(enc("lhm_nct6687d_0_fan-2.x~"), "lhm_nct6687d_0_fan-2.x~");
        assert_eq!(enc("a/b?c d"), "a%2Fb%3Fc%20d");
        assert_eq!(enc("é"), "%C3%A9");
    }
}
