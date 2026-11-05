# M12 checks on the Windows PC

**None has been run yet.** Do these in order, on branch `m12-daemon`. M12 is ticked only after they pass and both PRs are approved. Paste each step's output into chat, and stop at the first failure.

These steps write to one case-fan header on the real board and hard-kill the service on purpose. They leave that header on the BIOS at the end. They never write to the CPU fan or a pump, as long as you pick a case header in step 1. Don't commit `out\` dumps.

Use an **admin** PowerShell, and close Fan Control, MSI Center and games first.

| Step | What it proves | README check |
|---|---|---|
| 1 | The service starts the bridge, and the pipe check passes | 7 |
| 2 | Takeover, no low first write, read-back and `GetReport` timing | 1, 2, 3, 6 |
| 3 | Hand-back on service stop | 1 |
| 4 | Hard kill, then a restart hands back from the saved originals | 4 |
| 5 | A hung bridge is killed and restarted (the #58 review fix) | new |
| 6 | The bridge's watchdog, when the daemon goes silent | 5 |
| 7 | Anti-cheat mode, with a fake client | 8 |
| 8 | Real FACEIT | 9 |
| 9 | Clean up | |

Hardware can't provoke two things, so only tests cover them: a header that stays manual and goes to 100%, and the Fan Control / MSI Center message.

## 1. Install, and start with no fan under control

```powershell
cd C:\Users\samjo\dev\thermaliq
git switch m12-daemon; git pull
cargo build -p thermaliq-daemon
$pf = "$env:ProgramFiles\ThermalIQ"
$dir = "$env:ProgramData\ThermalIQ"
if (Get-Service ThermalIQ -ErrorAction SilentlyContinue) { Stop-Service ThermalIQ; & "$PWD\target\debug\thermaliq-daemon.exe" uninstall-service }
Get-Process lhm-bridge -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item -Recurse -Force "$pf\lhm-bridge" -ErrorAction SilentlyContinue
Remove-Item "$dir\lhm-bridge.log", "$dir\service.log" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $pf | Out-Null
Copy-Item target\debug\thermaliq-daemon.exe $pf -Force
dotnet publish bridges\lhm-bridge\LhmBridge\LhmBridge.csproj -c Release -r win-x64 --self-contained -o "$pf\lhm-bridge"
if (Test-Path "$dir\config.toml") { Copy-Item "$dir\config.toml" "$dir\config.toml.m12bak" -Force }
& "$pf\thermaliq-daemon.exe" install-service
Get-Process lhm-bridge -ErrorAction SilentlyContinue   # must print nothing: install-service starts no bridge
Start-Service ThermalIQ
Start-Sleep 15
$api = 'http://127.0.0.1:7272'
function Show-State {
    Invoke-RestMethod "$api/status" | Select-Object degraded, degraded_reasons, unavailable_sources | Format-List
    Invoke-RestMethod "$api/fans" | Select-Object id, label, kind, rpm, duty_percent, curve_duty | Format-Table
    Get-Process lhm-bridge -ErrorAction SilentlyContinue | Select-Object Id, StartTime | Format-Table
}
Show-State
Get-Content "$dir\service.log", "$dir\lhm-bridge.log" -ErrorAction SilentlyContinue
```

**Pass:** `/fans` lists `lhm_nct6687d_0_fan_*` headers beside the GPU fan, `degraded` is false, `unavailable_sources` holds only `hwinfo` (if HWiNFO isn't running), and `lhm-bridge` is running. Listing the headers proves the pipe's server check passed, because no client gets a fan list without it.

**If it fails,** it now says why in three places, so paste all three: `degraded_reasons` (a bridge that should have started and didn't shows as `lhm-bridge: <reason>`), `unavailable_sources`, and `service.log` (everything the service prints, bridge start-up included; before this fix it was thrown away). An engine with no fans at all reports `degraded` too.

Then pick a **case-fan** header you can hear, not `CPU` or `Pump`. Last time that was control 5:

```powershell
$fan = 'lhm_nct6687d_0_fan_5'
```

## 2. Takeover

Add a flat 60% curve on that one header. Listen as you run the restart: the fan may rise briefly, because the first write is 100%, but it must never dip.

```powershell
New-Item -ItemType Directory -Force "$dir\profiles" | Out-Null
Set-Content -Encoding ascii "$dir\profiles\m12.toml" "name = `"m12`"`n[[curves]]`ntype = `"linear`"`nid = `"c`"`nsource = `"nvml_gpu0_temp`"`nmin_temp = 30.0`nmax_temp = 90.0`nmin_pwm = 60`nmax_pwm = 60`n[[controls]]`nfan = `"$fan`"`ncurve = `"c`"`nstep_up_pct = 100`nstep_down_pct = 100`n"
Set-Content -Encoding ascii "$dir\config.toml" 'default_profile = "m12"'
Restart-Service ThermalIQ
Start-Sleep 15
Show-State
Get-Content "$dir\bridge-originals.json"
Get-Content "$dir\lhm-bridge.log"
```

**Pass:**
- The fan's `duty_percent` is about 60, and you heard no dip.
- `bridge-originals.json` holds that header, with `Mode` 0.
- `degraded` is false.
- Every `GetReport took` line in the log is well under 2000 ms. Under 500 ms is comfortable, and over 1000 ms needs a look even if it passes.
- Any `read-back polls` line says 4 or less. 5 polls is the limit (500 ms), so a 4 means the margin is thin.

## 3. Hand-back on stop

```powershell
Measure-Command { Stop-Service ThermalIQ } | Select-Object TotalSeconds
Get-Content "$dir\bridge-originals.json"
Get-Process lhm-bridge -ErrorAction SilentlyContinue
cd docs\dev\research\m12-spike; dotnet run -c Release -- list | Out-Null; Select-String '0A00' out\list.txt; cd ..\..\..\..
```

**Pass:** the stop takes under 8 s, the file no longer holds that header, no `lhm-bridge` is left, and the `0A00` row's first byte has that header's bit clear (bit 5 = `20` for control 5). You should hear the fan return to its BIOS speed.

## 4. Hard kill, then recover

The bridge goes first, so nothing gets to hand back. Setting the service to Disabled stops the SCM restarting it, so the chip can be read in between.

```powershell
Start-Service ThermalIQ; Start-Sleep 15
Set-Service ThermalIQ -StartupType Disabled
Stop-Process -Name lhm-bridge -Force; Stop-Process -Name thermaliq-daemon -Force
Start-Sleep 2
Get-Content "$dir\bridge-originals.json"
cd docs\dev\research\m12-spike; dotnet run -c Release -- list | Out-Null; Select-String '0A00' out\list.txt; cd ..\..\..\..
```

**Expected so far:** the header is still in the file, and its bit in `0A00` is **set** (`20`). That is the state to recover from.

Now start without the header in the profile, so the engine doesn't take it back:

```powershell
Set-Content -Encoding ascii "$dir\config.toml" 'default_profile = "balanced"'
Set-Service ThermalIQ -StartupType Automatic
Start-Service ThermalIQ; Start-Sleep 15
Get-Content "$dir\bridge-originals.json"
Show-State
Stop-Service ThermalIQ
cd docs\dev\research\m12-spike; dotnet run -c Release -- list | Out-Null; Select-String '0A00' out\list.txt; cd ..\..\..\..
Set-Content -Encoding ascii "$dir\config.toml" 'default_profile = "m12"'
Start-Service ThermalIQ; Start-Sleep 15
```

**Pass:** the file no longer holds the header, its `0A00` bit is clear, and `degraded` is false. If `balanced` is missing, the service reports it: tell me rather than working around it.

## 5. A hung bridge is killed and restarted

This freezes the bridge. It can't answer, and its watchdog can't run, which is what a call stuck in PawnIO looks like from outside. The helper was tried on a harmless process, not on the bridge.

```powershell
[Diagnostics.Process]::EnterDebugMode()
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class Pause{[DllImport("ntdll.dll")]public static extern int NtSuspendProcess(IntPtr h);[DllImport("ntdll.dll")]public static extern int NtResumeProcess(IntPtr h);}'
$b = Get-Process lhm-bridge; $old = $b.Id
[Pause]::NtSuspendProcess($b.Handle)
$t = [Diagnostics.Stopwatch]::StartNew()
while ((Get-Process -Id $old -ErrorAction SilentlyContinue) -and $t.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 100 }
"gone after $($t.Elapsed.TotalSeconds) s"
Invoke-RestMethod "$api/status" | Select-Object degraded, degraded_reasons
Start-Sleep 20
Show-State
```

**Pass:** the old bridge is gone in about 5 to 7 s. `degraded_reasons` then says `hung on ...`. After the 20 s wait there's a new `lhm-bridge` id, the reasons are empty, and the fan is back at about 60%.

## 6. The bridge's watchdog

This freezes the daemon, so heartbeats stop while the bridge itself is healthy.

```powershell
$d = Get-CimInstance Win32_Process -Filter "Name='thermaliq-daemon.exe'" | Where-Object CommandLine -like '*--service*'
$dp = Get-Process -Id $d.ProcessId
$old = (Get-Process lhm-bridge).Id
[Pause]::NtSuspendProcess($dp.Handle)
$t = [Diagnostics.Stopwatch]::StartNew()
while ((Get-Process -Id $old -ErrorAction SilentlyContinue) -and $t.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 100 }
"bridge gone after $($t.Elapsed.TotalSeconds) s"
Get-Content "$dir\bridge-originals.json"
[Pause]::NtResumeProcess($dp.Handle)
Start-Sleep 20
Show-State
```

**Pass:** the bridge exits in about 5 to 6 s, the file no longer holds the header, and the fan goes back to its BIOS speed while frozen. After resuming, a new bridge runs and the fan is back at about 60%.

Always run the `NtResumeProcess` line, even if something looks wrong.

## 7. Anti-cheat mode, with a fake client

Part A: enter and leave without a restart.

```powershell
function Show-Pawn { pnputil /enum-devices /instanceid 'ROOT\PAWNIO\0000'; "marker: $(Test-Path "$dir\anti-cheat-mode")"; "bridge: $([bool](Get-Process lhm-bridge -ErrorAction SilentlyContinue))" }
Copy-Item "$env:SystemRoot\System32\ping.exe" "$env:TEMP\faceitclient.exe" -Force
$fake = Start-Process "$env:TEMP\faceitclient.exe" -ArgumentList '-n','600','127.0.0.1' -WindowStyle Hidden -PassThru
Start-Sleep 12
Show-Pawn
Show-State
Stop-Process -Id $fake.Id -Force
Start-Sleep 40
Show-Pawn
Show-State
```

**Pass:** while it runs, PawnIO is `Disabled`, the marker is there, no bridge runs, the header is off `/fans`, `unavailable_sources` lists `lhm-bridge`, and the GPU fan still reads. 40 s after it closes, PawnIO is `Started`, the marker is gone, the bridge is back and the fan is at about 60%.

Part B: a service restart in the middle.

```powershell
$fake = Start-Process "$env:TEMP\faceitclient.exe" -ArgumentList '-n','600','127.0.0.1' -WindowStyle Hidden -PassThru
Start-Sleep 12
Restart-Service ThermalIQ; Start-Sleep 10
Show-Pawn
Stop-Process -Id $fake.Id -Force
Start-Sleep 40
Show-Pawn
Restart-Service ThermalIQ; Start-Sleep 15
Show-State
```

**Pass:** after the restart with the client running, PawnIO stays `Disabled` and the marker stays. 40 s after closing it, PawnIO is `Started` and the marker is gone, but there's still no bridge: that's the known limit, and it needs the next service restart. After that restart the fan is at about 60%.

Part C: the marker is set and the client has gone by the time the service starts.

```powershell
$fake = Start-Process "$env:TEMP\faceitclient.exe" -ArgumentList '-n','600','127.0.0.1' -WindowStyle Hidden -PassThru
Start-Sleep 12
Stop-Service ThermalIQ
Stop-Process -Id $fake.Id -Force
Show-Pawn
Start-Service ThermalIQ; Start-Sleep 15
Show-Pawn
Show-State
Remove-Item "$env:TEMP\faceitclient.exe"
```

**Pass:** before the start, PawnIO is `Disabled` and the marker is there. After it, PawnIO is `Started`, the marker is gone and the fan is at about 60%, with no 30 s wait.

## 8. Real FACEIT

This is the only check that needs FACEIT installed. Report what you see: nothing here is assumed to be right yet.

```powershell
Get-Service FACEITService | Select-Object Name, Status, StartType
Get-Process | Where-Object Name -like '*faceit*' | Select-Object Name, Path
```

1. Before opening FACEIT: is `FACEITService` stopped? If it runs all the time, anti-cheat mode would never end, so tell me.
2. Open the desktop app only. The mode should **not** start (`Show-Pawn`).
3. Start the anti-cheat client. Does `faceitclient.exe` appear? Does `Show-Pawn` show PawnIO `Disabled`? Does the client refuse to start before that happens, and does it start on a second try?
4. Close it. Does `FACEITService` stop? If it keeps running, motherboard fans stay on the BIOS until it stops, and we should decide what to do.

## 9. Clean up

```powershell
Stop-Service ThermalIQ
& "$pf\thermaliq-daemon.exe" uninstall-service
Remove-Item "$dir\profiles\m12.toml"
if (Test-Path "$dir\config.toml.m12bak") { Move-Item "$dir\config.toml.m12bak" "$dir\config.toml" -Force } else { Remove-Item "$dir\config.toml" }
Remove-Item -Recurse $pf
```

G3 is separate and still needs a case fan with a tach signal.
