# ThermalIQ Implementation Guide

**Last Updated:** 2026-04-15 10:35 GMT  
**Current Focus:** Phase 3.1–3.3 PR #10 (awaiting merge); Phase 3.4 blocked  
**Roadmap Reference:** `docs/ROADMAP.md`

---

## 📊 Current Status Dashboard

| Phase | Feature | Status | Branch | PR | Notes |
|-------|---------|--------|--------|----|----|
| **Phase 0** | Foundation | ✅ COMPLETE | main | — | All core types, daemon, HAL, CLI, UI |
| **Phase 1** | Windows HAL | ✅ COMPLETE | main (f792c34) | — | HWiNFO64 shared memory, WMI fallback, NSIS, service |
| **Phase 2.1** | macOS Fan Writes | ✅ MERGED | main (ea2285f) | #8 MERGED | SMJobBless helper, Unix socket protocol |
| **Phase 2.2** | Linux RDNA3 GPU | ✅ COMPLETE | `feat/phase-2.2-linux-rdna3-fan-control` (5e492fd) | — | RDNA3 OverDrive fan curves, kernel 6.7+ |
| **Phase 2.3** | Laptop EC Support | ✅ COMPLETE | merged to main | — | ThinkPad, ASUS, NBFC integration |
| **Phase 2.4** | Custom Sensors | ✅ COMPLETE | merged to main | — | TimeAverage, Mix, File, Offset evaluation |
| **Phase 2.5** | Fan Calibration | ✅ COMPLETE | merged to main | — | 0→100% ramp, RPM table storage, CLI/UI |
| **Phase 2.6** | Hysteresis & DC Mode | ✅ COMPLETE | merged to main | — | Start%/stop%, DC fan mode (pwm1_enable=2) |
| **Phase 3.1** | Per-App Curves | ✅ COMPLETE | `feat/phase-3-ai-depth` (af46036) | #10 OPEN | sysinfo process matching, smooth ramp |
| **Phase 3.2** | NL Profile Gen | ✅ COMPLETE | `feat/phase-3-ai-depth` (af46036) | #10 OPEN | Claude API, TOML validation, CLI `tq profile create` |
| **Phase 3.3** | Schedule Profiles | ✅ COMPLETE | `feat/phase-3-ai-depth` (af46036) | #10 OPEN | Time+day matching, midnight wrap, `tq schedule list` |
| **Phase 3.4** | Battery State | 🔵 BLOCKED | — | — | Needs Phase 3.3 merged first |

---

## 🎯 Immediate Next Steps

### 1. **Merge Phase 2.1 (macOS Fan Writes)** ✅ DONE
- PR #8 merged to main (commit ea2285f)
- All CI checks passed, no conflicts

### 2. **Rebase Phase 3.1–3.3 onto main** ✅ DONE
- `feat/phase-3-ai-depth` rebased onto ea2285f (main)
- All 8 commits applied cleanly
- New base: commit af46036

### 3. **PR Phase 3.1–3.3 to main** ✅ DONE
- PR #10 created: "feat(daemon): Phase 3.1–3.3 — Per-app curves, NL profile generation, and schedule-based profiles"
- Status: OPEN, awaiting CI checks and review
- Once merged, unblocks Phase 3.4

### 4. **Implement Phase 3.4 (Battery State)** 🔵 BLOCKED
- Waiting for Phase 3.3 (PR #10) to merge
- Detection logic already scoped (GetSystemPowerStatus, IOPSCopyPowerSourcesInfo, /sys/class/power_supply)
- Implementation plan:
  - Add `[battery]` section to config: `on_battery = "silent" | plugged_in = "balanced"`
  - TOML parsing + AppState field
  - OS event subscription (not poll for speed)
  - Daemon integration point: `start_battery_monitor()`
  - Create `thermaliq-daemon/src/battery_monitor.rs` module
  - Wire into main.rs startup

---

## 📂 Feature Branch Map

```
origin/main (Phase 0–1 + Phase 2.1 merged)
├─ feat/macos-fan-writes-phase-2-1    [MERGED to main]
├─ feat/phase-2.2-linux-rdna3-fan-control  (Phase 2.2: RDNA3 GPU fans)  [ready to merge]
├─ feat/phase-3-ai-depth              (Phase 3.1–3.3: Per-app, NL, Schedule)  [PR #10 OPEN]
```

**Current Merge Status:**
1. ✅ 2.1 (macOS) → main (commit ea2285f)
2. ✅ 3.1–3.3 rebased onto main (commit af46036)
3. ⏳ 3.1–3.3 → main as PR #10 (awaiting CI + merge)
4. 🔵 Phase 3.4 blocked until PR #10 merges

---

## 📋 Key Files by Phase

| Phase | Key Files | Type |
|-------|-----------|------|
| 2.1 | `thermaliq-hal/src/macos.rs`, `thermaliq-helper/*`, `com.thermaliq.helper.plist` | HAL + Helper |
| 2.2 | `thermaliq-hal/src/linux.rs` (RDNA3 fan control) | GPU fans |
| 2.3 | `thermaliq-hal/src/linux.rs` (EC fan logic) | Laptop fans |
| 3.1 | `thermaliq-daemon/src/app_monitor.rs`, `thermaliq-daemon/src/poller.rs` | Process + ramp |
| 3.2 | `thermaliq-daemon/src/profile_gen.rs`, `thermaliq-daemon/src/api/routes.rs` | Claude API |
| 3.3 | `thermaliq-daemon/src/schedule_monitor.rs`, `thermaliq-common/src/config.rs` | Schedule eval |
| 3.4 | TBD: `thermaliq-daemon/src/battery_monitor.rs` | Power state |

---

## 🔗 Dependency Chain

```
Phase 1 (Windows HAL)
  └→ Phase 2.1 (macOS SMC writes)  ✅ [READY TO MERGE]
  └→ Phase 2.2 (RDNA3)            ✅ [READY TO MERGE]
  └→ Phase 2.3 (Laptop EC)        ✅ [READY TO MERGE]
      └→ Phase 3.1 (Per-app)      ✅ [READY TO MERGE]
      └→ Phase 3.2 (NL Profile)   ✅ [READY TO MERGE]
      └→ Phase 3.3 (Schedule)     ✅ [READY TO COMMIT + MERGE]
          └→ Phase 3.4 (Battery)  🔵 [BLOCKED: Waiting for 3.3]
```

---

## ✅ Verification Checklist (Per Phase)

Before merging each phase:

- [ ] `cargo build --features mock-hardware` succeeds on all platforms
- [ ] `cargo test --features mock-hardware` passes all tests
- [ ] `cargo clippy` has no warnings
- [ ] `cargo fmt` has been run
- [ ] New config types are in `thermaliq-common/src/config.rs`
- [ ] New daemon modules added to `thermaliq-daemon/src/main.rs`
- [ ] New API routes wired into `api/routes.rs`
- [ ] CLI commands added (if applicable) to `thermaliq-cli/src/main.rs`
- [ ] TOML schema documented (if config added)
- [ ] README updated if user-facing changes
- [ ] Commit message follows format: `feat(<area>): Phase X.<Y> <description>`

---

## 🚀 Build & Test Commands

```bash
# Full build (all platforms, mock-hardware)
cargo build --features mock-hardware

# Run all tests
cargo test --features mock-hardware

# Lint check
cargo clippy --all-targets --features mock-hardware -- -D warnings

# Format check
cargo fmt --check

# Build docs
cargo doc --no-deps --open
```

---

## 📖 Session Auto-Load

This file is **auto-loaded** at session start. To update:

1. After committing a phase: update the **Status** column
2. After completing a feature: update **Immediate Next Steps**
3. Before starting new work: verify **Current Status Dashboard** against `git log`

Update via:
```bash
# Update this guide
$EDITOR IMPLEMENTATION_GUIDE.md
```

---

## 🔍 How to Use This Guide

**At session start:**
1. Check the **Current Status Dashboard** — tells you what's done, what's in progress
2. Review **Immediate Next Steps** — start with task #1, don't skip ahead
3. Verify **Dependency Chain** — understand what unblocks what

**Before committing code:**
1. Run **Build & Test Commands** (copy/paste them)
2. Check **Verification Checklist** for your phase
3. Update **Status Dashboard** in this file

**When blocked or confused:**
1. Cross-reference **Key Files by Phase** to find the right file
2. Check **Merge Order** to understand why something isn't working
3. Review `docs/ROADMAP.md` for full phase details

---

## 📝 Last Session Summary

**What was completed:**
- Phase 3.3 (Schedule-based profiles) fully implemented
- 16 daemon tests passing, all clippy warnings resolved
- Working directory ready for commit

**What's blocking progress:**
- Phase 3.3 not yet committed to git
- Phase 2.1 PR #8 open but not merged (macOS SMJobBless)
- Phase 3.1–3.3 need rebasing onto main after 2.1 merges

**Next immediate action:**
```bash
# 1. Commit Phase 3.3
git add [files...]
git commit -m "feat(daemon): Phase 3.3 schedule-based profiles"

# 2. Check merge status of Phase 2.1
gh pr list | grep "2.1"

# 3. If 2.1 ready, merge it
gh pr merge 8 --squash

# 4. Rebase Phase 3.1–3.3 onto main
git fetch origin
git rebase origin/main feat/phase-3-ai-depth
```

---

**Questions?** Check `docs/ROADMAP.md` for full technical specs, or run `git log --oneline --graph --all` to visualize branch state.
