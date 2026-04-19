---
name: Session 10 — v1.0.0 Launch
description: Final pre-flight, version bump, tag, promote draft to Latest, submit winget, post launch announcement
session: 10
target_date: 2026-05-15
duration_minutes: 30
risk: low
---

# Session 10 — v1.0.0 Ship

## Why this session exists

Cutting v1.0 is the moment "beta" becomes "stable." This affects:
- Updater behavior (beta users on `-beta.N` get auto-bumped to `1.0.0`)
- Discoverability (winget submission, GitHub "Latest" tag)
- Reputation (you're shipping a 1.0 — code-quality bar implicit)

**This session is mostly ceremony — the work is done.** If you find yourself debugging here, you skipped S9.

## Prerequisites

- S9 complete: 48h stability window closed clean, no open P0/P1
- Latest beta has ≥3 confirmed-working testers
- CHANGELOG up to date
- All "known issues" documented (already in beta release notes from S8)
- README reflects current feature set

## Step-by-step

### 10.1 Final pre-flight (15 min)

- [ ] On the Windows test desktop, install latest beta from GitHub (not local build)
- [ ] Run abbreviated smoke test:
  - [ ] `Get-Service ThermalIQ` → Running
  - [ ] `/api/health` → 200 with current beta version
  - [ ] Dashboard streams sensors
  - [ ] Switch profile silent → balanced → performance, fans respond
  - [ ] AI agent responds to a query (verify provider key still set)
  - [ ] Reboot → service auto-starts → dashboard shows live data
- [ ] If ANY of these fails: STOP. Loop back to S9. Don't ship.
- [ ] Open `lessons.md` — confirm all real-hardware findings either fixed or documented in known-issues
- [ ] Open `status.md` — confirm "Phase Dashboard" reflects v1.0 readiness
- [ ] Open `ROADMAP.md` — confirm post-v1.0 items have been moved to v1.1 column

### 10.2 Final docs sweep (10 min)

- [ ] `README.md`:
  - [ ] Install instructions point to v1.0 release
  - [ ] Screenshots are current
  - [ ] No "WIP" / "coming soon" badges on shipped features
  - [ ] Add launch date line
- [ ] `CHANGELOG.md`:
  - [ ] Replace `[Unreleased]` with `[1.0.0] — 2026-05-15`
  - [ ] Aggregate all `-beta.N` fixes into the v1.0 entry (or keep beta history visible — your call, but consolidate)
  - [ ] Add "Initial stable release" header
- [ ] `docs/ROADMAP.md`:
  - [ ] Mark Phases 0–3 as Complete
  - [ ] Add v1.1 / v1.2 sections for deferred work (macOS, Corsair iCUE, etc.)

### 10.3 Version bump (3 min)

- [ ] Bump to `1.0.0` (drop `-beta.N` suffix) in:
  - `Cargo.toml`
  - `thermaliq-ui/src-tauri/tauri.conf.json`
  - `thermaliq-ui/package.json`
- [ ] Commit:
  ```
  git checkout main && git pull
  git checkout -b release-1.0.0
  git commit -am "release: 1.0.0"
  git push -u origin release-1.0.0
  gh pr create --fill --title "Release 1.0.0"
  gh pr merge --squash
  git checkout main && git pull
  ```

### 10.4 Tag + workflow (5 min wait)

- [ ] Tag from main:
  ```
  git tag v1.0.0
  git push origin v1.0.0
  ```
- [ ] Watch `.github/workflows/release.yml`:
  - [ ] Build succeeds
  - [ ] Sign succeeds
  - [ ] Bundle uploads
  - [ ] `latest.json` generated with `version: "1.0.0"`
  - [ ] **Winget manifest** — for stable tags this should auto-submit if a `WINGET_PAT` secret exists. If not set up, the manifest file is just generated as an artifact for manual submission.

### 10.5 Promote draft to Latest

This is the moment beta users get bumped to stable.

- [ ] Browser: open the draft v1.0.0 release
- [ ] Edit release notes:
  - [ ] Use a clean v1.0 announcement (NOT the beta "what to test" framing)
  - [ ] Highlight the journey: "After N betas and M tester reports, ThermalIQ v1.0 is here"
  - [ ] Thank top contributors / testers by name (with permission)
- [ ] Check **"Set as the latest release"** ✅
- [ ] Uncheck "Set as a pre-release" ❌
- [ ] Click "Publish release"
- [ ] **Updater immediately starts pushing v1.0 to all installs.** Watch GitHub Actions / your error tracking for any spike in update failures.

### 10.6 Winget submission (if not auto)

- [ ] Check if v1.0.0 release has `manifests/` artifact attached
- [ ] If auto-submitted: search https://winstall.app/apps?q=thermaliq in ~24h to confirm
- [ ] If NOT auto-submitted, manual:
  ```bash
  # Download winget-create
  # Or use the YAML manifest from the release artifact
  winget-create submit --token $WINGET_PAT manifests/t/thermaliq/...
  ```
- [ ] Approval typically takes 24-48h via the Microsoft community repo
- [ ] Once approved: `winget install thermaliq` works for all Windows users

### 10.7 Launch announcement

Mac-side prep: I'll draft. You edit + post.

```markdown
🎉 **ThermalIQ v1.0 is here!**

After [N] beta releases and tester contributions from [community], the first stable version of ThermalIQ is shipping today.

**What is it?**
Open-source, AI-native thermal management for Windows. Real-time sensor monitoring, 10 fan curve types, multi-provider AI agent — all running locally on your machine.

**Get it:**
- Direct download: https://github.com/lulacoding/thermaliq/releases/tag/v1.0.0
- Winget (~24h): `winget install thermaliq`

**Why ThermalIQ?**
[1-2 sentences on motivation — your voice]

**What's next?**
v1.1 will bring [macOS / Corsair iCUE / NZXT CAM / whatever you committed to]. Tracker: https://github.com/lulacoding/thermaliq/blob/main/docs/ROADMAP.md

Thanks to all beta testers — this wouldn't ship without you. ❤️
```

- [ ] Post to:
  - [ ] r/buildapc (or wherever S8 announcement got traction)
  - [ ] r/overclocking
  - [ ] Hacker News Show HN (with care — read https://news.ycombinator.com/showhn.html)
  - [ ] r/programming (only if there's a technical angle worth sharing — HAL design, AI agent architecture, etc.)
  - [ ] Personal channels (X, Mastodon, blog)
  - [ ] Project Discord (pinned message)
- [ ] Monitor for 4+ hours. Reply to questions. Triage immediate bug reports as P0 candidates for v1.0.1.

### 10.8 Post-launch monitoring (rest of day)

- [ ] Watch for:
  - Spike in GitHub issues (expected — more eyes)
  - Updater failure reports (any sign of mass failure → roll back release to Pre-release status)
  - SmartScreen warnings (should be minimal after beta builds reputation)
  - Defender false positives
- [ ] Block 1-2 hours that evening for first-day issue triage

### 10.9 Update brain memory + status

Mac-side prep + you confirm:
- [ ] `status.md` Phase Dashboard → v1.0 SHIPPED
- [ ] `~/dev/brain/projects/thermaliq.md` → mark v1.0 complete with date
- [ ] MEMORY.md if any new patterns to capture
- [ ] Tag a final session log entry

### 10.10 Celebrate

- [ ] Tell someone IRL
- [ ] Take 24h off launch work (resist the urge to immediately start v1.1)

## Mac-side prep (Claude does)

- [ ] Pre-staged version-bump commit (3 files synchronized)
- [ ] Drafted release notes for v1.0 (clean, non-beta tone)
- [ ] Drafted launch announcement (you edit voice)
- [ ] Drafted `status.md` + `thermaliq.md` updates marking v1.0 shipped
- [ ] Confirmed `release.yml` winget auto-submit logic (or flagged manual path)

## Risks & rollback

| Symptom | Action |
|---------|--------|
| `latest.json` malformed → updater 500s | Roll back to "Pre-release" status; updater will see no stable version; cut `1.0.1` with fix |
| Mass install failures reported (>5 in first hour) | Same as above; roll back, fix, re-cut |
| Defender false positive spreading | Submit to https://www.microsoft.com/wdsi/filesubmission immediately; warn users in pinned issue |
| SmartScreen blocking installs | Should be rare with EV cert; if happening, it's a publishing pipeline bug — investigate |

**Rollback procedure:**
1. Edit v1.0.0 release on GitHub → uncheck "Latest" → check "Pre-release"
2. Beta users continue on whatever beta they had
3. Stable users on v1.0 stay on v1.0 (no downgrade)
4. Fix bug, cut v1.0.1, ship that as Latest

## Done when

- [ ] v1.0.0 released as Latest with signed installer + `latest.json`
- [ ] Pre-flight smoke test passed
- [ ] Docs (README, CHANGELOG, ROADMAP) reflect v1.0
- [ ] Launch announcement posted in ≥2 channels
- [ ] Winget submission either auto-confirmed or manually triggered
- [ ] First-day monitoring complete, no rollback triggered
- [ ] Status updated in brain memory

## Session log

```
Date executed:
Time taken:
v1.0.0 release URL:
Channels announced in:
Issues opened in first 4 hours: 
Rollback triggered: yes / no
Subjective reaction (1 line):
```

## What unblocks next

- v1.1 planning (Mac, Corsair, NZXT, frontend tests, etc.)
- Marketing & growth work
- Sleep
