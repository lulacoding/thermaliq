//! Where the daemon keeps its files and what a valid API token looks like (SPEC §6). The daemon
//! and `tq` both use this, so they cannot disagree about either.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "THERMALIQ_DATA_DIR";

/// Env override wins (dev, CI); else `%ProgramData%\ThermalIQ`.
pub fn data_dir_from(
    override_dir: Option<OsString>,
    program_data: Option<OsString>,
) -> io::Result<PathBuf> {
    if let Some(d) = override_dir.filter(|d| !d.is_empty()) {
        return Ok(d.into());
    }
    match program_data.filter(|d| !d.is_empty()) {
        Some(p) => Ok(PathBuf::from(p).join("ThermalIQ")),
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("%ProgramData% is not set; set {DATA_DIR_ENV}"),
        )),
    }
}

pub fn data_dir() -> io::Result<PathBuf> {
    data_dir_from(
        std::env::var_os(DATA_DIR_ENV),
        std::env::var_os("ProgramData"),
    )
}

/// The token if `raw` (a file's text or an env value) is 64 hex digits once trimmed.
pub fn parse_token(raw: &str) -> Option<String> {
    let s = raw.trim();
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let p = data_dir_from(Some("devdir".into()), Some("pd".into())).unwrap();
        assert_eq!(p, PathBuf::from("devdir"));
    }

    #[test]
    fn program_data_path() {
        let p = data_dir_from(None, Some("pd".into())).unwrap();
        assert_eq!(p, PathBuf::from("pd").join("ThermalIQ"));
    }

    #[test]
    fn no_dir_is_an_error_and_empty_override_is_ignored() {
        assert!(data_dir_from(None, None).is_err());
        let p = data_dir_from(Some("".into()), Some("pd".into())).unwrap();
        assert_eq!(p, PathBuf::from("pd").join("ThermalIQ"));
    }

    #[test]
    fn a_token_is_64_hex_digits_and_surrounding_whitespace_is_ignored() {
        let t = "ab".repeat(32);
        assert_eq!(parse_token(&format!(" {t}\r\n")), Some(t));
        for bad in ["", "short", &"zz".repeat(32), &"ab".repeat(33)] {
            assert_eq!(parse_token(bad), None, "{bad}");
        }
    }
}
