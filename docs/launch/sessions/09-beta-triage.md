---
name: Session 9 — Beta Triage Loop
description: Reproduce, fix, ship, repeat. 5-day rolling session that ends when no P0/P1 bugs remain and beta has been stable 48h
session: 9
target_date_range: 2026-05-07 to 2026-05-12
duration: rolling (~30-90 min/day)
risk: medium
---

# Session 9 — Beta Triage Loop

## Why this session exists

Real users will find bugs you can't predict. This session is the structured loop for handling them. Without it, you ship v1.0 with whatever bugs happened to surface first and miss the systemic ones.

**Goal:** drive open P0/P1 count to zero AND have 48 wall-clock hours pass with no new P0/P1 reports.

## Prerequisites

- S8 done (beta is live)
- Notification routing: GitHub mobile app + email for new issues
- Discord/Slack channel for tester comms (recommended)
- ~30-90 min/day blocked through May 12

## Daily loop

### 9.1 Morning triage (10-15 min)

- [ ] Open https://github.com/lulacoding/thermaliq/issues
- [ ] Filter: created in last 24h
- [ ] For each new issue:
  - [ ] Read once
  - [ ] Apply severity label (`P0` / `P1` / `P2` / `P3`):
    - **P0:** crashes daemon, data loss, security, fan-write failures, install failures, signature errors
    - **P1:** wrong sensor data, UI components fail, profile switch broken, autostart broken
    - **P2:** UX friction, missing affordance, cosmetic UI bugs
    - **P3:** feature requests, "nice to have"
  - [ ] Apply hardware label (`hw:nvidia`, `hw:amd`, `hw:intel`, `hw:laptop`, `hw:threadripper`, etc.)
  - [ ] Acknowledge with comment within 24h:
    > Thanks for the report — triaged as P1, will investigate this week. If you can attach `%APPDATA%\thermaliq\logs\daemon.log`, that helps a lot.

### 9.2 Reproduction batch (30-60 min)

Pick the highest-severity issue NOT YET reproduced.

- [ ] Read the issue carefully — note hardware specs, OS version, repro steps
- [ ] Try to reproduce on your test desktop. If you have access to similar hardware to the reporter, use that.
- [ ] **If you can reproduce:**
  - Comment: "Reproduced on <my hardware>. Investigating."
  - Continue to 9.3 (fix)
- [ ] **If you can't reproduce:**
  - Comment: "Couldn't reproduce on <my hardware>. Could you provide:
    - HWiNFO64 version + screenshot of Sensors window
    - `daemon.log` for the 30s before/after the issue
    - Output of `wmic cpu get name`, `wmic path win32_videocontroller get name`"
  - Tag as `needs-info`
  - Move on, don't block

### 9.3 Fix + commit + PR

- [ ] Branch from main:
  ```
  git checkout main && git pull
  git checkout -b fix/issue-NNN-short-description
  ```
- [ ] Write a failing test if at all possible (cheap insurance)
- [ ] Fix the bug
- [ ] Run local tests:
  ```
  cargo test --features mock-hardware
  cargo clippy
  ```
- [ ] Commit with reference to issue:
  ```
  git commit -am "fix: <description> (#NNN)"
  git push -u origin fix/issue-NNN-...
  ```
- [ ] PR (auto-fills from commit):
  ```
  gh pr create --fill
  ```
- [ ] Self-review the diff once
- [ ] Merge:
  ```
  gh pr merge --squash --delete-branch
  ```

### 9.4 Cut beta.N+1 (if any P0 fix landed today)

Don't cut a beta tag for every fix — batch them. Rule of thumb:
- Any P0 → cut new beta within 24h of merge
- ≥3 P1 fixes → cut new beta
- Only P2/P3 → wait, accumulate

To cut:
- [ ] Bump version: `1.0.0-beta.N+1` (3 files)
- [ ] Append CHANGELOG entry:
  ```
  ## [1.0.0-beta.N+1] — 2026-05-NN
  ### Fixed
  - <bug 1> (#issue1)
  - <bug 2> (#issue2)
  ```
- [ ] Commit, push:
  ```
  git commit -am "release: 1.0.0-beta.N+1"
  git push
  git tag v1.0.0-beta.N+1
  git push origin v1.0.0-beta.N+1
  ```
- [ ] Wait for release workflow
- [ ] Promote to Pre-release in GitHub UI
- [ ] Comment on each fixed issue:
  > Fixed in v1.0.0-beta.N+1. Auto-update should deliver it within ~30 min of launch. Please confirm and close if working.

### 9.5 Communicate

- [ ] Post short update in Discord/community channel after each beta:
  ```
  📦 v1.0.0-beta.N+1 out — fixes:
  - Sensor freeze on HWiNFO restart (#42)
  - Profile switch not persisted (#47)
  - AMD ADL fan write returning empty (#51)
  
  Auto-updater should deliver within 30 min. Restart the app to expedite.
  ```
- [ ] Update GitHub release notes if you missed anything
- [ ] **Don't disappear.** Even brief updates ("triaging today, no new fixes yet") keep testers engaged.

### 9.6 Track 48h stability window

You need 48 wall-clock hours with NO new P0/P1 to ship v1.0.

- [ ] Maintain a simple tracker (in `status.md` or a pinned note):
  ```
  P0 count: 0 (last opened: 2026-05-08)
  P1 count: 2 (last opened: 2026-05-09 — both have fixes in progress)
  Stability window: started 2026-05-09 22:00, target 2026-05-11 22:00
  ```
- [ ] Reset window any time a new P0/P1 lands
- [ ] When window completes: ready for S10

## Common beta issues to anticipate

Mac-side prep includes contingency scripts/notes for these likely-to-appear issues:

| Issue | Frequency | Fix complexity |
|-------|-----------|----------------|
| HWiNFO64 not in Sensors-only mode | High — first-run UX gap | Add detection + clearer dialog |
| NVML fan write requires admin, daemon installed per-user | High | Service must run as LocalSystem; verify in S5 |
| Intel iGPU appears in fan list with no controllable fans | Medium | Filter at HAL discovery |
| Laptop EC controllers not supported | Medium | Graceful degradation, "no writable fans" UI state |
| AMD Adrenalin updates break ADL handle | Low but visible | Auto-reconnect logic |
| Toast notifications spam (rate limit broken) | If S4 caught it, fix; if not, will surface | Cooldown bug |
| Custom curve resets on profile switch | Medium | Curve persistence (relates to S4 known issue) |
| Auto-update banner appears but install fails | Medium | Permission/elevation issue |

## Mac-side prep (Claude does — running)

- [ ] Triage assistant: I'll read each new issue as it lands, propose severity + repro hypothesis, draft acknowledgment comment for your review/post
- [ ] Code-read each reported bug, propose fix in a branch
- [ ] Maintain a triage table in `.claude/context/beta-triage.md`:
  ```
  | Issue | Severity | Reporter HW | Repro'd | Fix branch | Status |
  ```
- [ ] Pre-draft each beta.N+1 release notes from merged fixes

## Hard limits (when to push back)

- **Don't fix during triage:** scope creep features, refactors, "while I'm here" cleanups
- **Don't ship unfixed P0s** to satisfy timeline — slip v1.0 instead, document why
- **Don't argue with reporters** — even if they're wrong about the bug, they had a bad UX. Fix the UX or the docs.

## Done when

- [ ] All open P0 issues: closed
- [ ] All open P1 issues: closed OR explicitly deferred to v1.0.1 with comment
- [ ] 48h has passed with no new P0/P1
- [ ] Last beta tag is stable (≥3 testers confirmed working)
- [ ] CHANGELOG up to date

## Session log (running)

```
Day 1 (2026-05-07): triaged N issues, P0:_ P1:_ P2:_, betas cut: _
Day 2 (2026-05-08): ...
Day 3 (2026-05-09): ...
Day 4 (2026-05-10): ...
Day 5 (2026-05-11): ...
Day 6 (2026-05-12): stability window complete? yes/no
```

## What unblocks next

- S10 (v1.0 ship) — only when stability window closes clean
