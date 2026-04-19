# ThermalIQ Quickstart

Get from install to a working fan profile in under five minutes.

---

## Prerequisites

### Windows

- Windows 10 or 11 (x86-64)
- [HWiNFO64](https://www.hwinfo.com/download/) installed and running — required for full sensor and fan coverage
- NVIDIA GPU owners: driver version 520+ for NVML fan writes
- AMD GPU fan writes: Coming in v1.1

### macOS

- macOS 12 (Monterey) or later
- Xcode Command Line Tools: `xcode-select --install`
- Rust toolchain: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Fan writes require the SMJobBless privileged helper.** Without a signed Developer ID build, the daemon reads temperatures and logs intended fan changes — control is advisory only. See [docs/macos-helper.md](macos-helper.md).

### Linux

- Kernel 4.14+ with hwmon support (`/sys/class/hwmon/` present)
- `lm-sensors` for sensor enumeration: `sudo apt install lm-sensors && sudo sensors-detect`
- AMD RDNA3 GPU fan curves: kernel 6.7+ and `amdgpu.ppfeaturemask=0xffffffff` in kernel parameters
- Rust toolchain: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

---

## Step 1: Install

### Windows (recommended)

```powershell
winget install thermaliq.ThermalIQ
```

Or download the NSIS installer from [GitHub Releases](https://github.com/lulacoding/thermaliq/releases). The installer registers the daemon as a Windows service that starts automatically.

### macOS / Linux (build from source)

```bash
git clone https://github.com/lulacoding/thermaliq
cd thermaliq
cargo build --release
sudo cp target/release/thermaliq-daemon /usr/local/bin/
sudo cp target/release/tq /usr/local/bin/
```

Linux: allow fan writes without running as root (optional):

```bash
sudo usermod -aG video $USER
# log out and back in for the group change to take effect
```

---

## Step 2: Install bundled profiles

```bash
mkdir -p ~/.config/thermaliq/profiles
cp profiles/*.toml ~/.config/thermaliq/profiles/
```

On Windows, profiles go to `%APPDATA%\thermaliq\profiles\`. The installer copies them automatically.

---

## Step 3: Start the daemon

**Windows:** The daemon starts automatically as a service after installation. Check its status:

```powershell
sc query ThermalIQ
```

**macOS / Linux:**

```bash
thermaliq-daemon
# Logs to stderr. Set RUST_LOG=debug for verbose output.
```

The daemon creates the config file and `api.token` on first run:
- **macOS/Linux**: `~/.config/thermaliq/api.token`
- **Windows**: `%APPDATA%\thermaliq\api.token`

---

## Step 4: Read your first sensor data

```bash
tq sensors
```

Expected output:

```
cpu_package    58.5 C   CPU Package
gpu_edge       42.0 C   GPU Edge
```

```bash
tq fans
```

Expected output:

```
fan1   1800 RPM   45%   CPU Fan
fan2   1200 RPM   35%   Chassis
```

If no sensors appear, see [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md).

---

## Step 5: Apply your first profile

```bash
tq profile list
# balanced
# performance
# silent
# gaming
# efficiency

tq profile set balanced
# active profile: balanced
```

Profile changes take effect on the next poll tick (default: 1 second).

---

## Step 6: Verify the daemon status

```bash
tq status
# health:  ok
# version: 1.0.0
# agent:   passive (pending: 0)
```

---

## Step 7: (Optional) Enable the AI agent

```bash
# Approval mode — agent proposes changes, you approve each one
tq agent mode approval

# Check for pending interventions
tq agent interventions

# Approve a specific intervention
tq agent approve <id>

# Or hand full control to the agent
tq agent mode autonomous
```

See [docs/AI_AGENT.md](AI_AGENT.md) for a full explanation of modes, guardrails, and AI providers.

---

## Step 8: (Optional) Create a profile via natural language

Requires an AI provider configured in `~/.config/thermaliq/thermaliq.toml`:

```toml
[ai]
provider = "anthropic"
api_key  = "sk-ant-..."
```

Then:

```bash
tq profile create "quiet for video calls, keep CPU below 70 degrees"
# Generating profile...
# Saved: quiet-video-calls.toml
tq profile set quiet-video-calls
```

---

## Next steps

- [docs/AI_AGENT.md](AI_AGENT.md) — AI modes, providers, guardrails
- [docs/profiles.md](profiles.md) — full profile and curve syntax
- [docs/HARDWARE_SUPPORT.md](HARDWARE_SUPPORT.md) — supported hardware matrix
- [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) — fixes for common problems
- [docs/api.md](api.md) — REST and WebSocket API reference
