---
name: Session 8 — Beta Release (v1.0.0-beta.1)
description: Cut the first public beta with signed artifacts; promote to Pre-release; post announcement; recruit testers
session: 8
target_date: 2026-05-06
duration_minutes: 45
risk: low
---

# Session 8 — Beta Cut

## Why this session exists

You can't catch every real-hardware bug alone — beta testers have hardware combinations you don't. This session ships the first version that real users install and use. Treat it like a launch (because socially it is).

## Prerequisites

- S1–S7 complete and green
- All bugs from S2–S5 either fixed or documented as "known issue, ship anyway"
- Signed-installer pipeline proven (S7)
- Updater pipeline proven (S6)
- A place to recruit testers (Discord, subreddit, forum, mailing list)

## Step-by-step

### 8.1 Pre-flight

- [ ] All open P0 bugs: closed or downgraded with justification
- [ ] All open P1 bugs: closed OR triaged into "v1.0.0 known issues" doc
- [ ] `lessons.md` reviewed; copy any user-facing items into release notes
- [ ] `CHANGELOG.md` updated with v1.0 highlights (see 8.2)
- [ ] `README.md` reflects v1.0 features (no stale "coming soon" notes)
- [ ] Test desktop has the latest test.N installed and working

### 8.2 Draft CHANGELOG.md entry

Mac-side prep: I'll draft this from `git log v0.2.0..HEAD`. Final form (you edit):

```markdown
## [1.0.0-beta.1] — 2026-05-06

### Highlights
- First public beta of ThermalIQ v1.0
- Windows desktop support (Win 10 22H2+ / Win 11)
- Real-hardware HAL via HWiNFO64 shared memory + WMI fallback
- 10 fan curve types: linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- AI agent with 3 modes (Passive / Approval / Autonomous)
- Multi-provider AI (Anthropic, OpenAI, Gemini, Ollama, OpenRouter)
- Auto-updater via signed releases
- System tray with quick profile switching
- Toast notifications for AI decisions

### Known issues
- macOS fan writes advisory-only (SMJobBless helper unsigned — coming v1.1)
- RDNA3 AMD cards need latest Adrenalin
- No Corsair iCUE / NZXT CAM integration (v1.1)
- Frontend has zero unit tests (tracked tech debt)
- AI profile/curve generation is Anthropic-only (multi-provider for chat only)

### Installation
Download `ThermalIQ_1.0.0-beta.1_x64-setup.exe` and run. Requires admin for fan control.

### Reporting bugs
GitHub Issues: https://github.com/lulacoding/thermaliq/issues
Include: hardware (CPU/GPU/mobo), HWiNFO64 version, reproduction steps.
```

### 8.3 Version bump

- [ ] Bump to `1.0.0-beta.1` in:
  - `Cargo.toml`
  - `thermaliq-ui/src-tauri/tauri.conf.json`
  - `thermaliq-ui/package.json`
- [ ] Add CHANGELOG entry (8.2)
- [ ] Commit:
  ```
  git checkout -b release-beta-1
  git commit -am "release: 1.0.0-beta.1"
  git push -u origin release-beta-1
  gh pr create --fill --title "Release 1.0.0-beta.1"
  gh pr merge --squash
  git checkout main && git pull
  ```

### 8.4 Tag + watch release workflow

- [ ] Tag from main:
  ```
  git tag v1.0.0-beta.1
  git push origin v1.0.0-beta.1
  ```
- [ ] Watch GitHub Actions:
  - Build (Windows): ~15 min
  - Sign: ~1 min
  - Bundle + upload: ~2 min
  - Generate `latest.json`: ~10s
  - (Optional) Mac build/notarize: 15-30 min
  - Winget manifest generation: should run but **NOT submit** (beta filter)
- [ ] Confirm release artifacts:
  - `ThermalIQ_1.0.0-beta.1_x64-setup.exe` (signed)
  - `latest.json` with non-empty signature
  - `(optional) ThermalIQ_1.0.0-beta.1_universal.dmg` (if Mac)
  - `SHA256SUMS.txt` (good practice; add if not present)

### 8.5 Promote to Pre-release

GitHub creates the release as draft. Do NOT mark it "Latest" yet — that triggers updater banners on stable users (none yet for v1.0, but the habit matters).

- [ ] Browser: https://github.com/lulacoding/thermaliq/releases
- [ ] Edit draft release for v1.0.0-beta.1
- [ ] Paste the CHANGELOG entry as release notes (also add a "What to test" section — see 8.6)
- [ ] Check **"Set as a pre-release"** ✅
- [ ] Uncheck "Set as the latest release" ❌
- [ ] Click "Publish release"

### 8.6 Add "What to test" guidance for beta users

Append to release notes:

```markdown
## What we want you to test

1. **Install** — does the NSIS installer complete cleanly? Any SmartScreen warnings?
2. **Reboot** — does ThermalIQ auto-start?
3. **Sensors** — open Dashboard. Do temps match HWiNFO64 / your other monitoring tools?
4. **Profiles** — switch between Silent / Balanced / Performance. Do fans respond?
5. **Curve editor** — drag a custom curve, save it. Survives restart?
6. **AI agent** — set up your AI provider, send a query. Get a useful answer?
7. **Auto-update** — when v1.0.0-beta.2 lands, does the updater notify you?

## How to report a bug

- Open https://github.com/lulacoding/thermaliq/issues/new
- Include: CPU model, GPU model + driver version, HWiNFO64 version, repro steps, screenshots
- Attach `%APPDATA%\thermaliq\logs\daemon.log` if relevant
```

### 8.7 Sanity-install the released artifact

Don't trust the build — install it like a real user.

- [ ] On Windows test desktop: uninstall any test.N
- [ ] Download `ThermalIQ_1.0.0-beta.1_x64-setup.exe` from the release page (not from local build dir)
- [ ] Run installer
- [ ] Walk through full S5 verify checklist (service registered, reboot survives, etc.)
- [ ] Walk through abbreviated S4 UI checklist (Dashboard, ProfileManager, AgentPanel)
- [ ] **If anything broken:** delete the release immediately (`gh release delete v1.0.0-beta.1 --cleanup-tag`), fix, re-cut as `1.0.0-beta.1.1` or `beta.2`

### 8.8 Recruit testers

- [ ] Draft announcement (Mac-side prep can do this for you):
  ```
  🚀 ThermalIQ v1.0.0-beta.1 is live!
  
  AI-native thermal management for Windows. Open source, self-hosted, multi-provider AI.
  
  ✅ Real-time sensor monitoring (CPU/GPU/mobo via HWiNFO64)
  ✅ 10 fan curve types
  ✅ AI agent (Passive / Approval / Autonomous)
  ✅ Multi-provider: Anthropic, OpenAI, Gemini, Ollama, OpenRouter
  ✅ Auto-updater
  
  Beta is signed (no SmartScreen drama after first few installs).
  
  Testers wanted — especially with:
  - AMD GPUs (RDNA2/3)
  - Threadripper / EPYC builds  
  - Multi-fan custom loops
  - Laptops with EC fan control
  
  Download: https://github.com/lulacoding/thermaliq/releases/tag/v1.0.0-beta.1
  Issues: https://github.com/lulacoding/thermaliq/issues
  ```
- [ ] Post to:
  - [ ] r/buildapc (read rules first; "promotion" rules are strict — frame as "looking for testers")
  - [ ] r/overclocking (more receptive to dev tools)
  - [ ] Hardware/PC enthusiast Discord servers you're in
  - [ ] Hacker News Show HN (only if you're confident in stability)
  - [ ] Personal social channels (X, Mastodon, blog)
- [ ] Monitor each post for the first 4 hours — engagement window matters

### 8.9 Set up triage cadence for S9

- [ ] Pin a GitHub issue: "v1.0.0-beta.1 testing — report here or open new issue"
- [ ] Watch for issues + emails for the next 48-72 hours
- [ ] Block 30 min/day for triage during S9

## Mac-side prep (Claude does)

- [ ] Draft `CHANGELOG.md` v1.0 entry from `git log v0.2.0..HEAD` (factual, no marketing)
- [ ] Draft "What to test" + announcement copy (you edit tone to match your voice)
- [ ] Verify `release.yml` beta-filter logic (winget submit should be SKIPPED for `-beta.N` tags)
- [ ] Code-read `latest.json` generation to confirm pre-release tags don't update stable users

## Bugs to log if seen

- Release workflow fails silently (no signed artifact) → P0
- `latest.json` for beta promotes to stable updater path → P0 (would push beta to all users)
- SmartScreen blocks signed installer entirely (not just warns) → P0 (cert problem)
- Winget manifest auto-submits despite beta filter → P0 (would push to winget Stable channel)

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Tag pushed but workflow didn't trigger | Workflow `on.push.tags` filter mismatch | Check `release.yml` triggers on `v*` |
| Signed installer but Defender removes it | Defender false positive on new EV cert | Submit to https://www.microsoft.com/wdsi/filesubmission for whitelisting |
| Reddit auto-removes post | "Promotion" rule | Re-frame as "looking for beta testers", less marketing-y |
| Issues flooding in faster than you can triage | Good problem | Set expectation in pinned issue: "responses within 48h" |

## Done when

- [ ] v1.0.0-beta.1 release published as Pre-release with signed installer
- [ ] CHANGELOG and release notes published
- [ ] Test installer downloaded fresh + verified clean install
- [ ] Announcement posted in ≥2 channels
- [ ] Pinned testing issue created
- [ ] First-day-pinned: respond to comments / first issues within 4h

## Session log

```
Date executed:
Time taken:
Release URL:
SmartScreen behavior on first install:
Channels announced in:
Issues opened in first 24h: 
Tester signups (rough count):
```

## What unblocks next

- S9 (triage) — feedback loop opens here
- S10 (v1.0 ship) — beta has to be stable for 48h before promoting to stable
