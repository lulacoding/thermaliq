//! Data directory lockdown, done when the service is installed (PORTING M9).
//!
//! A standard user must not be able to drop a `config.toml` or a profile the SYSTEM
//! service loads, and must not read the API token. Uses `icacls` with well-known SIDs
//! (locale-independent) because the workspace denies `unsafe` and the ACL APIs need it.
//! After locking, the result is read back and must be exactly what we meant to set.

use crate::store;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

const SYSTEM_SID: &str = "S-1-5-18";
const ADMINS_SID: &str = "S-1-5-32-544";
const SYSTEM: &str = "*S-1-5-18";
const ADMINS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";

const FULL: u32 = 0x1f_01ff;
const READ_DATA: u32 = 0x1;
// WRITE_DATA|APPEND|WRITE_EA|WRITE_ATTR|DELETE_CHILD|DELETE|WRITE_DAC|WRITE_OWNER|GENERIC_WRITE|GENERIC_ALL
const WRITE_BITS: u32 =
    0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

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

// ---- reading what is set ----

#[derive(Debug)]
pub struct Ace {
    pub kind: String,
    pub flags: String,
    pub mask: u32,
    /// SDDL name (`SY`, `BA`, `WD`, …) or a full SID.
    pub sid: String,
}

#[derive(Debug)]
pub struct Dacl {
    pub protected: bool,
    pub aces: Vec<Ace>,
}

impl Dacl {
    /// For error and assertion messages.
    pub fn show(&self) -> String {
        let aces: Vec<String> = self
            .aces
            .iter()
            .map(|a| format!("{}:{}:{}:{:#x}", a.kind, a.flags, a.sid, a.mask))
            .collect();
        format!("protected={} {}", self.protected, aces.join(" "))
    }
}

/// SDDL names SYSTEM, Administrators, Users and the built-in Administrator account (RID 500) by alias.
pub fn sddl_name(sid: &str) -> String {
    match sid {
        SYSTEM_SID => "SY".into(),
        ADMINS_SID => "BA".into(),
        "S-1-5-32-545" => "BU".into(),
        s if s.starts_with("S-1-5-21-") && s.ends_with("-500") => "LA".into(),
        s => s.into(),
    }
}

/// The SID behind an SDDL name, for `icacls /remove`. `None` for names we don't know.
fn sid_of(name: &str, installer_sid: &str) -> Option<String> {
    let known = match name {
        "SY" => SYSTEM_SID,
        "BA" => ADMINS_SID,
        "BU" => "S-1-5-32-545",
        "WD" => "S-1-1-0",
        "AU" => "S-1-5-11",
        "IU" => "S-1-5-4",
        "NU" => "S-1-5-2",
        "AN" => "S-1-5-7",
        "LS" => "S-1-5-19",
        "NS" => "S-1-5-20",
        "BG" => "S-1-5-32-546",
        "CO" => "S-1-3-0",
        "LA" if sddl_name(installer_sid) == "LA" => installer_sid,
        s if s.starts_with("S-1-") => s,
        _ => return None,
    };
    Some(known.to_owned())
}

fn rights_mask(rights: &str) -> io::Result<u32> {
    if let Some(h) = rights.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).map_err(io::Error::other);
    }
    rights.as_bytes().chunks(2).try_fold(0, |acc, c| {
        Ok(acc
            | match c {
                b"FA" => FULL,
                b"FR" => 0x12_0089,
                b"FW" => 0x12_0116,
                b"FX" => 0x12_00a0,
                _ => return Err(io::Error::other(format!("unknown right {rights}"))),
            })
    })
}

/// `icacls /save` writes the SDDL, which names trustees by SID: no locale issues.
pub fn read_dacl(path: &Path) -> io::Result<Dacl> {
    static N: AtomicU32 = AtomicU32::new(0);
    let out = std::env::temp_dir().join(format!(
        "thermaliq-{}-{}.sddl",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let saved = icacls(&[&path.to_string_lossy(), "/save", &out.to_string_lossy()])
        .and_then(|()| std::fs::read(&out));
    let _ = std::fs::remove_file(&out);
    let units: Vec<u16> = saved?
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    let bad =
        |what: &str| io::Error::other(format!("{what} in the SDDL of {}: {text}", path.display()));
    let sddl = text
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .find(|l| l.starts_with("D:"))
        .ok_or_else(|| bad("no DACL"))?;
    let sddl = sddl.split("S:").next().unwrap_or(sddl); // drop any SACL
    let (head, rest) = sddl[2..].split_once('(').ok_or_else(|| bad("no entries"))?;
    let aces = rest
        .trim_end_matches(')')
        .split(")(")
        .map(|a| {
            let f: Vec<&str> = a.split(';').collect();
            match f[..] {
                [kind, flags, rights, _, _, sid] => Ok(Ace {
                    kind: kind.into(),
                    flags: flags.into(),
                    mask: rights_mask(rights)?,
                    sid: sid.into(),
                }),
                _ => Err(bad("malformed entry")),
            }
        })
        .collect::<io::Result<_>>()?;
    Ok(Dacl {
        protected: head.contains('P'),
        aces,
    })
}

/// The owner SID of `dir` and everything under it, as `(path, sid)`.
pub fn owners(dir: &Path) -> io::Result<Vec<(PathBuf, String)>> {
    // GetAccessControl() needs no module (Get-Acl fails to load one under a nested shell).
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; $p=$env:TIQ_DIR; \
        foreach($i in @(Get-Item -LiteralPath $p -Force)+@(Get-ChildItem -LiteralPath $p -Recurse -Force)){ \
        '{0}|{1}' -f $i.GetAccessControl().GetOwner([System.Security.Principal.SecurityIdentifier]).Value, $i.FullName }";
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("TIQ_DIR", dir)
        .env_remove("PSModulePath")
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
    owners
        .iter()
        .filter(|(_, o)| ![SYSTEM_SID, ADMINS_SID, installer_sid].contains(&o.as_str()))
        .map(|(p, _)| p.clone())
        .collect()
}

// ---- checking it is exactly right ----

/// `(sddl name, full control?)`; otherwise read-only.
type Want = Vec<(String, bool)>;

/// Problems with `d` against `want`, and the SDDL names of the entries to remove to fix them.
fn check(d: &Dacl, want: &Want, inheriting: bool) -> (Vec<String>, Vec<String>) {
    let (mut problems, mut remove) = (Vec::new(), Vec::new());
    if !d.protected {
        problems.push("inheritance is not broken".to_owned());
    }
    for a in &d.aces {
        let right = want
            .iter()
            .find(|(s, _)| *s == a.sid)
            .is_some_and(|(_, full)| {
                if *full {
                    a.mask & FULL == FULL
                } else {
                    a.mask & READ_DATA != 0 && a.mask & WRITE_BITS == 0
                }
            });
        let flags_ok = !a.flags.contains("ID")
            && (!inheriting || (a.flags.contains("OI") && a.flags.contains("CI")));
        if a.kind != "A" || !right || !flags_ok {
            problems.push(format!(
                "unexpected entry {}:{}:{}:{:#x}",
                a.kind, a.flags, a.sid, a.mask
            ));
            if !remove.contains(&a.sid) {
                remove.push(a.sid.clone());
            }
        }
    }
    for (s, _) in want {
        if !d.aces.iter().any(|a| a.sid == *s) {
            problems.push(format!("missing entry for {s}"));
        }
    }
    (problems, remove)
}

type Removals = Vec<(PathBuf, String)>;

/// What is wrong with the locked dir, and which entries to remove from which path.
fn verify(dir: &Path, installer_sid: &str) -> io::Result<(Vec<String>, Removals)> {
    let token = dir.join("token.txt");
    let w = |s: &str, full| (s.to_owned(), full);
    let dir_want = vec![w("SY", true), w("BA", true), w("BU", false)];
    let token_want = vec![
        w("SY", true),
        w("BA", true),
        (sddl_name(installer_sid), false),
    ];
    let (mut problems, mut remove) = (Vec::new(), Vec::new());
    for (path, want, inheriting) in [(dir, dir_want, true), (token.as_path(), token_want, false)] {
        let (p, r) = check(&read_dacl(path)?, &want, inheriting);
        problems.extend(p.into_iter().map(|p| format!("{}: {p}", path.display())));
        remove.extend(r.into_iter().map(|r| (path.to_path_buf(), r)));
    }
    for (p, o) in owners(dir)? {
        if o != ADMINS_SID {
            problems.push(format!(
                "{} is owned by {o}, not Administrators",
                p.display()
            ));
        }
    }
    Ok((problems, remove))
}

// ---- locking ----

fn lock_once(dir: &Path, installer_sid: &str) -> io::Result<()> {
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
    // Users are added after the token exists, so it never inherits a Users entry.
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
    icacls(&[&d, "/grant:r", &format!("{USERS}:(OI)(CI)RX")])?;
    // The previous owner keeps WRITE_DAC and could undo all of this.
    icacls(&[&d, "/setowner", ADMINS, "/T", "/C"])
}

/// Creates `dir` if needed and locks it: inheritance broken, SYSTEM and Administrators full
/// control, Users read-only, owner Administrators. Creates `token.txt` readable only by SYSTEM,
/// Administrators and `installer_sid`. Reads the result back and fails unless it is exactly that.
/// Safe to run again. An existing dir with anything owned by someone else (a standard user
/// could have planted a profile) is refused unless `force`.
pub fn lock_data_dir(dir: &Path, installer_sid: &str, force: bool) -> io::Result<()> {
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
    if dir.exists() && !force {
        let bad = untrusted_owners(&owners(dir)?, installer_sid);
        if !bad.is_empty() {
            let list: Vec<_> = bad.iter().map(|p| p.display().to_string()).collect();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} has items not owned by SYSTEM, Administrators or you, which the service would trust: {}. \
                     Check them, then rerun with --force to take them over",
                    dir.display(),
                    list.join(", ")
                ),
            ));
        }
    }
    for attempt in 0..3 {
        lock_once(dir, installer_sid)?;
        let (problems, remove) = verify(dir, installer_sid)?;
        if problems.is_empty() {
            return Ok(());
        }
        if attempt == 2 {
            return Err(io::Error::other(format!(
                "the data directory is not locked down as intended: {}",
                problems.join("; ")
            )));
        }
        for (path, name) in remove {
            let sid = sid_of(&name, installer_sid).ok_or_else(|| {
                io::Error::other(format!(
                    "cannot remove unknown entry {name} from {}",
                    path.display()
                ))
            })?;
            icacls(&[&path.to_string_lossy(), "/remove", &format!("*{sid}")])?;
        }
    }
    unreachable!("the loop returns on its last attempt")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn dacl(path: &Path) -> Dacl {
        read_dacl(path).unwrap()
    }

    fn sids(d: &Dacl) -> BTreeSet<&str> {
        d.aces.iter().map(|a| a.sid.as_str()).collect()
    }

    fn ace<'a>(d: &'a Dacl, sid: &str) -> &'a Ace {
        d.aces.iter().find(|a| a.sid == sid).unwrap()
    }

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

    type Locked = (tempfile::TempDir, PathBuf, String, Unlock);

    /// Installing needs an elevated token (Administrators must be active to write the locked
    /// dir). CI is elevated, so there a missing elevation fails; a dev shell skips.
    fn locked(files: &[(&str, &str)]) -> Option<Locked> {
        locked_after(files, |_| {})
    }

    /// `before` runs on the dir (files already written) just before it is locked.
    fn locked_after(files: &[(&str, &str)], before: impl FnOnce(&Path)) -> Option<Locked> {
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
        assert_eq!(sids(&d), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
        assert!(d
            .aces
            .iter()
            .all(|a| a.kind == "A" && a.flags.contains("OI") && a.flags.contains("CI")));
        for s in ["SY", "BA"] {
            assert_eq!(ace(&d, s).mask & FULL, FULL, "{s}");
        }
        let users = ace(&d, "BU").mask;
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
            sids(&t),
            BTreeSet::from(["SY", "BA", sid.as_str()]),
            "{}",
            t.show()
        );
        assert!(t.aces.iter().all(|a| !a.flags.contains("ID")));
        let me = ace(&t, &sid).mask;
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
        assert_eq!(sids(&c), BTreeSet::from(["SY", "BA", "BU"]), "{}", c.show());
        assert!(
            c.aces.iter().all(|a| a.flags.contains("ID")),
            "inherited from the dir"
        );
        assert_eq!(ace(&c, "BU").mask & WRITE_BITS, 0);
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
            sids(&t),
            BTreeSet::from(["SY", "BA", sid.as_str()]),
            "{}",
            t.show()
        );
        assert_eq!(sids(&d), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
    }

    #[test]
    fn a_bad_sid_is_refused_before_anything_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ThermalIQ");
        assert!(lock_data_dir(&dir, "everyone:F", false).is_err());
        assert!(!dir.exists());
    }

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
        assert_eq!(sids(&d), BTreeSet::from(["SY", "BA", "BU"]), "{}", d.show());
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
            sids(&t),
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
        let Some((_t, dir, sid, _g)) = locked(&[]) else {
            return;
        };
        let profiles = dir.join("profiles");
        fs::create_dir(&profiles).unwrap();
        let file = profiles.join("x.toml");
        fs::write(&file, "name = \"x\"").unwrap();
        // Users stand in for a standard account
        icacls(&[&file.to_string_lossy(), "/setowner", "*S-1-5-32-545"]).unwrap();
        let err = lock_data_dir(&dir, &sid, false).unwrap_err().to_string();
        assert!(err.contains("x.toml") && err.contains("--force"), "{err}");
        lock_data_dir(&dir, &sid, true).unwrap();
        assert_eq!(owners(&file).unwrap()[0].1, ADMINS_SID);
    }
}
