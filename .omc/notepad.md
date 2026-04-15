# Notepad
<!-- Auto-managed by OMC. Manual edits preserved in MANUAL section. -->

## Priority Context
<!-- ALWAYS loaded. Keep under 500 chars. Critical discoveries only. -->
THERMALIQ Lane 2 (Multi-Provider AI) IN PROGRESS: Phase 1+2 complete (AIProvider trait, 5 implementations, agent_task integration). Phase 3 remaining: POST /api/settings/ai-provider endpoint to dynamically save/reload config. ~1h remaining on Lane 2.

## Working Memory
<!-- Session notes. Auto-pruned after 7 days. -->
### 2026-04-14 12:11
### 2026-04-15 00:40
2026-04-15: Full product roadmap mapped and saved. Phase 0 complete. Phase 1 (Windows) is active focus. Windows HAL decision: HWiNFO64 shared memory (Global\HWiNFO_SENS_SM2) as primary, WMI fallback. Avoid WinRing0 (Defender quarantine) and PawnIO (anti-cheat). NVML for NVIDIA GPU fan control. Full roadmap in docs/ROADMAP.md. All technical decisions locked and saved to project memory. Next session: start thermaliq-hal/src/windows.rs implementation.


## 2026-04-14 12:11
## ThermalIQ v0.2 session wrap-up (2026-04-14)
Commits pushed: 05ea9a2 (data layer), 5ca81c2 (community), 041b6df (CI), 60e3f30 (arch doc), 43c3cc9 (v0.2 — config/poll/agent/CLI)
32 tests passing. Repo: lulacoding/thermaliq main.
Next: real HAL implementations, fan calibration, remaining curve types, Tauri+React UI.


## MANUAL
<!-- User content. Never auto-pruned. -->

