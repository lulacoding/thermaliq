# Stage 2: Agent Architecture Gaps & Edge Cases
**Status:** Complete | **Tier:** HIGH | **Model:** Sonnet

## Critical Gaps Found

### 1. STATE MACHINE
- No ERROR/DEGRADED/SUSPENDED state — fault handling is implicit and untraceable
- SAMPLING interruption: no minimum sample threshold before proceeding to ANALYZING
- AI provider failure during ANALYZING: no fallback provider sequencing defined
- Hardware offline during ACTING: double-fault scenario (can't read state to create rollback snapshot)
- Conflicting concurrent interventions: no priority hierarchy (emergency > planned > fine-tune)
- VERIFYING: no timeout, no success criteria (what metric? what threshold?)
- ROLLING_BACK failure: no defined next state — terminal state gap

### 2. SAFETY SYSTEM
- No self-validation: incorrect safety rules have no detection path
- No dual-control: safety rule changes not audited/reviewed separately
- Race condition between enforcer and agent unaddressed (Tokio async: stale read risk)
- Min PWM + emergency response: ramp-down path safety not defined
- Cooldown (5min) could BLOCK emergency response — explicit bypass not stated
- No dT/dt (rate-of-change) thermal runaway guard — only point-in-time thresholds

### 3. WORKLOAD DETECTION
- Classification mechanism entirely unspecified (signals? feature vector? model type?)
- VM/container workloads misclassified (qemu → "stress", containerd → generic)
- Rapid transitions (<30s): no hysteresis/debounce → wrong profile, then blocked by cooldown
- "Mixed" class has no sub-classification or weighted profile blending
- Novel workload types (AI training, etc.) have no "unknown" class or learning path

### 4. MEMORY/LEARNING
- Learning curve algorithm entirely unspecified (PID? Bayesian? bandit? gradient descent?)
- No oscillation prevention / damping — fan bearing wear + acoustic annoyance risk
- Concept drift from hardware degradation (paste, dust, bearing wear) not addressed
- 7-day SQLite rolling window destroys long-term baseline needed for degradation detection
- No feedback loop: intervention outcomes don't improve workload classifier

### 5. AI CONTEXT (AiContext) GAPS
- Process context missing (which processes + their resource share)
- Fan age/bearing wear proxy missing
- Ambient temperature absent
- User preference history absent (user keeps rejecting proposals → AI keeps making them)
- Hardware capability limits absent (actual min/max RPM, PWM frequency, DC vs PWM)
- Thermal history granularity undefined (raw 7-day = 600k rows; hourly = loses spikes)
- Hardware identity missing (motherboard model, fan model, CPU TDP, cooler model)

### 6. ROLLBACK
- Snapshot definition missing (full profile? profile ID only? all register values? diff?)
- No snapshot validity check (driver update between snapshot and rollback)
- Partial rollback unaddressed (3/4 fans restored, 4th driver call fails)
- Circular buffer eviction policy undefined; no manual checkpoint feature

### 7. CONCURRENCY
- No explicit synchronization (Arc<Mutex<AgentState>> or actor model) — state corruption risk
- Double-approval bug: GUI + CLI can both approve same intervention_id
- WebSocket event ordering not guaranteed — GUI may render approve button in wrong state
- 6 interventions/hr rate limit: TOCTOU race on counter increment

### 8. MISSING AGENT CAPABILITIES
- No predictive maintenance (bearing failure, paste degradation — precursor signals exist)
- No noise/acoustic optimization — objective is thermal only, not noise-temp tradeoff
- No quiet hours / time-of-day scheduling
- No per-application fan curves (only workload-class-level)
- No energy efficiency mode (power consumption not an optimization target)
- No remote monitoring/alerting beyond local WebSocket
- No multi-GPU / asymmetric topology support (single temp constant per device type)
- No OS power plan / CPU governor integration
- OpenClaw webhook semantics undefined (what events? auth? safety implications?)
