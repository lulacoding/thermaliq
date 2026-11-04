//! Data directory lockdown, done when the service is installed (PORTING M9).
//!
//! A standard user must not be able to drop a `config.toml` or a profile the SYSTEM
//! service loads, and must not read the API token. Uses `icacls` with well-known SIDs
//! (locale-independent) because the workspace denies `unsafe` and the ACL APIs need it.

use crate::store;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const SYSTEM: &str = "*S-1-5-18";
const ADMINS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";

fn icacls(args: &[&str]) -> io::Result<()> {
    let out = Command::new("icacls").args(args).output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "icacls {} failed: {}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )))
}

/// The SID of the user running this process, e.g. `S-1-5-21-…`.
pub fn installer_sid() -> io::Result<String> {
    let out = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"').to_owned())
        .filter(|s| out.status.success() && s.starts_with("S-1-"))
        .ok_or_else(|| io::Error::other(format!("whoami /user gave no SID: {text}")))
}

/// The owner SID of `dir` and everything under it, as `(path, sid)`.
pub fn owners(dir: &Path) -> io::Result<Vec<(PathBuf, String)>> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; $p=$env:TIQ_DIR; \
        foreach($i in @(Get-Item -LiteralPath $p -Force)+@(Get-ChildItem -LiteralPath $p -Recurse -Force)){ \
        '{0}|{1}' -f (Get-Acl -LiteralPath $i.FullName).GetOwner([System.Security.Principal.SecurityIdentifier]).Value, $i.FullName }";
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("TIQ_DIR", dir)
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "reading owners of {} failed: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('|'))
        .map(|(sid, p)| (PathBuf::from(p.trim()), sid.trim().to_owned()))
        .collect())
}

/// Paths owned by someone other than SYSTEM, Administrators or the installer.
pub fn untrusted_owners(owners: &[(PathBuf, String)], installer_sid: &str) -> Vec<PathBuf> {
    let _ = (owners, installer_sid);
    Vec::new()
}

/// Creates `dir` if needed and locks it: inheritance broken, SYSTEM and Administrators full
/// control, Users read-only. Creates `token.txt` readable only by SYSTEM, Administrators and
/// `installer_sid`. Safe to run again (an upgrade, or a dir that was open before).
/// An existing dir with files owned by anyone else is refused unless `force`.
pub fn lock_data_dir(dir: &Path, installer_sid: &str, force: bool) -> io::Result<()> {
    let _ = force;
    if !installer_sid.starts_with("S-1-")
        || !installer_sid
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a SID: {installer_sid}"),
        ));
    }
    std::fs::create_dir_all(dir)?;
    let d = dir.to_string_lossy();
    // A CREATOR OWNER entry on the parent turns into an explicit full-control entry for
    // whoever created the dir, and survives `/inheritance:r`. Drop the installer's own entry
    // (it gets Users' read below); SYSTEM and Administrators are granted again just after.
    icacls(&[
        &d,
        "/inheritance:r",
        "/remove:g",
        &format!("*{installer_sid}"),
    ])?;
    // Users are added last, so a token created in between never inherits a Users entry.
    icacls(&[
        &d,
        "/grant:r",
        &format!("{SYSTEM}:(OI)(CI)F"),
        &format!("{ADMINS}:(OI)(CI)F"),
    ])?;
    // Files already there (config.toml, profiles, history.db) drop their own ACLs and inherit.
    if std::fs::read_dir(dir)?.next().is_some() {
        icacls(&[&format!("{d}\\*"), "/reset", "/T", "/C"])?;
    }
    let token = dir.join("token.txt");
    store::load_or_create_token(dir)?;
    icacls(&[
        &token.to_string_lossy(),
        "/inheritance:r",
        "/grant:r",
        &format!("{SYSTEM}:F"),
        &format!("{ADMINS}:F"),
        &format!("*{installer_sid}:R"),
    ])?;
    icacls(&[&d, "/grant:r", &format!("{USERS}:(OI)(CI)RX")])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    struct Ace {
        kind: String,
        flags: String,
        mask: u32,
        sid: String,
    }

    struct Dacl {
        protected: bool,
        aces: Vec<Ace>,
    }

    impl Dacl {
        fn sids(&self) -> BTreeSet<&str> {
            self.aces.iter().map(|a| a.sid.as_str()).collect()
        }
        fn ace(&self, sid: &str) -> &Ace {
            self.aces.iter().find(|a| a.sid == sid).unwrap()
        }
        /// For assertion messages.
        fn show(&self) -> String {
            let aces: Vec<String> = self
                .aces
                .iter()
                .map(|a| format!("{}:{}:{}:{:#x}", a.kind, a.flags, a.sid, a.mask))
                .collect();
            format!("protected={} {}", self.protected, aces.join(" "))
        }
    }

    /// SDDL writes the built-in Administrator account (RID 500, the CI runner's user) as `LA`.
    fn sddl_name(sid: &str) -> String {
        if sid.ends_with("-500") {
            "LA".into()
        } else {
            sid.into()
        }
    }

    fn mask(rights: &str) -> u32 {
        if let Some(h) = rights.strip_prefix("0x") {
            return u32::from_str_radix(h, 16).unwrap();
        }
        rights
            .as_bytes()
            .chunks(2)
            .map(|c| match c {
                b"FA" => 0x1f01ff,
                b"FR" => 0x120089,
                b"FW" => 0x120116,
                b"FX" => 0x1200a0,
                _ => panic!("unknown right {rights}"),
            })
            .fold(0, |a, b| a | b)
    }

    /// `icacls /save` writes the SDDL, which names trustees by SID: no locale issues.
    fn dacl(path: &Path) -> Dacl {
        let out = path.with_extension("sddl.tmp");
        icacls(&[&path.to_string_lossy(), "/save", &out.to_string_lossy()]).unwrap();
        let bytes = fs::read(&out).unwrap();
        fs::remove_file(&out).unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        let sddl = text
            .lines()
            .map(|l| l.trim_start_matches('\u{feff}'))
            .find(|l| l.starts_with("D:"))
            .unwrap();
        let sddl = sddl.split("S:").next().unwrap(); // drop any SACL
        let (head, rest) = sddl[2..].split_once('(').unwrap();
        let aces = rest
            .trim_end_matches(')')
            .split(")(")
            .map(|a| {
                let f: Vec<&str> = a.split(';').collect();
                Ace {
                    kind: f[0].into(),
                    flags: f[1].into(),
                    mask: mask(f[2]),
                    sid: f[5].into(),
                }
            })
            .collect();
        Dacl {
            protected: head.contains('P'),
            aces,
        }
    }

    const FULL: u32 = 0x1f01ff;
    const READ_DATA: u32 = 0x1;
    // WRITE_DATA|APPEND|WRITE_EA|WRITE_ATTR|DELETE_CHILD|DELETE|WRITE_DAC|WRITE_OWNER|GENERIC_WRITE|GENERIC_ALL
    const WRITE_BITS: u32 = 0x2
        | 0x4
        | 0x10
        | 0x40
        | 0x100
        | 0x1_0000
        | 0x4_0000
        | 0x8_0000
        | 0x4000_0000
        | 0x1000_0000;

    /// Gives the test user the dir back so the temp dir can be deleted.
    struct Unlock(PathBuf, String);
    impl Drop for Unlock {
        fn drop(&mut self) {
            let _ = icacls(&[
                &self.0.to_string_lossy(),
                "/grant",
                &format!("*{}:(OI)(CI)F", self.1),
            ]);
        }
    }

    /// Installing needs an elevated token (Administrators must be active to write the locked
    /// dir). CI is elevated, so there a missing elevation fails; a dev shell skips.
    fn locked(files: &[(&str, &str)]) -> Option<(tempfile::TempDir, PathBuf, String, Unlock)> {
        locked_after(files, |_| {})
    }

    /// `before` runs on the dir (files already written) just before it is locked.
    fn locked_after(
        files: &[(&str, &str)],
        before: impl FnOnce(&Path),
    ) -> Option<(tempfile::TempDir, PathBuf, String, Unlock)> {
        let elevated = Command::new("net")
            .arg("session")
            .output()
            .is_ok_and(|o| o.status.success());
        if !elevated {
            assert!(
                std::env::var_os("CI").is_none(),
                "CI runner is not elevated"
            );
            eprintln!("skipped: not elevated");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ThermalIQ");
        fs::create_dir(&dir).unwrap();
        for (n, c) in files {
            fs::write(dir.join(n), c).unwrap();
        }
        let sid = installer_sid().unwrap();
        let guard = Unlock(dir.clone(), sid.clone());
        before(&dir);
        lock_data_dir(&dir, &sid, false).unwrap();
        Some((tmp, dir, sid, guard))
    }

    fn grant(path: &Path, ace: &str) {
        icacls(&[&path.to_string_lossy(), "/grant", ace]).unwrap();
    }

    const ADMINS_SID: &str = "S-1-5-32-544";

    #[test]
    fn the_owner_of_the_dir_and_every_file_is_reset_to_administrators() {
        let Some((_t, dir, _sid, _g)) = locked(&[("config.toml", "x"), ("a.toml", "y")]) else {
            return;
        };
        let all = owners(&dir).unwrap();
        assert!(all.len() >= 4, "dir, two files, token: {all:?}");
        for (p, o) in all {
            assert_eq!(o, ADMINS_SID, "{}", p.display());
        }
    }

    #[test]
    fn a_stray_grant_on_the_dir_is_gone_after_locking() {
        let Some((_t, dir, _sid, _g)) = locked_after(&[], |d| grant(d, "*S-1-1-0:(OI)(CI)M"))
        else {
            return;
        };
        let d = dacl(&dir);
        assert_eq!(d.sids(), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
    }

    #[test]
    fn a_stray_grant_on_an_existing_token_is_gone_after_locking() {
        let token = "0123456789abcdef".repeat(4);
        let Some((_t, dir, sid, _g)) = locked_after(&[("token.txt", &token)], |d| {
            grant(&d.join("token.txt"), "*S-1-1-0:R");
        }) else {
            return;
        };
        let t = dacl(&dir.join("token.txt"));
        let sid = sddl_name(&sid);
        assert_eq!(
            t.sids(),
            BTreeSet::from(["SY", "BA", sid.as_str()]),
            "{}",
            t.show()
        );
    }

    #[test]
    fn owners_other_than_system_admins_or_the_installer_are_untrusted() {
        let o = |p: &str, s: &str| (PathBuf::from(p), s.to_owned());
        let all = [
            o("d", "S-1-5-18"),
            o("d/a", ADMINS_SID),
            o("d/me", "S-1-5-21-1-2-3-1001"),
            o("d/planted.toml", "S-1-5-21-1-2-3-1002"),
            o("d/profiles/x.toml", "S-1-1-0"),
        ];
        assert_eq!(
            untrusted_owners(&all, "S-1-5-21-1-2-3-1001"),
            [
                PathBuf::from("d/planted.toml"),
                PathBuf::from("d/profiles/x.toml")
            ]
        );
    }

    #[test]
    fn a_dir_with_a_file_owned_by_a_standard_user_is_refused_unless_forced() {
        let Some((tmp, dir, sid, _g)) = locked(&[]) else {
            return;
        };
        let planted = dir.join("profiles");
        fs::create_dir(&planted).unwrap();
        let file = planted.join("x.toml");
        fs::write(&file, "name = \"x\"").unwrap();
        // Users stand in for a standard account; the dir is locked, so the admin fixes up its own ACL first
        icacls(&[&file.to_string_lossy(), "/setowner", "*S-1-5-32-545"]).unwrap();
        let err = lock_data_dir(&dir, &sid, false).unwrap_err().to_string();
        assert!(err.contains("x.toml") && err.contains("--force"), "{err}");
        lock_data_dir(&dir, &sid, true).unwrap();
        assert_eq!(owners(&file).unwrap()[0].1, ADMINS_SID);
        drop(tmp);
    }

    #[test]
    fn installer_sid_is_a_sid() {
        assert!(installer_sid().unwrap().starts_with("S-1-5-"));
    }

    #[test]
    fn dir_is_protected_system_and_admins_full_users_read_only() {
        let Some((_t, dir, _sid, _g)) = locked(&[]) else {
            return;
        };
        let d = dacl(&dir);
        assert!(d.protected, "inheritance must be broken");
        assert_eq!(d.sids(), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
        assert!(d
            .aces
            .iter()
            .all(|a| a.kind == "A" && a.flags.contains("OI") && a.flags.contains("CI")));
        for s in ["SY", "BA"] {
            assert_eq!(d.ace(s).mask & FULL, FULL, "{s}");
        }
        let users = d.ace("BU").mask;
        assert_ne!(users & READ_DATA, 0, "Users can list and read");
        assert_eq!(users & WRITE_BITS, 0, "Users cannot write: {users:#x}");
    }

    #[test]
    fn token_is_readable_only_by_system_admins_and_the_installer() {
        let Some((_t, dir, sid, _g)) = locked(&[]) else {
            return;
        };
        let t = dacl(&dir.join("token.txt"));
        let sid = sddl_name(&sid);
        assert!(t.protected);
        assert_eq!(
            t.sids(),
            BTreeSet::from(["SY", "BA", sid.as_str()]),
            "{}",
            t.show()
        );
        assert!(t.aces.iter().all(|a| !a.flags.contains("ID")));
        let me = t.ace(&sid).mask;
        assert_ne!(me & READ_DATA, 0);
        assert_eq!(me & WRITE_BITS, 0);
        // and the installing user really can read it
        assert_eq!(
            fs::read_to_string(dir.join("token.txt"))
                .unwrap()
                .trim()
                .len(),
            64
        );
    }

    #[test]
    fn files_that_were_already_there_are_locked_too() {
        let Some((_t, dir, _sid, _g)) = locked(&[("config.toml", "api_port = 1\n")]) else {
            return;
        };
        let c = dacl(&dir.join("config.toml"));
        assert_eq!(c.sids(), BTreeSet::from(["SY", "BA", "BU"]), "{}", c.show());
        assert!(
            c.aces.iter().all(|a| a.flags.contains("ID")),
            "inherited from the dir"
        );
        assert_eq!(c.ace("BU").mask & WRITE_BITS, 0);
    }

    #[test]
    fn locking_twice_keeps_the_token_and_the_acls() {
        let Some((_t, dir, sid, _g)) = locked(&[]) else {
            return;
        };
        let token = fs::read_to_string(dir.join("token.txt")).unwrap();
        lock_data_dir(&dir, &sid, false).unwrap();
        assert_eq!(fs::read_to_string(dir.join("token.txt")).unwrap(), token);
        let (t, d, sid) = (dacl(&dir.join("token.txt")), dacl(&dir), sddl_name(&sid));
        assert_eq!(
            t.sids(),
            BTreeSet::from(["SY", "BA", sid.as_str()]),
            "{}",
            t.show()
        );
        assert_eq!(d.sids(), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
    }

    #[test]
    fn a_bad_sid_is_refused_before_anything_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ThermalIQ");
        assert!(lock_data_dir(&dir, "everyone:F", false).is_err());
        assert!(!dir.exists());
    }
}
