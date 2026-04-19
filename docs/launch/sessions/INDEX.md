---
name: ThermalIQ v1.0 Session Files Index
description: Per-session ultra-detailed execution guides for the v1.0 launch (May 15 2026)
type: project
last_validated: 2026-04-19
---

# Session Index — v1.0 Launch

Each file is a self-contained execution guide for one session in `launch-plan.md`. Open the file for the session you're starting, follow it top-to-bottom, check off boxes as you go.

| # | File | Date target | Duration | Risk |
|---|------|-------------|----------|------|
| 0 | [00-async-kickoff.md](00-async-kickoff.md) | 2026-04-19 | 10 min | Low |
| 1 | [01-windows-dev-env.md](01-windows-dev-env.md) | by 2026-04-22 | 90 min | Med |
| 2 | [02-hwinfo-real-reads.md](02-hwinfo-real-reads.md) | by 2026-04-24 | 60 min | Low |
| 3 | [03-real-fan-writes.md](03-real-fan-writes.md) | by 2026-04-26 | 60 min | **HIGH** |
| 4 | [04-tauri-ui-walkthrough.md](04-tauri-ui-walkthrough.md) | by 2026-04-28 | 75 min | Low |
| 5 | [05-service-install.md](05-service-install.md) | by 2026-04-30 | 45 min | Med |
| 6 | [06-auto-updater.md](06-auto-updater.md) | by 2026-05-02 | 60 min | Med |
| 7 | [07-signing-live.md](07-signing-live.md) | by 2026-05-04 | 30 min | Low |
| 8 | [08-beta-cut.md](08-beta-cut.md) | 2026-05-06 | 45 min | Low |
| 9 | [09-beta-triage.md](09-beta-triage.md) | 2026-05-07 → 12 | rolling | Med |
| 10 | [10-launch.md](10-launch.md) | 2026-05-15 | 30 min | Low |

## How to use

1. **Before starting a session:** open the file, read top-to-bottom once (5 min).
2. **Pre-flight:** confirm prerequisites listed at the top.
3. **Execute:** follow steps in order. Don't skip ahead.
4. **On failure:** scroll to "Troubleshooting" — most common failures are documented.
5. **At end:** record results in the "Session log" block at the bottom; commit it to git.
6. **Exit criteria:** all boxes in "Done when" must be checked before moving to the next session.

## Slack & risk

- **Total budget:** 26 days, 11 sessions, ~6 days slack.
- **Spend slack on:** S9 (beta triage). Most likely to overrun.
- **Cuttable:** S6 (auto-updater) — can ship in v1.0.1 if blocked.
- **Non-negotiable:** S1, S2, S3 (proves real hardware works).

## Mac-side prep I (Claude) own

For each session, I prepare scripts/templates ahead of time so you can paste-run on Windows. See "Mac-side prep" section in each file.
