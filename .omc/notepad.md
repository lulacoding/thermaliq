# Notepad
<!-- Auto-managed by OMC. Manual edits preserved in MANUAL section. -->

## Priority Context
<!-- ALWAYS loaded. Keep under 500 chars. Critical discoveries only. -->
THERMALIQ Week 1 UI Backend Integration COMPLETE ✅. All components wired: SettingsPanel (820f3b0), CurveEditor (70834fb), HardwareDetect (e94ccfb), ProfileManager (pre-wired), Dashboard (WebSocket live). Next: CalibrationWizard, AgentPanel API wiring. Windows HAL Phase 1 complete (NVML GPU fan writes). Target v1.0 May 6 (19 days).

## Working Memory
<!-- Session notes. Auto-pruned after 7 days. -->
### 2026-04-15 11:20pm
Week 1 UI backend wiring sprint completed. CurveEditor → daemon /api/curves/ai-generate and /api/curves/save. HardwareDetect → daemon /sensors and /fans endpoints. ProfileManager already wired to /api/profiles endpoints. Dashboard fully wired via useDaemon WebSocket hook to /ws/sensors. All components compile without new TypeScript errors.

## MANUAL
<!-- User content. Never auto-pruned. -->

