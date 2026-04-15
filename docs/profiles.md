# ThermalIQ Profile Format

Fan control profiles are TOML files stored at `~/.config/thermaliq/profiles/`.

Each profile defines curves, fan controls, and alerts. ThermalIQ ships with five built-in profiles (balanced, performance, silent, gaming, efficiency).

---

## Full Profile Structure

```toml
name = "my-profile"
description = "Optional human-readable description"

# One or more curve definitions
[[curves]]
# ... (see Curve Types below)

# One or more fan control bindings
[[controls]]
fan = "fan1"       # Fan ID from /fans endpoint
curve = "my-curve" # Must match a curve id above
min_pwm = 15       # Floor — fan never goes below this %

# Zero or more alerts
[[alerts]]
sensor = "cpu_package"
threshold = 90.0
severity = "warn"   # "warn" | "crit"
action = "notify"
```

---

## Curve Types

### `linear`
Straight-line interpolation between two temperature setpoints.

```toml
[[curves]]
type = "linear"
id = "cpu_linear"
source = "cpu_package"   # Sensor ID driving this curve
min_temp = 40.0          # Below this → min_pwm
max_temp = 80.0          # Above this → max_pwm
min_pwm = 20             # Duty % at min_temp (0–100)
max_pwm = 90             # Duty % at max_temp (0–100)
```

### `flat`
Fixed duty cycle regardless of temperature.

```toml
[[curves]]
type = "flat"
id = "chassis_quiet"
pwm = 25      # Fixed duty % (0–100)
```

### `graph`
Multi-point curve — define exact temperature→PWM breakpoints. Values between points are linearly interpolated.

```toml
[[curves]]
type = "graph"
id = "cpu_graph"
source = "cpu_package"

[[curves.points]]
temp = 35.0
pwm = 15

[[curves.points]]
temp = 55.0
pwm = 30

[[curves.points]]
temp = 70.0
pwm = 60

[[curves.points]]
temp = 85.0
pwm = 100
```

### `mix`
Combine multiple curve outputs using a mixing function.

```toml
[[curves]]
type = "mix"
id = "combined"
curves = ["cpu_linear", "gpu_linear"]  # Curve IDs to combine
function = "max"   # "max" | "min" | "average" | "sum"
```

`sum` is clamped to 100%.

### `trigger`
Two-state step function — idle below threshold, load above.

```toml
[[curves]]
type = "trigger"
id = "chassis_trigger"
source = "cpu_package"
idle_temp = 45.0
idle_pwm = 20
load_temp = 70.0
load_pwm = 75
```

### `sync`
Mirror another curve with a fixed offset.

```toml
[[curves]]
type = "sync"
id = "chassis_sync"
target = "cpu_linear"  # Curve ID to follow
offset = -10           # PWM offset (positive or negative)
```

The result is clamped to 0–100%.

### `auto`
Adaptive linear with a configurable deadband. Reduces fan changes during minor temperature fluctuations.

```toml
[[curves]]
type = "auto"
id = "cpu_auto"
source = "cpu_package"
idle_temp = 40.0
load_temp = 85.0
min_pwm = 15
max_pwm = 100
deadband = 5.0       # Temperature swing to ignore (°C)
step_up_pct = 10     # Max PWM increase per tick (prevents spin-up noise)
step_down_pct = 3    # Max PWM decrease per tick (smoother spin-down)
```

### `ai`
Fully agent-controlled. The AI agent sets the duty directly; this curve provides the routing.

```toml
[[curves]]
type = "ai"
id = "ai_curve"
source = "cpu_package"
goal = "efficiency"    # Hint to agent: "efficiency" | "performance" | "quiet"
```

Requires agent to be in `approval` or `autonomous` mode. Falls back to 60% duty if agent is passive.

### `predictive`
Ramps up cooling ahead of expected load based on temperature trend.

```toml
[[curves]]
type = "predictive"
id = "cpu_predictive"
source = "cpu_package"
min_pwm = 20
max_pwm = 100
lookahead_secs = 300   # How far ahead to look (default: 300s)
```

### `learning`
Adapts its response curve over time based on observed thermal patterns.

```toml
[[curves]]
type = "learning"
id = "cpu_learning"
source = "cpu_package"
base_min_pwm = 20
base_max_pwm = 90
learning_rate = 0.1    # How quickly to adapt (0.0–1.0, default: 0.1)
```

---

## Fan Controls

Each `[[controls]]` entry binds a curve to a physical fan.

```toml
[[controls]]
fan = "fan1"       # Fan ID — use `tq fans` to list available IDs
curve = "my_curve" # Must match a curve's `id` field in this profile
min_pwm = 15       # Absolute floor — fan never spins below this %
```

A fan can only be controlled by one curve per profile.

---

## Alerts

Alerts fire when a sensor exceeds a threshold. They are evaluated on every poll tick.

```toml
[[alerts]]
sensor = "cpu_package"   # Sensor ID — use `tq sensors` to list
threshold = 85.0         # Trigger temperature (°C)
severity = "warn"        # "warn" | "crit"
action = "notify"        # Action (see below)
```

### Alert Actions

| Action | Description |
|--------|-------------|
| `"notify"` | Log a warning via tracing (visible in daemon output) |
| `"log"` | Silently log at debug level |
| `{ apply_profile = "performance" }` | Switch to a named profile |

### Alert Example with Profile Switch
```toml
[[alerts]]
sensor = "cpu_package"
threshold = 92.0
severity = "crit"
action = { apply_profile = "performance" }
```

---

## Built-In Profiles

| Profile | Strategy |
|---------|----------|
| `balanced` | Linear curve, quiet at idle, responsive under load |
| `performance` | Aggressive curves, maximises thermal headroom |
| `silent` | Flat low PWM, accepts higher temperatures for silence |
| `gaming` | High-performance cooling with GPU awareness |
| `efficiency` | Auto curve with 0–70% ceiling, auto-escalates to balanced at 88°C |

---

## Finding Sensor and Fan IDs

```bash
tq sensors   # Lists sensor IDs and current temperatures
tq fans      # Lists fan IDs, RPM, and current duty %
```

Match the `id` field in the output to the `source` and `fan` fields in your profile.
