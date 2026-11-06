# M13 checks on the Windows PC

**Not run yet.** Do these after reviewing #59 and #60. Paste each step's output into chat and stop at the first failure.

Branch: `m13-identify` (it holds #59 and #60). Use an **admin** PowerShell, and close Fan Control, MSI Center and games first. Don't commit `out\` dumps.

What this does to hardware: By default `identify` probes nothing: it reads, then writes `roles.toml`. Only `identify --probe <fan-id>` runs a fan (100% for 5 s, only ever a raise), and it saves nothing. Each step that touches a fan says how it gets handed back.

| Step | What it proves |
|---|---|
| 1 | Install, first-run inventory baseline, nothing mapped yet |
| 2 | `identify` proposes `case_fans` (System Fan #1-#6) and Enter saves |
| 3 | `identify --probe`: the agent is paused during it and running again after |
| 4 | Ctrl-C in the middle of a probe still puts the profile back |
| 5 | After a restart the mapping loads and every `case_fans` member follows the curve, at 30% or more |
| 6 | A changed board unmaps every role and says what to run |
| 7 | `g1.ps1` on the mapped bundled `balanced` profile |
| 8 | Clean up |

## 1. Install, and look at the first run

```powershell
cd C:\Users\samjo\dev\thermaliq
git fetch; git switch m13-identify; git pull
cargo build -p thermaliq-daemon
$pf = "$env:ProgramFiles\ThermalIQ"
$dir = "$env:ProgramData\ThermalIQ"
$api = 'http://127.0.0.1:7272'
if (Get-Service ThermalIQ -ErrorAction SilentlyContinue) { Stop-Service ThermalIQ; & "$PWD\target\debug\thermaliq-daemon.exe" uninstall-service }
Wait-Process -Name thermaliq-daemon -Timeout 30 -ErrorAction SilentlyContinue
if (Get-Process thermaliq-daemon -ErrorAction SilentlyContinue) { throw 'thermaliq-daemon is still running: wait or end it, then run this step again' }
Get-Process lhm-bridge -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item -Recurse -Force "$pf\lhm-bridge" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $pf | Out-Null
Copy-Item target\debug\thermaliq-daemon.exe $pf -Force
dotnet publish bridges\lhm-bridge\LhmBridge\LhmBridge.csproj -c Release -r win-x64 --self-contained -o "$pf\lhm-bridge"
# Keep what is there, then start from nothing: no roles, no inventory, the bundled balanced profile.
foreach ($f in 'roles.toml', 'inventory.json', 'config.toml') {
    if (Test-Path "$dir\$f") { Copy-Item "$dir\$f" "$dir\$f.m13bak" -Force; Remove-Item "$dir\$f" }
}
Remove-Item "$dir\service.log" -ErrorAction SilentlyContinue
[IO.File]::WriteAllText("$dir\config.toml", "default_profile = `"balanced`"`n")
& "$pf\thermaliq-daemon.exe" install-service
Start-Service ThermalIQ
Start-Sleep 20
function Show-Identity {
    $v = Invoke-RestMethod "$api/identity"
    "board: $($v.board)"
    'roles:'; $v.roles | Format-List
    "unmapped_roles: $($v.unmapped_roles -join ', ')"
    'inventory:'; $v.inventory | Format-List
    'problems:'; $v.problems
    'proposals:'; $v.proposals | Format-Table
    'fans:'; $v.fans | Format-Table id, label, kind, rpm, controllable
}
function Pause-Agent {
    $h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
    Invoke-RestMethod "$api/agent/pause" -Method Post -Headers $h | Out-Null
    "agent: $((Invoke-RestMethod "$api/status").agent)"
}
Show-Identity
(Invoke-RestMethod "$api/status") | Select-Object degraded, degraded_reasons, active_profile | Format-List
Get-Content "$dir\inventory.json"
Get-Content "$dir\service.log"
```

**Pass:** `board` names your B550M board, `inventory.json` exists and lists every fan, `inventory` shows nothing new/missing/changed, `roles` is empty, and `unmapped_roles` lists all six roles (`cpu_fan`, `pump`, `gpu_fan`, `case_fans`, `cpu_temp`, `gpu_temp`). `problems` has **one line per role** (not one per profile) naming the profiles that use it. `proposals` lists `gpu_fan`, `cpu_fan` (CPU Fan), `pump` (Pump Fan), a CPU and a GPU sensor, and `case_fans` with System Fan #1 to #6 (even though they read 0 RPM). `degraded` is **true** with `degraded_reasons` = `profile 'balanced' controls no fans: run thermaliq-daemon identify`, because nothing is mapped yet. `active_profile` is `balanced`. The service log says "first run: saved the fan inventory".

## 2. Identify

First answer `n` to see that nothing is written, then run it again and press Enter:

```powershell
& "$pf\thermaliq-daemon.exe" identify
Test-Path "$dir\roles.toml"
& "$pf\thermaliq-daemon.exe" identify
Get-Content "$dir\roles.toml"
```

**Expect:** each run prints the board and the problems, then the proposals (`case_fans <- ...` once per System Fan), then "Roles to save:" with `case_fans = lhm_nct6687d_0_fan_2, ...` and "Save these? [Y/n]". It does **not** run any fan and asks nothing about front/rear/top. On the first run type `n`: "Nothing was saved." On the second press **Enter**: "Saved. Restart the ThermalIQ service...".

**Pass:** `Test-Path` says `False` after the `n`. After the Enter, `roles.toml` has `gpu_fan`, `cpu_fan`, `pump`, `cpu_temp`, `gpu_temp` as single ids and `case_fans` as a list of the System Fan headers. The pump header (`lhm_nct6687d_0_fan_1`, "Pump Fan") is **not** in `case_fans`.

## 3. `identify --probe`, and the agent during it

This runs one System Fan at 100% for 5 s. It reports the tach pairing and saves nothing. System Fan #1 is `lhm_nct6687d_0_fan_2`.

In a **second** admin PowerShell, start this first:

```powershell
$api = 'http://127.0.0.1:7272'
1..40 | ForEach-Object { "{0:HH:mm:ss.f}  agent={1}" -f (Get-Date), (Invoke-RestMethod "$api/status").agent; Start-Sleep -Milliseconds 500 }
```

Then, in the first window:

```powershell
Get-Content "$dir\roles.toml" > "$env:TEMP\roles.before"
& "$pf\thermaliq-daemon.exe" identify --probe lhm_nct6687d_0_fan_2
(Compare-Object (Get-Content "$env:TEMP\roles.before") (Get-Content "$dir\roles.toml")).Count
```

**Pass:** the command prints "Running 'System Fan #1' (lhm_nct6687d_0_fan_2) at 100% for 5 s...", then a pairing line (on this PC probably "no tach moved", since the header reads 0 RPM), then "Nothing was saved." The last line is `0`. In the second window: `agent=running`, then `agent=paused` for about 5 s, then `agent=running` again. Listen: that fan goes up for the 5 s and comes back.

Then pause the agent yourself and probe again:

```powershell
$h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
Invoke-RestMethod "$api/agent/pause" -Method Post -Headers $h | Out-Null
& "$pf\thermaliq-daemon.exe" identify --probe lhm_nct6687d_0_fan_2
(Invoke-RestMethod "$api/status").agent
Invoke-RestMethod "$api/agent/resume" -Method Post -Headers $h | Out-Null
(Invoke-RestMethod "$api/status").agent
```

**Pass:** the first status line says `paused` (a probe resumes the agent only if it paused it), the second `running`.

## 4. Ctrl-C in the middle of a probe

```powershell
& "$pf\thermaliq-daemon.exe" identify --probe lhm_nct6687d_0_fan_2
```

Hear the fan go up and press **Ctrl-C** about 2 s in. Then:

```powershell
Start-Sleep 8
(Invoke-RestMethod "$api/status") | Select-Object active_profile, degraded, degraded_reasons, agent | Format-List
Invoke-RestMethod "$api/fans" | Select-Object id, rpm, duty_percent, curve_duty | Format-Table
Get-Content "$dir\service.log" -Tail 20
& "$pf\thermaliq-daemon.exe" identify --probe lhm_nct6687d_0_fan_2
```

**Pass:** the fan drops back within a few seconds of the 5 s ending, `active_profile` is `balanced`, `agent` is `running`, nothing in `degraded_reasons` says "could not put profile back", and the last command runs normally (it must not say another probe is running; if it does, wait 2 s and try again once, then report it).

## 5. Restart: the mapping loads, and the case fans have a floor

```powershell
Restart-Service ThermalIQ
Start-Sleep 20
Show-Identity
(Invoke-RestMethod "$api/status") | Select-Object degraded, degraded_reasons, active_profile | Format-List
$ids = @((Invoke-RestMethod "$api/identity").roles.case_fans)
$case = @(Invoke-RestMethod "$api/fans" | Where-Object { $ids -contains $_.id })
$case | Select-Object id, label, rpm, duty_percent, curve_duty | Format-Table
"case_fans members: $($ids.Count); with curve_duty >= 30: $(@($case | Where-Object { $null -ne $_.curve_duty -and $_.curve_duty -ge 30 }).Count)"
Get-Content "$dir\service.log" -Tail 30
```

**Pass:** `roles` matches `roles.toml`, `inventory` shows nothing new/missing/changed, `degraded` is false, and `problems` names no role. The two numbers on the "case_fans members" line are equal, and they equal the number of System Fans (6 here). Every member has a `curve_duty` of 30 or more, even at a cool idle, where the bundled `balanced` curve alone would ask for less.

## 6. A changed board

This fakes it by editing the saved board name. The real board is unchanged.

```powershell
Stop-Service ThermalIQ
$inv = Get-Content "$dir\inventory.json" -Raw | ConvertFrom-Json
$inv.board = 'SOME OTHER BOARD'
[IO.File]::WriteAllText("$dir\inventory.json", ($inv | ConvertTo-Json -Depth 6))
Start-Service ThermalIQ
Start-Sleep 20
(Invoke-RestMethod "$api/status") | Select-Object degraded, degraded_reasons | Format-List
(Invoke-RestMethod "$api/identity") | Select-Object board, roles, unmapped_roles, problems | Format-List
Invoke-RestMethod "$api/fans" | Select-Object id, rpm, duty_percent, curve_duty | Format-Table
```

**Pass:** `degraded` is true and `degraded_reasons` has two lines: the board changed (both boards named, and to run `thermaliq-daemon identify`), and `profile 'balanced' controls no fans: run thermaliq-daemon identify`. `roles` is empty and every role is in `unmapped_roles`, even though `roles.toml` still has your mapping. No fan has a `curve_duty` any more (the BIOS has them).

Restart once more, to see it stays that way, then accept:

```powershell
Restart-Service ThermalIQ; Start-Sleep 20
(Invoke-RestMethod "$api/status").degraded_reasons
$h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
Invoke-RestMethod "$api/inventory/accept" -Method Post -Headers $h
Restart-Service ThermalIQ; Start-Sleep 20
(Invoke-RestMethod "$api/status") | Select-Object degraded, degraded_reasons | Format-List
(Invoke-RestMethod "$api/identity") | Select-Object board, roles | Format-List
```

**Pass:** the first line still says the board changed; after accepting and restarting, `degraded` is false, `board` is your real board, and `roles` is the mapping from step 2 again.

## 7. G1 on the mapped `balanced` profile

`g1.ps1` installs its own service, so remove ours first. It writes its own `roles.toml` and `config.toml` (GPU fan and temperature on the first NVIDIA GPU) and puts yours back at the end.

```powershell
Stop-Service ThermalIQ
& "$pf\thermaliq-daemon.exe" uninstall-service
Wait-Process -Name thermaliq-daemon -Timeout 30 -ErrorAction SilentlyContinue
.\scripts\g1.ps1
Get-Content "$dir\roles.toml"
```

**Pass:** the script ends `G1: PASS`, every step `PASS`, and the last line before the summary is `fan: automatic`. `roles.toml` is yours again afterwards.

## 8. Clean up

```powershell
if (Get-Service ThermalIQ -ErrorAction SilentlyContinue) { Stop-Service ThermalIQ; & "$pf\thermaliq-daemon.exe" uninstall-service }
foreach ($f in 'roles.toml', 'inventory.json', 'config.toml') {
    Remove-Item "$dir\$f" -ErrorAction SilentlyContinue
    if (Test-Path "$dir\$f.m13bak") { Move-Item "$dir\$f.m13bak" "$dir\$f" }
}
Get-Process lhm-bridge -ErrorAction SilentlyContinue
Get-Service ThermalIQ -ErrorAction SilentlyContinue
```

**Pass:** nothing is printed by the last two commands, and every fan is on the BIOS or driver curve.
