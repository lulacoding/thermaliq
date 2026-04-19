# ThermalIQ Troubleshooting

---

## No sensors detected

### Windows

ThermalIQ on Windows reads from HWiNFO64 shared memory. If sensors are missing:

1. Confirm HWiNFO64 is installed and **currently running** (it must be open in the background).
2. In HWiNFO64: Settings > SMBus/I2C Sensors — ensure shared memory support is enabled.
3. Run `tq sensors`. If the output is empty, check the HWiNFO64 status banner in the ThermalIQ UI.
4. WMI fallback activates automatically when HWiNFO64 is absent but covers fewer sensors.

Verify HWiNFO64 shared memory is active:

```powershell
# Check that the shared memory segment exists
[System.IO.MemoryMappedFiles.MemoryMappedFile]::OpenExisting("Global\HWiNFO_SENS_SM2")
```

### macOS

```bash
# Check IOKit is accessible
system_profiler SPHardwareDataType | grep -i "model identifier"

# Verify the daemon can read SMC keys
RUST_LOG=debug thermaliq-daemon 2>&1 | grep -i smc
```

### Linux

```bash
# Check hwmon devices are present
ls /sys/class/hwmon/

# Run sensors-detect if nothing appears
sudo sensors-detect --auto

# Check kernel modules for your CPU
lsmod | grep -E "coretemp|k10temp|nct6775"

# Load missing module (example for Intel)
sudo modprobe coretemp
```

---

## Fan won't change speed

### Windows

- Motherboard fan headers cannot be controlled directly without a kernel driver — this is a documented limitation. NVIDIA GPU fans (NVML) and some ACPI-accessible fans work.
- NVIDIA fans require driver version 520+. Check: `nvidia-smi --query-gpu=driver_version --format=csv,noheader`
- AMD GPU fan writes are Coming in v1.1.

### macOS

Fan writes require the SMJobBless privileged helper installed from a Developer ID-signed build. Without it, ThermalIQ logs intended changes but does not write to hardware.

Check helper status:

```bash
# Check if the helper process is running
launchctl list | grep com.thermaliq.helper

# Check daemon logs for helper errors
RUST_LOG=debug thermaliq-daemon 2>&1 | grep -i helper
```

If the helper is not running, see [docs/macos-helper.md](macos-helper.md).

### Linux

```bash
# Check pwm files exist
ls /sys/class/hwmon/hwmon*/pwm*

# Check pwm is in manual mode (value should be 1)
cat /sys/class/hwmon/hwmon0/pwm1_enable

# Enable manual mode manually to test
echo 1 | sudo tee /sys/class/hwmon/hwmon0/pwm1_enable

# Test writing a value
echo 128 | sudo tee /sys/class/hwmon/hwmon0/pwm1
```

If writes fail with permission errors, check the daemon is running as a user in the `video` group or as root:

```bash
groups $USER   # should include "video"
```

---

## AI agent not responding

1. Check the daemon is running: `tq status`
2. Check agent mode: `tq agent status` — must be `approval` or `autonomous` (not `passive`) for the agent to act.
3. Check AI provider configuration in `~/.config/thermaliq/thermaliq.toml`:

```toml
[ai]
enabled  = true
provider = "anthropic"
api_key  = "sk-ant-..."
```

4. If no API key is configured, AI features are disabled and the agent falls back to passive monitoring. Manual fan control still works.
5. Check daemon logs for provider errors:

```bash
RUST_LOG=debug thermaliq-daemon 2>&1 | grep -i "ai\|provider\|anthropic\|openai"
```

---

## HWiNFO64 not detected (Windows)

The UI shows a yellow banner "HWiNFO64 not detected" when the shared memory segment `Global\HWiNFO_SENS_SM2` is absent.

1. Download and launch [HWiNFO64](https://www.hwinfo.com/download/).
2. Start it in "Sensors-only" mode.
3. Keep it running in the background — ThermalIQ reads from it every 2 seconds.
4. Restart the ThermalIQ daemon after HWiNFO64 is running:

```powershell
sc stop ThermalIQ
sc start ThermalIQ
```

---

## tq diagnose

```bash
tq status           # daemon health and version
tq sensors          # confirm sensor readings
tq fans             # confirm fan detection
tq agent status     # agent mode and pending count
```

For verbose logs:

```bash
# Linux / macOS
RUST_LOG=debug thermaliq-daemon 2>&1 | tee /tmp/thermaliq-debug.log

# Windows (PowerShell)
$env:RUST_LOG="debug"
thermaliq-daemon 2>&1 | Tee-Object -FilePath "$env:TEMP\thermaliq-debug.log"
```

---

## Log file locations

| Platform | Log path |
|----------|----------|
| Windows | `%APPDATA%\thermaliq\logs\` |
| macOS | `~/Library/Logs/ThermalIQ/` |
| Linux | stderr / `journalctl -u thermaliq` (if running as a service) |

---

## Filing a bug report

Before opening an issue:

1. Run `tq status` and note the version.
2. Collect logs with `RUST_LOG=debug` (see above).
3. On Windows: include a HWiNFO64 sensor screenshot.
4. Open a [Bug Report](https://github.com/lulacoding/thermaliq/issues/new?template=bug_report.yml) and attach the log file.

For hardware-specific problems (sensors missing, wrong RPM values), use the [Hardware Compatibility Report](https://github.com/lulacoding/thermaliq/issues/new?template=hardware_compatibility.yml) template instead.
