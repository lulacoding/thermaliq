---
name: Session 6 — Auto-Updater End-to-End
description: Generate Ed25519 signing keypair; cut a test release; verify updater downloads, verifies signature, installs new version
session: 6
target_date: 2026-05-02
duration_minutes: 60
risk: medium
---

# Session 6 — Auto-Updater End-to-End

## Why this session exists

Tauri's updater is a critical UX for a service-based app — without it, users stay on broken versions forever. But it's also fragile:
- Wrong key = silent signature failures
- Wrong endpoint URL = "no updates available" forever
- Missing `latest.json` schema = updater crashes
- App not restarting cleanly = user thinks update failed

This session validates the full chain: build → sign → publish → detect → download → verify → install → restart.

**Cuttable from v1.0:** if this session blocks, ship v1.0 without auto-updater and add it in v1.0.1. Document the decision and ship with a banner directing users to GitHub Releases.

## Prerequisites

- S1, S5 complete (working installer)
- GitHub Actions release workflow exists (`.github/workflows/release.yml`)
- Repo admin access (to add secrets)
- A clean inbox-check on the dev side (this session pushes test tags publicly)

## Step-by-step

### 6.1 Generate Ed25519 keypair

This is the signing key for updater bundles. **Generate ONCE, never regenerate** — old installs will reject anything signed with a new key.

- [ ] On Mac OR Windows (doesn't matter — the key file is portable):
  ```bash
  npx @tauri-apps/cli signer generate -w ~/thermaliq-updater.key
  ```
  - Prompts for password — pick something strong, save in password manager
  - Outputs:
    - Private key file: `~/thermaliq-updater.key`
    - Public key string: shown in terminal (base64)
- [ ] **Copy both immediately into password manager:**
  - Entry: "ThermalIQ updater signing key"
  - Field 1: Public key (base64 string)
  - Field 2: Private key file contents (`cat ~/thermaliq-updater.key | base64` or attach raw)
  - Field 3: Password used to encrypt the key

### 6.2 Add GitHub repo secrets

- [ ] In browser: https://github.com/lulacoding/thermaliq/settings/secrets/actions
- [ ] Add secret `TAURI_SIGNING_PUBLIC_KEY` = the public key string
- [ ] Add secret `TAURI_SIGNING_PRIVATE_KEY` = the full file contents (multi-line)
- [ ] Add secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = the encryption password
- [ ] **DELETE the local private key file** (`rm ~/thermaliq-updater.key`) — only GitHub secrets and your password manager hold it now

### 6.3 Wire pubkey into the app config

- [ ] In `thermaliq-ui/src-tauri/tauri.conf.json`, find `plugins.updater` (or similar)
- [ ] Set `pubkey` field to the public key string
- [ ] Confirm `endpoints` points to the right URL — typically:
  ```
  https://github.com/lulacoding/thermaliq/releases/latest/download/latest.json
  ```
- [ ] Commit:
  ```
  git checkout -b updater-test
  git add thermaliq-ui/src-tauri/tauri.conf.json
  git commit -m "chore: wire updater pubkey for v1.0"
  git push -u origin updater-test
  ```
  - Open PR, merge to main when ready (or merge directly if solo)

### 6.4 Cut v1.0.0-test.0 baseline

This is the version users will be "upgrading FROM."

- [ ] Bump version in three files (must be in lockstep):
  - `Cargo.toml` (workspace `[workspace.package].version`)
  - `thermaliq-ui/src-tauri/tauri.conf.json` (`version`)
  - `thermaliq-ui/package.json` (`version`)
  - All three: `1.0.0-test.0`
- [ ] Commit:
  ```
  git commit -am "chore: bump to 1.0.0-test.0 for updater test"
  git push
  ```
- [ ] Tag + trigger release:
  ```
  git tag v1.0.0-test.0
  git push origin v1.0.0-test.0
  ```
- [ ] Watch `.github/workflows/release.yml` in GitHub Actions tab
- [ ] On success: download `ThermalIQ_1.0.0-test.0_x64-setup.exe` from the release
- [ ] On Windows: uninstall any existing ThermalIQ (S5), then install test.0
- [ ] Verify it runs: tray icon, Dashboard works

### 6.5 Cut v1.0.0-test.1 (the upgrade)

- [ ] Bump version to `1.0.0-test.1` in the same three files
- [ ] Add a visible change so you can confirm the new version is running — e.g., change the window title or About dialog version string
- [ ] Commit, push:
  ```
  git commit -am "chore: bump to 1.0.0-test.1 (visible UI marker)"
  git push
  ```
- [ ] Tag:
  ```
  git tag v1.0.0-test.1
  git push origin v1.0.0-test.1
  ```
- [ ] Wait for GitHub Actions to complete release workflow
- [ ] Open the release page, verify it has:
  - `ThermalIQ_1.0.0-test.1_x64-setup.exe`
  - `latest.json` (THIS IS CRITICAL — without it, updater fails)
- [ ] `latest.json` should look like:
  ```json
  {
    "version": "1.0.0-test.1",
    "notes": "...",
    "pub_date": "2026-05-02T...",
    "platforms": {
      "windows-x86_64": {
        "signature": "<base64 sig>",
        "url": "https://github.com/.../ThermalIQ_1.0.0-test.1_x64-setup.exe"
      }
    }
  }
  ```
- [ ] **If `signature` is empty:** signing failed in CI. Check workflow logs for missing/wrong secrets. Block here, fix, retry.

### 6.6 Trigger update from test.0 → test.1

- [ ] On the Windows desktop, currently running test.0
- [ ] Launch the UI (or just wait — updater check usually fires on app start + every N minutes)
- [ ] Within ~30s, expect:
  - Update banner in UI: "Update available: 1.0.0-test.1"
  - OR a system notification
- [ ] Click "Install update":
  - New installer downloads (~30 MB)
  - Installer runs (may briefly show progress)
  - App restarts
- [ ] Verify new version:
  - Window title shows the visible marker you added in 6.5
  - About dialog shows `1.0.0-test.1`
  - Service is still running (`Get-Service ThermalIQ`)
- [ ] Check daemon was also updated (not just the UI):
  ```powershell
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/health -Headers @{Authorization="Bearer $tok"}
  ```
  - `version` field in response should be `1.0.0-test.1`

### 6.7 Negative test — wrong signature

- [ ] Manually edit `latest.json` (download, modify, host on a temp endpoint OR push a deliberately wrong test.2 release)
  - Easier alternative: skip this and trust that signature verification works. Document as "untested" — most production updates will be correctly signed.
- [ ] If you do test it:
  - Bump to `1.0.0-test.2` but tamper with the signature in the release manually
  - On test.1 install, attempt update
  - Expect: update fails with signature error, NO install happens, app keeps running on test.1
- [ ] Restore by cutting a clean `1.0.0-test.3` and ensuring update path works

## Mac-side prep (Claude does)

- [ ] Pre-staged version-bump diff (3 files synchronized) — single commit ready to apply
- [ ] Document the exact secrets payload format expected by `release.yml`
- [ ] Verify `release.yml` reads the right secret names; if mismatch, propose fix PR
- [ ] If `latest.json` schema differs from above (it varies between Tauri versions), update this doc

## Bugs to log if seen

- Updater check never fires → P0 (silent breakage)
- "No updates available" when newer version exists → P0 (URL or version-parsing bug)
- Signature verification fails on legitimate update → P0 (key mismatch)
- Update downloads but doesn't install → P1
- Update installs but app doesn't restart → P1 (`app.restart()` missing)
- Updater UI shows but install button does nothing → P0 (event wiring)

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `latest.json` missing from release | Workflow failed silent | Check Actions logs; usually missing pubkey secret |
| `signature` field empty in `latest.json` | Private key secret not loaded | Re-add `TAURI_SIGNING_PRIVATE_KEY` |
| "Signature invalid" in app | Pubkey in `tauri.conf.json` ≠ key used to sign | Re-paste pubkey from password manager, rebuild |
| Update banner never shows | Endpoint URL wrong, or version comparison broken | Hit endpoint manually, verify JSON; check version semver |
| Install silently succeeds, no restart | Missing `app.restart()` after install | Patch updater hook |
| Service version doesn't match UI version | Updater only updated UI binary, not daemon | Check installer bundles both; this is a packaging bug |

## Decision point — block or defer?

If this session can't complete by end of day 2026-05-02:
- **Block:** ≥3 hours into debugging, no clear path → mark v1.0 as "manual update only"
- **Defer:** ship v1.0 with a banner: "Auto-update coming in v1.0.1 — get the latest at github.com/lulacoding/thermaliq/releases"
- Update `status.md` with the decision

## Done when

- [ ] Ed25519 keypair generated and stored in password manager
- [ ] GitHub secrets added
- [ ] Pubkey wired into `tauri.conf.json`
- [ ] test.0 → test.1 update completed via UI flow
- [ ] New version verified running (UI version + daemon `/api/health` version)
- [ ] Service still functional post-update (sensors stream, fans controllable)

## Session log

```
Date executed:
Time taken:
test.0 release URL:
test.1 release URL:
Update flow latency: ___ s from launch to "update available"
Restart succeeded: yes / no
Daemon version post-update matches UI: yes / no
Bugs filed:
Decision (ship updater in v1.0 / defer to v1.0.1):
```

## What unblocks next

- S7 (signing live) — eSigner cert wire-up touches the same workflow
- S8 (beta) — beta testers will exercise the updater repeatedly
- S9 (triage) — fixes will be delivered via this updater chain
