---
name: Session 7 — Signing Procurement & Production Secrets
description: Wire SSL.com eSigner cert (and optional Apple) into release.yml; produce a signed Windows installer that verifies in Explorer
session: 7
target_date: 2026-05-04
duration_minutes: 30
risk: low
---

# Session 7 — Signing Live

## Why this session exists

Unsigned Windows installers trigger SmartScreen warnings ("Windows protected your PC — Don't run / Run anyway"). New EV certs build SmartScreen reputation immediately. This session:
1. Wires the eSigner cert (ordered in S0, ready ~3-5 business days later) into CI
2. Confirms a signed installer verifies in Windows Explorer
3. (Optional) Wires Apple notarization for macOS

By the end, every release artifact downstream is signed without further action.

## Prerequisites

- S0 cert order placed and **identity verified** (you got the SSL.com email confirming verification complete)
- 4 SSL.com eSigner secrets in your password manager:
  - `ESIGNER_USERNAME`
  - `ESIGNER_PASSWORD`
  - `ESIGNER_CREDENTIAL_ID`
  - `ESIGNER_TOTP_SECRET`
- (If Mac IN) Apple Developer Team ID, signing certificate exported as `.p12`, app-specific password
- S6 complete (release workflow proven, just needs signing)

## Step-by-step

### 7.1 Verify cert is actually ready

- [ ] Log in at https://www.ssl.com/account/
- [ ] Check Certificates → eSigner. Should show:
  - Status: Active / Issued
  - Expiry: 1+ year out
  - Credential ID visible
- [ ] If status is still "Pending verification": this session is blocked. Follow up with SSL.com support — typical SLA is 48h.

### 7.2 Add Windows signing secrets to GitHub

- [ ] Browse: https://github.com/lulacoding/thermaliq/settings/secrets/actions
- [ ] Add each secret (case-sensitive names — must match what `release.yml` reads):
  - `ESIGNER_USERNAME`
  - `ESIGNER_PASSWORD`
  - `ESIGNER_CREDENTIAL_ID`
  - `ESIGNER_TOTP_SECRET`
- [ ] Verify by browsing the secrets list — no values shown but names should be present

### 7.3 Update CodeSignTool reference in release.yml

The current workflow has a placeholder. Replace it.

- [ ] Get latest CodeSignTool from SSL.com:
  - URL: https://www.ssl.com/guide/esigner-codesigntool-command-guide/
  - Download `CodeSignTool-v1.3.2-windows.zip` (or current version)
- [ ] Compute SHA256:
  ```powershell
  Get-FileHash CodeSignTool-v1.3.2-windows.zip -Algorithm SHA256
  ```
- [ ] In `.github/workflows/release.yml`, find the placeholder (likely `REPLACE_WITH_SHA256` or similar). Replace with actual hash.
- [ ] Update the URL if version changed.
- [ ] Commit:
  ```
  git checkout -b signing-live
  git add .github/workflows/release.yml
  git commit -m "ci: pin CodeSignTool v1.3.2 SHA256"
  git push -u origin signing-live
  gh pr create --fill
  gh pr merge --squash
  ```

### 7.4 (Optional) Apple notarization secrets

**Skip this section if Mac OUT for v1.0.**

- [ ] Export Apple Developer ID Application certificate as `.p12`:
  - Keychain Access → My Certificates → "Developer ID Application: <name>" → right-click → Export
  - Set a strong password, save the `.p12`
- [ ] Base64-encode for GitHub secret:
  ```bash
  base64 -i developerID.p12 | pbcopy   # macOS, copies to clipboard
  ```
- [ ] Add GitHub secrets:
  - `APPLE_CERTIFICATE` = base64 string
  - `APPLE_CERTIFICATE_PASSWORD` = password used in export
  - `APPLE_ID` = your Apple ID email
  - `APPLE_PASSWORD` = app-specific password (NOT your Apple ID password — generate at appleid.apple.com → Security → App-specific passwords)
  - `APPLE_TEAM_ID` = 10-char team ID from S0
- [ ] Replace `REPLACE_WITH_TEAM_ID` sentinels in:
  - `thermaliq-helper/Info.plist` (if exists)
  - Any host-app Info.plist
  - Any entitlements files
- [ ] Commit, push, merge

### 7.5 Trigger signed test build

- [ ] Bump version: `1.0.0-test.2`
  - `Cargo.toml`, `tauri.conf.json`, `package.json`
- [ ] Commit, push, tag:
  ```
  git commit -am "chore: bump 1.0.0-test.2 for signed build"
  git push
  git tag v1.0.0-test.2
  git push origin v1.0.0-test.2
  ```
- [ ] Watch `.github/workflows/release.yml`:
  - Build job — should succeed (same as before)
  - Sign step (NEW) — should NOT fail this time
  - Tauri bundle upload — should attach signed `.exe` and `latest.json`
- [ ] If sign step fails:
  - Most common: TOTP_SECRET wrong format. SSL.com gives a base32 seed; some workflows expect raw OTP code (rotates every 30s). Read the workflow code carefully.
  - Second most common: CREDENTIAL_ID has a typo
  - Check Actions log for CodeSignTool stderr

### 7.6 Verify signed installer

- [ ] Download `ThermalIQ_1.0.0-test.2_x64-setup.exe` from the test.2 release
- [ ] On Windows desktop, right-click the installer → Properties → Digital Signatures tab
- [ ] Expect:
  - Signature list shows ONE entry
  - Name of signer: your organization name (matches SSL.com cert)
  - Click Details → "This digital signature is OK"
  - Countersignatures section: timestamp authority signature
- [ ] **The "Don't run" SmartScreen warning may still appear initially** — EV certs build reputation over the first ~10-50 installs. After that, the warning disappears. Don't panic.
- [ ] Run installer. Confirm UAC prompt shows organization name (not "Unknown publisher")

### 7.7 (Optional) Verify macOS notarization

**Skip if Mac OUT for v1.0.**

- [ ] Download `ThermalIQ.dmg` from test.2 release
- [ ] On macOS:
  ```bash
  spctl --assess --type install ThermalIQ.dmg
  ```
  - Expect: `accepted` + organization name
- [ ] If `rejected`:
  - Notarization didn't complete (workflow may report success but Apple's submission queue can lag)
  - Check `xcrun notarytool history` if you have local notary creds

## Mac-side prep (Claude does)

- [ ] PR with the placeholder swap (CodeSignTool SHA256) — ready to apply once you give me the hash
- [ ] If Mac OUT: PR stripping macOS code paths from `release.yml` to reduce CI minutes and noise
- [ ] Document expected `release.yml` secret names in `.claude/context/decisions.md` so future contributors don't guess

## Bugs to log if seen

- Sign step succeeds but installer Properties shows no signature → P0 (publishing path bug)
- TOTP rotation makes some builds fail and others succeed → P1 (race in workflow — needs retry logic)
- Signed installer triggers Defender as malware → false positive, escalate to Microsoft for whitelisting (https://www.microsoft.com/wdsi/filesubmission)

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `CodeSignTool: invalid credentials` | Wrong username/password | Verify in SSL.com account, re-add secret |
| `CodeSignTool: TOTP failed` | TOTP_SECRET wrong format | Should be base32 seed from SSL.com onboarding email |
| `CodeSignTool: SHA256 mismatch` | Tool was updated upstream | Recompute hash, update workflow |
| Signed installer rejected by SmartScreen | Reputation building | Normal for first ~30 installs; not a bug |
| Apple notarization stuck "in progress" >30 min | Apple queue lag | Wait; if >24h, contact Apple Dev Support |

## Done when

- [ ] eSigner secrets in GitHub
- [ ] CodeSignTool SHA256 pinned in workflow
- [ ] (If Mac IN) Apple secrets in GitHub, Team ID replaced in plists
- [ ] v1.0.0-test.2 release has signed installer
- [ ] Windows Explorer Properties shows valid signature with org name
- [ ] (If Mac IN) `spctl --assess` accepts the DMG

## Session log

```
Date executed:
Time taken:
SSL.com cert verified date:
test.2 release URL:
Signature org name as shown in Explorer: 
Apple notarization (if applicable): pass / fail / skipped
Bugs filed:
```

## What unblocks next

- S8 (beta) — first public artifact MUST be signed; otherwise reputation building never starts
- S10 (v1.0 ship) — same signing pipeline, no further action
