# Stage 1: FanControl & Competitor Feature Gap Analysis
**Status:** Complete | **Tier:** HIGH | **Model:** Sonnet

## Summary
Research conducted across 10+ tools (FanControl, HWiNFO64, SpeedFan, NBFC, argus-monitor, Macs Fan Control, iStatMenus, lm-sensors/fancontrol, TG Pro). 25 gaps identified.

---

## CRITICAL Gaps (Blocking parity claim)

### F1 — Temperature Alert / Alarm System
Every single competitor has this. ThermalIQ spec has none.

**Tools with this:** SpeedFan (sound/popup/beep), HWiNFO64 (threshold alerts + red highlighting + configurable sound), argus-monitor (S.M.A.R.T. warnings + threshold notifications), TG Pro (local notifications + email alerts), iStatMenus 7 (rules-based notifications), Macs Fan Control (temperature threshold alerts), lm-sensors (MAXTEMP config triggers shutdown events).

**Gap:** v0.2 spec describes the AI agent with passive/approval/autonomous modes, but no user-configurable alert system independent of the AI agent. Users need per-sensor thresholds with notification actions (desktop notification, sound, email, script execution) that fire regardless of AI agent state. The AI agent's "intervention" is not a substitute — it has latency and requires AI subsystem active.

FanControl itself lacks this (confirmed open feature request #807), but every mature competitor has it.

**Evidence:** FanControl GitHub Issue #807 "Feature Request: Alarms" confirms FanControl lacks it. HWiNFO docs, TG Pro docs, argus-monitor docs, SpeedFan FAQ all confirm native alert systems.

---

### F7 — Fan Stop / Zero RPM Mode (Start% and Stop%)
FanControl advertises this prominently — ThermalIQ spec only addresses it for NVIDIA GPUs.

**Tools with this:** FanControl (Start % = min speed when running; Stop % = speed below which fan stops — enables smooth 0-RPM for PWM fans that support it).

**Gap:** Spec mentions "NVIDIA 30% min PWM" and "0RPM mode" for NVIDIA GPUs, but no general Start%/Stop% parameter for arbitrary fan headers. Modern case fans (Noctua, be quiet!, Fractal) support 0-RPM mode but only if PWM drops below their stop threshold. Without these parameters per fan channel, ThermalIQ cannot enable silent 0-RPM operation on supported fans.

**Evidence:** FanControl docs: "Start %, Stop % — control the fan's start and stopping logic for smooth 0 RPM operation (when supported)."

---

### F10 — USB Fan Controller Hardware Support (Corsair, NZXT, Aquacomputer)
Any user with a Corsair AIO, NZXT Kraken, or Aquacomputer setup cannot use ThermalIQ at all.

**Tools with this:** FanControl via plugin ecosystem — Aquacomputer QUADRO/OCTO/HighFlowNext, Corsair Commander/Hydro AIOs, NZXT Kraken X2/X3, Thermaltake, liquidctl-compatible AIOs, LianLi L-Connect 3.

**Gap:** Spec mentions GPU handling via ADLX and SMC/IOKit, but no USB fan controller support. No hardware plugin API analogous to FanControl's .NET plugin system. Cannot add new hardware without forking the Rust backend.

**Evidence:** FanControl community plugins include 15+ hardware integrations. DeepWiki lists: "Aquacomputer HighFlowNext/Quadro/Octo, GPU-Z, Corsair Commander, Razer, HomeAssistant temperature sensors, NZXT Kraken X2/X3, LianLi L-Connect 3."

---

### F11 — Laptop Embedded Controller (EC) Support
Linux laptop users are a core ThermalIQ target. Without EC support, the Linux platform claim is false for most laptops.

**Tools with this:** NBFC / nbfc-linux — the entire tool is built around EC interaction. EC register mapping, model config database (community-contributed, 300+ models), auto-detection via dmidecode.

**Gap:** Spec makes no mention of embedded controller support for laptops. On laptops, the EC — not the motherboard SuperIO — controls fans. Without EC access, ThermalIQ cannot control laptop fans on the vast majority of non-Apple laptops on Linux. Spec mentions NBFC as a competitor but does not specify how to achieve parity.

**Evidence:** NBFC GitHub: "Config files define how NBFC interacts with your notebook's embedded controller (EC). The EC monitors your system's temperatures and controls the fans." nbfc-linux includes model config auto-detection via dmidecode.

---

### F15 — Apple Silicon Fan Write Access
Spec marks macOS as read-only; competitors have write access.

**Tools with this:** TG Pro (claims full M1-M5 support with override capability), Macs Fan Control (M1-M5 support). The unlock sequence is publicly documented (github.com/agoodkind/macos-smc-fan, January 2026).

**Gap:** Spec states macOS fan control is "SMC/IOKit read-only." This is a choice, not a technical limitation. The `Ftst=1` unlock mechanism suppresses `thermalmonitord`'s `LifetimeServoController` and enables write access. Shipping read-only macOS while competitors offer write control is a competitive handicap.

**Evidence:** GitHub agoodkind/macos-smc-fan: "thermalmonitord enforces 'protected mode'; writes to F0Md and F0Tg are blocked unless Ftst key is set to 1 first — the unlock mechanism." TG Pro: "Override System Control option enables complete control of fan speeds including lowering them on Apple Silicon."

---

### F20 — Hardware Plugin / Extension API
Architectural gap: without it, ThermalIQ hardware support is frozen at whatever the core team implements.

**Tools with this:** FanControl (.NET DLL drop-in system — IPlugin, IPlugin2, IPlugin3, IPluginSensor, IPluginControlSensor interfaces), lm-sensors (hwmon driver interface), HWiNFO64 (shared memory API).

**Gap:** Spec has a rich AI interface (GUI, CLI, HTTP API, Claude Code skill, OpenClaw webhook) but no hardware plugin API. FanControl's plugin system enables 15+ community hardware integrations. Without analogous Rust plugin API, the community cannot extend ThermalIQ.

**Evidence:** FanControl wiki: "a simple plugin system with .NET that allows any third party developer to add temperature, speed or control sensors; installing is as easy as dropping a dll in the plugin folder."

---

## IMPORTANT Gaps (Significant user value)

### F2 — Non-Temperature Sensor Fan Control (Load, Power Draw, Frequency)
**Gap:** Curve input sources are temperature-only. No CPU/GPU load %, TDP/watt draw, clock frequency, or memory bandwidth as curve input. Power enthusiasts need to spin fans up when GPU power spikes before temperature catches up — latency advantage over temp-based control.
**Evidence:** FanControl Issue #1527: "allow data from sensors other than temperature — CPU Package Power, GPU Core Load — to control fan speed."

### F3 — S.M.A.R.T. Monitoring and Drive Health Integration
**Gap:** Spec mentions disk temperature but no S.M.A.R.T. health monitoring, attribute tracking, failure prediction, or vendor-threshold alerting.
**Evidence:** Argus-monitor: "informs users when critical S.M.A.R.T. values change, provides warnings when attributes reach vendor-specified thresholds indicating likely drive failure within 24 hours."

### F4 — Temperature History Logging, Graphing, and Export
**CRITICAL for AI agent trust.** Without visible historical data, there is no auditability.
**Gap:** SQLite history stores AI agent interventions, not sensor telemetry. No user-facing time-series logging, no historical graph viewer, no CSV/JSON export of raw sensor data.
**Evidence:** FanControl Discussion #3206 and #2009 confirm this gap even in market leader. HWiNFO: "log sensor data over time for analysis and troubleshooting; export to CSV."

### F5 — RPM-Mode Fan Curves (Target RPM instead of % PWM)
**Gap:** All curve types described in % output, not RPM. No explicit mention that curves can output in RPM space. Users want to synchronize fans at specific RPM (e.g., 1200 RPM for all case fans) regardless of individual PWM characteristics.
**Evidence:** FanControl: "all fan curves can be used in RPM mode from their menu; only controls with a valid calibration can use fan curves in RPM mode."

### F6 — Asymmetric Per-Direction Hysteresis and Step Rates
**Gap:** No hysteresis at all in spec, no asymmetric step rates. Well-tuned fan control needs faster ramp-up on temperature spikes than slow ramp-down to avoid hunting/oscillation.
**Evidence:** FanControl: "Up Hysteresis is additional temperature that must be exceeded before fan speed increases. Down Hysteresis is temperature decrease below curve point before fan speed decreases." Issue #1361 discusses making them independently settable.

### F8 — Rule-Based Automatic Profile Switching
**Gap:** No rule-based profile auto-switching. Missing: switch on AC/battery state change, specific process launch, time of day, Windows Power Plan.
**Evidence:** FanControl Issue #97 "App profiles" and Issue #1951 "Fan profiles changing based on time of day" confirm this is unimplemented even in FanControl. iStatMenus and argus-monitor have it.

### F12 — System Diagnostics and Hardware Health Report
**Gap:** Spec tracks AI interventions in SQLite but no diagnostic/health summary feature. TG Pro's diagnostics report is a major selling point.
**Evidence:** TG Pro: "comprehensive diagnostic reports showing Mac's overall health, from power supply issues and battery condition to status of temperature sensors and fans."

### F17 — BIOS Fan Curve Conflict Detection and Setup Wizard
**Gap:** No BIOS interaction guidance. BIOS "Smart Fan" control fighting with software control causes oscillation. No setup wizard or diagnostic for this.
**Evidence:** FanControl troubleshooting: "avoid any 'smart' fan control from BIOS." pwmconfig: "tests PWM outputs, shows correlation between PWM control and fan response."

### F18 — Mix Curve Semantic Specification
**Gap:** `mix` curve type is listed in spec but semantics undefined. FanControl Mix supports N inputs with max/min/average operations. If ThermalIQ's mix is 2-input only, significant downgrade.
**Evidence:** FanControl: "Mix fan curves — combine any amount of fan curves together and apply a function like maximum or average." Argus-monitor: "combination of up to four temperature values — maximum control value of up to 6 independent controllers for every fan channel."

### F22 — System Tray / macOS Menu Bar Live Sensor Display
**Gap:** No persistent system tray icon or menu bar widget. Users expect fan control app to be ambient — always visible without opening main window.
**Evidence:** iStatMenus, Macs Fan Control, TG Pro all use macOS menu bar as primary interface. HWiNFO64 and argus-monitor use Windows system tray with live sensor values.

### F23 — Per-Channel Response Tuning Parameters
**Gap:** No per-channel hysteresis, step-up rate, step-down rate, or response time. Every fan header may need different values — a case fan near GPU needs faster response than a CPU fan with large heatsink thermal mass.
**Evidence:** FanControl: "Fan Control has ALL the parameters: fan calibration, response time, hysteresis, hysteresis direction, step up, step down."

### F24 — DC vs. PWM Mode Selection Per Fan Channel
**Gap:** Spec doesn't address DC vs. PWM mode selection for motherboard fan headers. 3-pin fans use DC voltage control, not PWM. Without DC mode, cannot control older case fans or specific fan types.
**Evidence:** FanControl Issue #855 confirms both modes exist and are selectable. lm-sensors fancontrol.txt documents MINPWM/MAXPWM for DC fans.

---

## NICE Gaps (Would help adoption)

### F9 — Voltage, Frequency, Current, Power Monitoring
**Gap:** No voltage, frequency, current draw, or power consumption monitoring. Users choosing ThermalIQ as their single monitoring app will need a second tool.

### F13 — Native Coolant Temperature / Flow Rate Sensor Types
**Gap:** `file` sensor is a workaround. No native coolant temp, flow rate, liquid level, or pressure sensor types. Custom loop users control fans based on coolant temp, not CPU/GPU temp.

### F14 — In-Game OSD / Overlay for Sensor Data
**Gap:** No in-game overlay / OSD mechanism. NZXT CAM's overlay is a key reason users choose it. RTSS integration is expected in gamer market.

### F16 — Power Limit / Undervolting Recommendations
**Gap:** Fan-control-only scope, but AI agent could recommend power limit changes to complement fan control.

### F19 — Process/App-Aware Automatic Profile Switching
**Gap:** No deterministic process-monitoring trigger for profile switching. AI agent is actually the right tool for this — but not specified as agent capability.
**Note:** ThermalIQ's AI agent makes this a differentiator opportunity over FanControl (which lacks it entirely).

### F21 — Fan Acoustic Performance Modeling
**Gap:** No tool currently has a fan acoustic database or noise-optimized curve recommendations. Genuine differentiation opportunity.

### F25 — HomeAssistant / Network Sensor Integration
**Gap:** No native network sensor source type. FanControl's HomeAssistant plugin enables external Zigbee/network sensors as fan control inputs. `file` sensor is a crude workaround.
**Evidence:** FanControl community plugins: "HomeAssistant temperature sensors plugin enables integration with external smart home sensors."

---

## Prioritized Summary Table

| ID | Gap | Rating |
|----|-----|--------|
| F1 | Temperature alert/alarm system independent of AI agent | CRITICAL |
| F7 | Per-channel fan Start%/Stop% for 0-RPM operation | CRITICAL |
| F10 | USB fan controller hardware support via plugin API | CRITICAL |
| F11 | Laptop embedded controller (EC) support | CRITICAL |
| F15 | Apple Silicon fan write access | CRITICAL |
| F20 | Hardware plugin/extension API for third parties | CRITICAL |
| F2 | Non-temperature sensor inputs (load %, power draw) | IMPORTANT |
| F3 | S.M.A.R.T. drive health monitoring | IMPORTANT |
| F4 | Temperature/fan history logging and CSV export | IMPORTANT |
| F5 | RPM-mode curves (output in RPM units) | IMPORTANT |
| F6 | Asymmetric per-direction hysteresis and step rates | IMPORTANT |
| F8 | Rule-based automatic profile switching | IMPORTANT |
| F12 | System diagnostics and hardware health report | IMPORTANT |
| F17 | BIOS conflict detection and setup wizard | IMPORTANT |
| F18 | Mix curve semantic specification | IMPORTANT |
| F22 | System tray / macOS menu bar live sensor display | IMPORTANT |
| F23 | Per-channel response tuning (hysteresis, step up/down) | IMPORTANT |
| F24 | DC vs. PWM mode selection per fan channel | IMPORTANT |
| F9 | Voltage, frequency, current, power monitoring | NICE |
| F13 | Native coolant temperature / flow rate sensor types | NICE |
| F14 | In-game OSD / overlay for sensor data (RTSS) | NICE |
| F16 | Power limit / undervolting recommendations from AI | NICE |
| F19 | Process/app-aware auto profile switching via AI agent | NICE |
| F21 | Fan acoustic performance modeling | NICE |
| F25 | HomeAssistant / network sensor source integration | NICE |

**Note:** "Fan2" could not be identified as a distinct product — appears to be either a very niche or mislabeled tool; not separately analyzed.
