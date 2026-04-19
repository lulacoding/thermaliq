# ThermalIQ Code Signing Guide

All signing steps are **env-gated**: unsigned dev builds and CI PR builds work with zero secrets set.
Signing activates automatically when the relevant GitHub secrets are present on a tag push.

---

## Windows Code Signing

### Certificate options

| Provider | Type | Approx. cost | Lead time | SmartScreen benefit |
|----------|------|-------------|-----------|---------------------|
| SSL.com eSigner | EV (cloud HSM) | ~$249/yr | 1–3 business days | Immediate reputation |
| DigiCert | EV (USB token) | ~$499/yr | 3–5 business days | Immediate reputation |
| Sectigo | OV | ~$199/yr | 1–2 business days | Reputation builds over time |

**Recommendation:** SSL.com eSigner EV. It is cloud-based (no USB token needed in CI), works directly with the CodeSignTool workflow already wired in `release.yml`, and EV certificates bypass the SmartScreen "Unknown Publisher" warning immediately — important for v1.0 first impressions.

OV certificates do not get instant SmartScreen reputation and will show the warning until enough download volume accumulates. For a new app, EV is worth the extra cost.

### Procurement steps (SSL.com eSigner)

1. Go to <https://www.ssl.com/esigner/> and purchase an **EV Code Signing** certificate.
2. Complete identity verification (government ID + video call — typically 1 business day).
3. Once approved, log into the SSL.com dashboard and note:
   - Your **username** and **password**
   - Your **credential ID** (shown under "My eSigner Credentials")
   - Your **TOTP secret** (shown when setting up two-factor auth for eSigner API)

### Local signing setup

```powershell
# Download CodeSignTool
$DIR = "$env:USERPROFILE\CodeSignTool"
Invoke-WebRequest -Uri "https://www.ssl.com/download/codesigntool-for-windows/" `
  -OutFile "$env:TEMP\CodeSignTool.zip" -UseBasicParsing
Expand-Archive "$env:TEMP\CodeSignTool.zip" -DestinationPath $DIR -Force

# Sign a binary
$TOOL = (Get-ChildItem -Path $DIR -Filter "CodeSignTool.bat" -Recurse).FullName
& cmd /c "`"$TOOL`" sign" `
  "-username=YOUR_USERNAME" `
  "-password=YOUR_PASSWORD" `
  "-credential_id=YOUR_CREDENTIAL_ID" `
  "-totp_secret=YOUR_TOTP_SECRET" `
  "-input_file_path=`"path\to\thermaliq-daemon.exe`"" `
  "-override=true"
```

### Verify a signed Windows binary

```powershell
# Check signature and certificate chain
signtool verify /pa /v path\to\thermaliq-daemon.exe

# View full certificate details
signtool verify /pa /v /debug path\to\ThermalIQ_1.0.0_x64-setup.exe
```

---

## macOS Code Signing & Notarization

### Prerequisites

- **Apple Developer Program** membership: $99/year at <https://developer.apple.com/programs/>
- A Mac with Xcode command-line tools installed

### Certificate procurement

1. Log into <https://developer.apple.com/account> → Certificates, Identifiers & Profiles.
2. Create two certificates:
   - **Developer ID Application** — for signing the `.app` bundle and binaries
   - **Developer ID Installer** — for signing `.pkg` files (optional for DMG-only distribution)
3. Download and double-click each `.cer` file to import into your login keychain.
4. Export the **Developer ID Application** certificate as a `.p12` file:
   - Open Keychain Access → My Certificates → right-click → Export
   - Set a strong password; you will need it as `APPLE_CERTIFICATE_PASSWORD`
5. Base64-encode the `.p12` for CI:
   ```bash
   base64 -i DeveloperIDApplication.p12 | pbcopy
   # Paste the result as the APPLE_CERTIFICATE secret
   ```

### Local signing setup

```bash
# List available signing identities
security find-identity -v -p codesigning

# Set environment variables
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="your@apple.id"
export APPLE_ID_PASSWORD="xxxx-xxxx-xxxx-xxxx"   # app-specific password from appleid.apple.com
export APPLE_TEAM_ID="XXXXXXXXXX"

# Build and sign via Tauri
cd thermaliq-ui
npm run tauri build
```

### Generate an app-specific password

1. Go to <https://appleid.apple.com> → Sign-In and Security → App-Specific Passwords.
2. Generate one labeled "ThermalIQ CI notarization".
3. Store it as the `APPLE_ID_PASSWORD` secret (not your Apple ID login password).

### Hardened runtime entitlements

The build already passes `--target` to `tauri-action`. Tauri enables the hardened runtime automatically when `APPLE_SIGNING_IDENTITY` is set. If you need custom entitlements (e.g., for the privileged helper), add an entitlements file at `thermaliq-ui/src-tauri/entitlements.plist` and reference it in `tauri.conf.json` under `bundle.macOS.entitlements`.

### Verify a signed macOS build

```bash
# Check code signature
codesign -dv --verbose=4 ThermalIQ.app

# Check Gatekeeper acceptance
spctl -a -vvv ThermalIQ.app

# Check notarization ticket is stapled to DMG
xcrun stapler validate ThermalIQ_1.0.0_aarch64.dmg

# Inspect notarization log (replace UUID from notarytool output)
xcrun notarytool log <UUID> --apple-id "$APPLE_ID" \
  --password "$APPLE_ID_PASSWORD" --team-id "$APPLE_TEAM_ID"
```

---

## GitHub Actions — Secrets to Create

Navigate to the repository → Settings → Secrets and variables → Actions → New repository secret.

### Windows secrets

| Secret name | Value |
|-------------|-------|
| `ESIGNER_USERNAME` | SSL.com account username |
| `ESIGNER_PASSWORD` | SSL.com account password |
| `ESIGNER_CREDENTIAL_ID` | eSigner credential ID from SSL.com dashboard |
| `ESIGNER_TOTP_SECRET` | TOTP secret from eSigner two-factor setup |

### macOS secrets

| Secret name | Value |
|-------------|-------|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` (Developer ID Application) |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | Full identity string, e.g. `Developer ID Application: Your Name (XXXXXXXXXX)` |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_ID_PASSWORD` | App-specific password generated at appleid.apple.com |
| `APPLE_TEAM_ID` | 10-character team ID from developer.apple.com |

### Tauri updater secrets (already needed — add if not set)

| Secret name | Value |
|-------------|-------|
| `TAURI_SIGNING_PRIVATE_KEY` | Output of `tauri signer generate` (private key) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password chosen during key generation |

Generate the updater keypair once:
```bash
npx tauri signer generate -w ~/.tauri/thermaliq.key
# Public key goes in tauri.conf.json plugins.updater.pubkey
# Private key + password go in the secrets above
```

---

## How signing activates in CI

- **Windows**: The "Sign with ssl.com eSigner" step in `release.yml` checks `$env:ESIGNER_USERNAME`. If the secret is unset, it prints a warning and exits 0 — the build continues unsigned.
- **macOS import**: The "Import Apple signing certificate" step is gated on `secrets.APPLE_CERTIFICATE != ''`.
- **macOS notarize**: The "Notarize DMG with xcrun notarytool" step is gated on `env.APPLE_ID != ''`.
- **PR/CI builds**: The `ci.yml` workflow has no signing steps and never reads these secrets.

No changes to CI are needed after adding secrets — the gates already exist.

---

## Checklist before v1.0 launch

- [ ] Purchase SSL.com EV eSigner certificate; complete identity verification
- [ ] Join Apple Developer Program ($99/year) if not already enrolled
- [ ] Create Developer ID Application certificate; export as `.p12`
- [ ] Generate app-specific password at appleid.apple.com
- [ ] Add all 10 secrets listed above to the GitHub repository
- [ ] Push a test tag (e.g. `v0.9.9-rc1`) and verify the release workflow signs all artifacts
- [ ] Run `signtool verify /pa` on the Windows installer — confirm no errors
- [ ] Run `spctl -a -vvv ThermalIQ.app` — confirm "accepted" for both Intel + ARM builds
- [ ] Confirm DMG notarization ticket is stapled (`xcrun stapler validate`)
- [ ] Test the signed installer on a clean Windows 11 VM — SmartScreen should not warn (EV cert)
