//! API token generation and persistent storage.
//!
//! On first start, generates a 32-byte cryptographically random token, hex-
//! encodes it, and writes it to `$XDG_CONFIG_HOME/thermaliq/api.token` with
//! mode 0600 (owner read/write only). On subsequent starts, reads the existing
//! token from disk.
//!
//! The Tauri frontend reads this file at startup and injects the token as the
//! `Authorization: Bearer <token>` header on every API request. The CLI reads
//! it automatically for all mutation commands.

use anyhow::{Context, Result};
use rand::RngCore;
use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Returns the path where the API token is stored.
pub fn token_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq");
    config_dir.join("api.token")
}

/// Load the token from disk, generating and persisting it if missing.
pub fn load_or_create() -> Result<String> {
    let path = token_path();

    if path.exists() {
        let token = fs::read_to_string(&path)
            .with_context(|| format!("failed to read token from {}", path.display()))?;
        let token = token.trim().to_string();
        if token.len() == 64 {
            return Ok(token);
        }
        // Token file exists but is malformed — regenerate.
        tracing::warn!(path = %path.display(), "token file malformed, regenerating");
    }

    let token = generate_token();
    persist(&token, &path)?;
    tracing::info!(path = %path.display(), "generated new API token");
    Ok(token)
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn persist(token: &str, path: &PathBuf) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create config dir {}", dir.display()))?;
    }

    fs::write(path, token)
        .with_context(|| format!("failed to write token to {}", path.display()))?;

    // Restrict to owner read/write only (0600).
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to chmod 0600 {}", path.display()))?;
    }

    Ok(())
}
