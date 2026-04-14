# Stage 5: Public Release, Distribution & DevOps Gaps
**Status:** Complete | **Tier:** MEDIUM | **Model:** Sonnet

## Critical Blockers (Must fix before any public release)

1. **Hardware mock feature flag** — without `--features mock-hardware` in Cargo.toml, CI is impossible on any runner. Non-negotiable.
2. **Code signing setup** — unsigned artifacts = immediate Gatekeeper/SmartScreen rejection
3. **Apple Developer account ($99/yr) + notarization secrets** — before any macOS artifact distributed
4. **Tauri updater Ed25519 keypair** — generate with `tauri signer generate`, private key as GitHub secret, public key in `tauri.conf.json`
5. **Community health files** — GitHub shows warning banner without: SECURITY.md, CONTRIBUTING.md, CHANGELOG.md, CODE_OF_CONDUCT.md

## GitHub Repository Setup

### Required Files (none currently exist)
```
.github/
  ISSUE_TEMPLATE/bug_report.yml      # YAML form: OS, version, hardware model
  ISSUE_TEMPLATE/feature_request.yml
  ISSUE_TEMPLATE/config.yml          # disable blank issues, link to Discussions
  PULL_REQUEST_TEMPLATE.md           # checklist: daemon behavior change? signing affected?
  CODEOWNERS                          # * @yourusername, per-path for daemon/HAL
  dependabot.yml                      # weekly Cargo + GitHub Actions updates
  SECURITY.md                         # supported versions, private disclosure, CVE process
CONTRIBUTING.md                       # dev setup, mock-hardware mode, hardware test guide
CODE_OF_CONDUCT.md                    # Contributor Covenant 2.1 verbatim
CHANGELOG.md                          # keep-a-changelog format, start with [Unreleased]
LICENSE                               # MIT full SPDX text with year + name
```

### GitHub Discussions Categories
- Announcements (write: maintainers only)
- General
- Q&A (answers can be marked correct)
- Show and Tell (fan configs, hardware support reports)
- Ideas (pre-issue feature discussion)

### Branch Protection (main)
- Require PR before merging
- 1 approving review
- Dismiss stale reviews on new commits
- Required status checks: `ci/test (ubuntu)`, `ci/test (windows)`, `ci/test (macos)`, `clippy`, `cargo-deny`
- Require branches up to date before merge

## CI/CD Pipeline

### ci.yml (PR validation — fast, ~8-12 min with warm cache)
```yaml
matrix: [ubuntu-22.04, macos-latest, windows-latest]
steps:
  - rust-toolchain.toml (pinned stable version, e.g. 1.87.0)
  - sccache-action + rust-cache (50-70% build time reduction)
  - cargo test --all-features --features mock-hardware
  - cargo clippy -- -D warnings
  - cargo fmt --check
  - cargo-deny check licenses bans advisories
  - cargo-audit
```

### release.yml (artifact build — slow, ~25-40 min parallel)
```yaml
triggers: push of tag v*.*.*
matrix: [macos-13 (Intel), macos-latest (ARM), windows-latest, ubuntu-22.04]
steps:
  - Platform deps (libwebkit2gtk-4.1-dev on Linux)
  - Node.js + frontend build
  - Import signing keys from secrets
  - tauri-apps/tauri-action (builds + signs + uploads to GitHub Release)
  - macOS only: xcrun notarytool + xcrun stapler staple
post-build:
  - cargo publish thermaliq-cli (if crates.io-worthy)
  - Generate update JSON → Cloudflare Pages
  - Trigger winget-pkgs PR via winget-create
```

### Key Notes
- **Tauri cannot cross-compile** — native runners required per platform for GUI builds
- **Pin actions by commit SHA**, not tag (supply chain attack prevention)
- macOS runners cost 10x GitHub Actions minutes vs Linux — budget accordingly
- `cross` can cross-compile the standalone daemon/CLI (e.g., Linux ARM64 for Pi users)

## Versioning & Release Automation

- **Tool:** `release-plz` (not semantic-release — Node ecosystem mismatch)
- Workspace-level unified version: `[workspace.package] version = "x.y.z"`, crates inherit via `version.workspace = true`
- CHANGELOG format: keep-a-changelog (Added/Changed/Deprecated/Removed/Fixed/Security)
- Release PR created automatically from conventional commits → merge → tag → GitHub Release
- Single version across all binaries until post-v1.0

## Windows Distribution

### Code Signing
- EV cert ($300-500/yr) or OV cert ($70-200/yr) — both require HSM/cloud signing since June 2023
- **GitHub Actions cannot use USB HSM** → use cloud signing: SignPath.io (free OSS tier), SSL.com eSigner, or DigiCert KeyLocker
- EV still preferred — builds SmartScreen reputation faster in practice
- SmartScreen reputation is non-deterministic; expect warnings for first ~500-1000 downloads regardless

### SmartScreen Cold-Start Mitigation
- Sign every release artifact (MSI + internal exe)
- Submit to Microsoft Defender for Endpoint submission portal pre-release
- List on winget early — accelerates reputation building

### winget Submission Process
1. Build MSI, sign, upload to GitHub Release
2. `winget-create new <installer-url>` → auto-generates YAML manifest
3. PR to `microsoft/winget-pkgs` → automated validation → manual review (3-10 days, up to 4 weeks for new publishers)
4. Verified publisher ID after ~5 accepted packages

### What NOT to pursue
- **MSIX/Microsoft Store** — not feasible. Daemon's privileged hardware access incompatible with Store sandboxing. Remove from roadmap.

## macOS Distribution

### Required GitHub Actions Secrets
```
APPLE_CERTIFICATE              # base64-encoded Developer ID Application .p12
APPLE_CERTIFICATE_PASSWORD     # .p12 export password
APPLE_ID                       # Apple ID email
APPLE_ID_PASSWORD              # app-specific password from appleid.apple.com
APPLE_TEAM_ID                  # 10-char team ID from developer.apple.com
APPLE_SIGNING_IDENTITY         # "Developer ID Application: Name (TEAMID)"
```

### What Must Be Signed
- The .app bundle (all binaries within)
- Daemon binary if installed separately outside .app
- .dmg wrapper (recommended — avoids "unverified disk image" on macOS 15+)

### Notarization (use notarytool, NOT deprecated altool)
```bash
xcrun notarytool submit thermaliq.dmg \
  --apple-id $APPLE_ID --password $APPLE_ID_PASSWORD \
  --team-id $APPLE_TEAM_ID --wait
xcrun stapler staple thermaliq.dmg
```
CI should retry notarization up to 3 times with backoff (Apple service outages common).

### Homebrew Distribution
- Use **Cask** (not Formula) for GUI + daemon distribution
- Start with **private tap** (`brew tap yourusername/thermaliq`) — no review, iterate fast
- Submit to `homebrew/homebrew-cask` only after stable release + meaningful adoption
- **Mac App Store: NOT feasible** — SMC/IOKit entitlements blocked in sandboxed MAS apps

## Linux Distribution

### High Priority
**cargo-deb (.deb):**
- `[package.metadata.deb.systemd-units]` — auto-generates maintainer scripts for daemon lifecycle
- Ship `99-thermaliq.rules` udev file as asset
- Include systemd service file

**cargo-generate-rpm (.rpm):**
- Write `%post`/`%preun`/`%postun` scriptlets manually for daemon lifecycle

**AUR (thermaliq-bin):**
- Downloads pre-compiled binary, validates SHA256, installs + systemd unit
- Submit: SSH to `aur@aur.archlinux.org/thermaliq-bin.git`, push PKGBUILD + .SRCINFO
- No review queue — live immediately. Also maintain `thermaliq-git` optionally.

**install.sh:**
- Detect distro via /etc/os-release, select format, download + SHA256 verify, install, enable systemd unit
- Provide `--dry-run` flag. Never use raw `curl | bash` — add integrity verification.

### Medium Priority
**AppImage:** Use linuxdeploy. Include AppStream metainfo.xml. Submit to AppImageHub.

### Not Recommended
**Flatpak/Snap:** Sandboxing fundamentally incompatible with daemon hardware access model. `/dev/` and sysfs access requires broad device permissions that Flathub reviewers increasingly reject. Daemon lifecycle breaks in confinement model.

**NixOS nixpkgs:** Start with a `flake.nix` at repo root for early adopters. Submit to nixpkgs only after stable release (2-8 week review timeline).

## Auto-Update Strategy

- **Use Tauri's built-in updater** (Ed25519 signature verification, not Sparkle)
- **Endpoint:** Cloudflare Pages (thermaliq.dev/update/[platform]/[arch]/latest.json) — free, zero infrastructure
- **Alternative post-v1.0:** CrabNebula Cloud (staged rollouts, analytics, platform routing)

### Daemon Update While Running
The spec doesn't address this — critical gap:
1. Installer pre-install script stops daemon service
2. Binaries replaced
3. Post-install script starts new daemon
4. Tauri frontend relaunches via updater API

Platform specifics:
- Windows: `ServiceControl` WiX element for Windows Service stop/start
- macOS: daemon's launchd plist in `/Library/LaunchDaemons/` — custom post-install step
- Linux .deb/.rpm: systemd maintainer scripts handle automatically
- AppImage: NO daemon lifecycle management — known format limitation

## Dependency Security

```toml
# Add to CI:
cargo-audit          # RustSec advisory database, fail on any severity
cargo-deny           # licenses + banned crates + source allowlisting
cargo-auditable      # embed SBOM in compiled binary (cargo auditable build --release)
cargo-sbom           # generate SPDX/CycloneDX file per release artifact
```

### deny.toml key config
- `[licenses]`: OSI-approved only. Flag GPL transitive deps.
- `[bans]`: deny `openssl` — prefer `rustls`
- `[sources]`: allow only crates.io + explicit git deps

Watch for LGPL transitive deps in dynamically-linked components — requires compliance even in MIT OSS context.
