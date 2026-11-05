# M12 spike: prove one case fan moves

Throwaway console app, not product code. It answers three questions on Sam's PC before any bridge code is written. Results are at the end of this file.

The fan step writes to a real fan header. Pick a case fan, or use a CPU header only at `-Duty 100`, never lower. The spike refuses headers named CPU or pump below 100%, and any duty under 20%, but many boards only say `Fan #2`, so check the header first. If the process is hard-killed or the power goes out during the 10 s, that fan keeps 60% until the next reboot. Ctrl-C and closing the window run the hand-back, and on an NCT6687D the check that it worked is `0A00` in `out\fan-after.txt` (see Results).

## 1. Install

**.NET 10 SDK** (this PC only has 8.0):

```powershell
winget install Microsoft.DotNet.SDK.10
```

Open a new terminal afterwards, then run `dotnet --list-sdks` and check that a `10.0` line appears.

**PawnIO:** already on this PC (2.2.0, service running), so skip this unless you uninstalled it.

- Official site: <https://pawnio.eu>. Official releases: <https://github.com/namazso/PawnIO.Setup/releases>
- Download `PawnIO_setup.exe` from release 2.2.0, then check it before running it:

```powershell
Get-FileHash .\PawnIO_setup.exe -Algorithm SHA256
Get-AuthenticodeSignature .\PawnIO_setup.exe | Format-List Status, SignerCertificate
```

- Expected hash: `1F519A22E47187F70A1379A48CA604981C4FCF694F4E65B734AAA74A9FBA3032`. Expected: `Status` is `Valid`, signer `CN=namazso.eu`. Both checked on 2026-11-05.
- Run it, accept the prompt, and finish the setup.

## 2. Build

From the repo root, in an **elevated** PowerShell (Run as administrator):

```powershell
cd docs\dev\research\m12-spike
dotnet build -c Release
.\bin\Release\net10.0-windows\m12-spike.exe selftest
```

Every line should start with `ok`.

## 3. Run

Close FanControl, LibreHardwareMonitor and OpenRGB, or the spike refuses to start. HWiNFO can stay open for (a) and (b).

**(a) List the chip, fans and controls:**

```powershell
.\bin\Release\net10.0-windows\m12-spike.exe list
```

Copy the id of the control you want, for example `/lpc/nct6687d/0/control/5`. The `mode` column is LHM's own view and says `Undefined` until the spike touches a control. The chip's real mode is in the register dump at `out\list.txt`.

**(b) Set that fan to 60% (or `-Duty N`, 20-100) for 10 s, read RPM, hand it back:**

```powershell
.\bin\Release\net10.0-windows\m12-spike.exe fan /lpc/nct6687d/0/control/5
.\bin\Release\net10.0-windows\m12-spike.exe fan /lpc/nct6687d/0/control/0 -Duty 100
```

It prints every fan, so you can see which one really moved, then the same again after the hand-back. Listen to the fan as well. Register dumps go in `out\fan-before.txt`, `fan-held.txt` and `fan-after.txt`.

**(c) Unload and restart PawnIO.** Close HWiNFO and anything else that uses PawnIO first:

```powershell
.\bin\Release\net10.0-windows\m12-spike.exe driver
```

Look for the two `RESULT` lines. Restart counts as clean only if the chip and a spinning fan read correctly afterwards.

PawnIO is installed as a root PnP device (`ROOT\PAWNIO\0000`), so `sc stop` may be refused. If `RESULT unload: NO` shows error 1052, try this and report whether each step works without a reboot:

```powershell
pnputil /disable-device "ROOT\PAWNIO\0000"
sc.exe query PawnIO
pnputil /enable-device "ROOT\PAWNIO\0000"
.\bin\Release\net10.0-windows\m12-spike.exe list
```

If it stays in `STOP_PENDING`, something still holds it open. A reboot clears that.

## 4. Report back

Paste this into chat. Don't commit `out\`, because the reports can include board identifiers.

- The `list` output
- The `fan` output, and which fan you heard change
- From the `fan-before` and `fan-after` dumps: do the fan control registers match?
- The `driver` output, plus the `pnputil` results if you needed them
- With PawnIO 2.2.0 loaded, does FACEIT still refuse to start? (see SPEC §5.1)

## Why these checks

From the LibreHardwareMonitorLib source:

- `SetDefault()` writes back the register values saved at the **first** `SetSoftware` of that process, not a fixed "BIOS" value. So `fan-before` against `fan-after` is the real test of the hand-back.
- Those saved values live only in memory. A bridge that dies loses them (PORTING M12).
- `Computer.Close()` restores all controls. `driver` opens the library only after the unload, so nothing in this process can hold PawnIO open. The same holds for the bridge once it has exited, since Windows closes a dead process's handles.

## Results

Sam's PC, 2026-11-05: MSI B550M PRO-VDH WIFI, NCT6687D, PawnIO 2.2.0. On this chip `0xA00` holds one manual flag per header (bit 5 is control 5) and `0x160`–`0x167` hold the actual duty.

**LHM 0.9.6, control 5, starting from the BIOS (bug confirmed):**

| Run | Before | After | Meaning |
|---|---|---|---|
| 1 | `0A00 = 00`, duty 100% | `0A00 = 20`, duty 40% (`66`) | Stayed manual at a stale duty, BIOS curve off for that header |
| 2 | `0A00 = 20`, duty 40% | `0A00 = 00`, duty 100% (`FF`) | Back on the BIOS |

Cause: 0.9.6 restores with `mode & ~initial`, so it clears the flag only if it was already set when saved. Fixed in LHM `dc51e75` (#2527). `0.9.7-pre742` was built from exactly that commit (its nuspec records it), and pre741 is behind it.

**LHM `0.9.7-pre742`, control 5, starting from the BIOS (Sam's report):** both runs ended with `0A00 = 00` and duty `FF`, so the hand-back passes on this header. The same PawnIO disable and enable test also passed on this build (see below).

**Fan movement, LHM `0.9.7-pre742` (from Sam's review on PR #56):** CPU fan (control 0) went from 62% to 100% for 10 s, **1675 → 2010 RPM**, and hand-back ended `0A00 = 00`. After the run the duty read 83% (`D3`), not 62%: with the flag clear, that is the BIOS's own ramp-down. Sam counts this as the spike gate. G3 still needs a case fan with a tach signal: only the CPU fan reports RPM on this board, HWiNFO agrees, and no USB fan controller was found. The `moved` line in 0.9.6 run 2 was the CPU fan following its BIOS curve, not the spike.

**PawnIO unload (HWiNFO closed). `sc stop` tested on 0.9.6, `pnputil` on both 0.9.6 and pre742:**

- `sc stop PawnIO` is refused with error 1052.
- `pnputil /disable-device "ROOT\PAWNIO\0000"` stops the service with no reboot (`STATE 1 STOPPED`, exit code 31).
- `pnputil /enable-device "ROOT\PAWNIO\0000"` brings it back with no reboot (`STATE 4 RUNNING`), and `list` then reads the chip and the CPU fan.
- Not tested: disabling while another program holds PawnIO open, or whether FACEIT then starts.
