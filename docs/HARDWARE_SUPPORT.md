# ThermalIQ Hardware Support

---

## Sensor support

### CPU

| Vendor | Models | Backend | Status |
|--------|--------|---------|--------|
| Intel | Core (all generations), Xeon | HWiNFO64 (Win) / coretemp (Linux) / IOKit SMC (macOS) | Supported |
| AMD | Ryzen, Threadripper, EPYC | HWiNFO64 (Win) / k10temp (Linux) | Supported |
| Apple | M1, M2, M3, M4 | IOKit SMC | Sensor reads work; fan writes advisory-only (see below) |

### GPU

| Vendor | Models | Backend | Sensor reads | Fan writes |
|--------|--------|---------|--------------|------------|
| NVIDIA | GTX 10xx–RTX 50xx | NVML (driver 520+) | Supported | Supported (NVML) |
| AMD | RDNA1 / RDNA2 | HWiNFO64 (Win) / hwmon (Linux) | Supported | Linux only (`pwm*_enable`) |
| AMD | RDNA3 (RX 7000 series) | hwmon `gpu_od/fan_ctrl/` (kernel 6.7+) | Supported | Linux kernel 6.7+ |
| AMD | RDNA3 (Windows) | HWiNFO64 | Supported | Coming in v1.1 |
| Intel | Arc A-series | HWiNFO64 (Win) | Supported | Not supported |
| Apple | M-series integrated | IOKit | Supported | Advisory-only |

### Motherboard / chassis fans

| Platform | Coverage | Notes |
|----------|----------|-------|
| Windows | Read via HWiNFO64; writes not supported | Kernel driver required for writes — WinRing0/PawnIO avoided (Defender / anti-cheat risk) |
| Linux | Read + write via hwmon `pwm*` | Most boards supported; requires `video` group or root |
| macOS | Read via IOKit SMC | Write via SMJobBless helper (requires Developer ID build) |

### Laptop embedded controllers

| Brand | Mechanism | Status |
|-------|-----------|--------|
| ThinkPad | `thinkfan` | Supported (Linux) |
| ASUS | `asus-wmi` kernel module | Supported (Linux) |
| Dell | `dell-smm-hwmon` | Supported (Linux) |
| Others | NBFC config database (300+ models) | Supported (Linux) |
| Windows laptops | HWiNFO64 where available | Partial — varies by OEM |

---

## Platform-specific notes

### Apple M1–M4

- Temperature sensor reads work via IOKit SMC.
- Fan writes are advisory only in unsigned builds. A signed Developer ID build with the SMJobBless privileged helper is required for actual fan control.
- M3 and M4 chips enforce a firmware floor: fans remain off below approximately 77°C. This is a hardware limit, not a ThermalIQ bug.
- Full macOS fan write support is targeted for v1.2.

### Windows — HWiNFO64 dependency

ThermalIQ uses HWiNFO64's shared memory interface (`Global\HWiNFO_SENS_SM2`) as its primary sensor backend on Windows. This avoids kernel drivers that trigger Windows Defender or anti-cheat software.

- HWiNFO64 must be running for full sensor coverage.
- If HWiNFO64 is absent, ThermalIQ falls back to WMI (`Win32_TemperatureProbe`, `MSAcpi_ThermalZoneTemperature`) with limited sensor coverage.
- The UI displays a status banner indicating whether HWiNFO64 shared memory is active.

### Linux — RDNA3 fan curves

RDNA3 cards (RX 7000 series) expose a richer fan control interface via:

```
/sys/class/drm/cardX/device/gpu_od/fan_ctrl/
```

This requires:

1. Kernel 6.7 or later
2. `amdgpu.ppfeaturemask=0xffffffff` kernel parameter

Add to `/etc/default/grub`:

```
GRUB_CMDLINE_LINUX_DEFAULT="... amdgpu.ppfeaturemask=0xffffffff"
```

Then `sudo update-grub` and reboot. ThermalIQ detects this parameter and warns if it is absent.

Zero-RPM stop temperature (`fan_zero_rpm_stop_temperature`) requires kernel 6.13+. ThermalIQ falls back gracefully on older kernels.

---

## Known-good hardware (community tested)

<!-- Screenshot stub: community-tested hardware table — add after beta testers submit reports -->

This table will be populated from Hardware Compatibility reports filed by beta testers. If your hardware works, please [submit a report](https://github.com/lulacoding/thermaliq/issues/new?template=hardware_compatibility.yml).

---

## Known limitations

| Hardware | Limitation | Target |
|----------|-----------|--------|
| AMD GPU fan writes (Windows) | ADLX integration not yet implemented | v1.1 |
| Windows motherboard headers | Not writable without kernel driver | Investigating |
| macOS fan writes (unsigned build) | Advisory-only without Developer ID cert | v1.2 |
| Intel Arc fan writes | IGCL SDK integration not implemented | v1.2 |
| AIO pump speed | Requires vendor SDK (Corsair iCUE, NZXT) | v1.2 |

---

## Contribute a hardware report

File a [Hardware Compatibility Report](https://github.com/lulacoding/thermaliq/issues/new?template=hardware_compatibility.yml) with your CPU, GPU, motherboard, OS version, and whether sensors and fan writes work correctly.

This directly improves the known-good hardware table above.
