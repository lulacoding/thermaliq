//! Data directory, atomic writes, config and API token (SPEC §6).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thermaliq_common::DaemonConfig;

pub const DATA_DIR_ENV: &str = "THERMALIQ_DATA_DIR";
const CONFIG_FILE: &str = "config.toml";
const TOKEN_FILE: &str = "token.txt";

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

/// Temp file in the same directory, then rename over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

pub fn load_config(dir: &Path) -> io::Result<DaemonConfig> {
    match fs::read_to_string(dir.join(CONFIG_FILE)) {
        Ok(s) => {
            DaemonConfig::from_toml(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DaemonConfig::default()),
        Err(e) => Err(e),
    }
}

pub fn save_config(dir: &Path, cfg: &DaemonConfig) -> io::Result<()> {
    let s =
        toml::to_string_pretty(cfg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&dir.join(CONFIG_FILE), s.as_bytes())
}

/// Reads `token.txt`, or creates it. A malformed token is replaced.
pub fn load_or_create_token(dir: &Path) -> io::Result<String> {
    let path = dir.join(TOKEN_FILE);
    if let Ok(s) = fs::read_to_string(&path) {
        let s = s.trim();
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(s.to_owned());
        }
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| io::Error::other(e.to_string()))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    write_atomic(&path, token.as_bytes())?;
    Ok(token)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

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
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"two");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_creates_missing_parent() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("sub").join("a.txt");
        write_atomic(&f, b"x").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"x");
    }

    #[test]
    fn config_missing_is_default_and_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load_config(d.path()).unwrap(), DaemonConfig::default());
        let c = DaemonConfig {
            api_port: 9000,
            ..DaemonConfig::default()
        };
        save_config(d.path(), &c).unwrap();
        assert_eq!(load_config(d.path()).unwrap(), c);
    }

    #[test]
    fn invalid_config_is_an_error_not_a_default() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("config.toml"), "api_port = 0\n").unwrap();
        assert!(load_config(d.path()).is_err());
    }

    #[test]
    fn token_is_created_once_and_reused() {
        let d = tempfile::tempdir().unwrap();
        let t = load_or_create_token(d.path()).unwrap();
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(load_or_create_token(d.path()).unwrap(), t);
        assert_ne!(
            load_or_create_token(tempfile::tempdir().unwrap().path()).unwrap(),
            t
        );
    }

    #[test]
    fn malformed_token_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("token.txt"), "short\n").unwrap();
        let t = load_or_create_token(d.path()).unwrap();
        assert_eq!(t.len(), 64);
        assert_eq!(
            fs::read_to_string(d.path().join("token.txt"))
                .unwrap()
                .trim(),
            t
        );
    }
}
