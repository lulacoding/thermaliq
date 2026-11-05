# M13 checks on the Windows PC

**Not run yet.** Do these after reviewing #59 and #60. Paste each step's output into chat and stop at the first failure.

Branch: `m13-identify` (it holds #59 and #60). Use an **admin** PowerShell, and close Fan Control, MSI Center and games first. Don't commit `out\` dumps.

What this does to hardware: `identify` runs one fan at 100% for 5 s at a time, and only ever raises a fan. It never probes a pump. Everything else only reads. Each step that touches a fan says how it gets handed back.

| Step | What it proves |
|---|---|
| 1 | Install, first-run inventory baseline, no roles yet |
| 2 | `identify` end to end: proposals, a probe, the tach pairing, the save |
| 3 | The agent is paused during a probe and running again after |
| 4 | Ctrl-C in the middle of a probe still puts the profile back |
| 5 | After a restart the mapping loads and `/identity` shows no problems |
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

**Pass:** `board` names your B550M board, `inventory.json` exists and lists every fan, `inventory` shows nothing new/missing/changed, `roles` is empty, `unmapped_roles` lists all eight roles, and `problems` names each unmapped role (the case fans, `cpu_fan`, and so on) by name. `proposals` includes the GPU fan as `gpu_fan` and a CPU or GPU sensor. `active_profile` is `balanced`. The service log says "first run: saved the fan inventory".

## 2. Identify

Pick the window so you can hear the case fans. Keep the agent running here (step 3 needs it).

```powershell
& "$pf\thermaliq-daemon.exe" identify
```

It lists the proposals, then goes through each fan with no role. For each: answer `y` (or Enter) to the probe, listen for 5 s, and answer `front`, `rear`, `top` or `none`. It never offers a pump. Answer `q` once to see that it stops with "Nothing was saved." (then run it again). On the final "Save these? [Y/n]", Enter or `y` saves; `n` saves nothing.

Then:

```powershell
Get-Content "$dir\roles.toml"
Get-Content "$dir\inventory.json" | Select-Object -First 3
```

**Pass:** each probed fan's pairing line matches what you heard (the NCT6687D fans should say "its own tach rose"). Nothing was written before the final prompt. `roles.toml` has exactly what the summary showed. After the prompt, the fan you probed is back where it was (on the BIOS, since the profile does not control it yet).

## 3. The agent is paused during a probe

In a second admin PowerShell, start this first, then in the first window run `& "$pf\thermaliq-daemon.exe" identify` and answer `y` to one probe, but with an unmapped fan left (reset it first with `Remove-Item "$dir\roles.toml"; Restart-Service ThermalIQ; Start-Sleep 20` if step 2 mapped them all):

```powershell
$api = 'http://127.0.0.1:7272'
1..40 | ForEach-Object { "{0:HH:mm:ss.f}  agent={1}" -f (Get-Date), (Invoke-RestMethod "$api/status").agent; Start-Sleep -Milliseconds 500 }
```

**Pass:** `agent=running`, then `agent=paused` for about 5 s while the fan is at 100%, then `agent=running` again.

Then pause the agent yourself and probe again:

```powershell
Pause-Agent
& "$pf\thermaliq-daemon.exe" identify
(Invoke-RestMethod "$api/status").agent
```

**Pass:** the last line says `paused`: a probe resumes the agent only if it paused it.

## 4. Ctrl-C in the middle of a probe

Needs an unmapped, controllable case fan (use the reset from step 3 if needed).

```powershell
Invoke-RestMethod "$api/agent/resume" -Method Post -Headers @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" } | Out-Null
& "$pf\thermaliq-daemon.exe" identify
```

Answer `y` to a probe, hear the fan go up, and press **Ctrl-C** about 2 s in. Then:

```powershell
Start-Sleep 8
(Invoke-RestMethod "$api/status") | Select-Object active_profile, degraded, degraded_reasons, agent | Format-List
Invoke-RestMethod "$api/fans" | Select-Object id, rpm, duty_percent | Format-Table
Get-Content "$dir\service.log" -Tail 20
```

**Pass:** the fan drops back within a few seconds of the 5 s ending (without Ctrl-C-ing the service), `active_profile` is `balanced`, `agent` is `running`, nothing in `degraded_reasons` says "could not put profile back", and `/identity` still answers. Run `identify` once more and probe the same fan: it must not say another probe is running.

## 5. Restart: the mapping loads

Run `identify` again and map at least the fans you can, then save. Then:

```powershell
Restart-Service ThermalIQ
Start-Sleep 20
Show-Identity
(Invoke-RestMethod "$api/status") | Select-Object degraded, degraded_reasons, active_profile | Format-List
Invoke-RestMethod "$api/fans" | Select-Object id, label, rpm, duty_percent, curve_duty | Format-Table
Get-Content "$dir\service.log" -Tail 30
```

**Pass:** `roles` matches `roles.toml`, `inventory` shows nothing new/missing/changed, `degraded` is false, and `problems` names only the roles you answered `none` for or left unmapped. The fans you mapped follow the `balanced` curve (`curve_duty` filled in, `duty_percent` close to it).

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

**Pass:** `degraded` is true and `degraded_reasons` says the board changed, names both boards, and says to run `thermaliq-daemon identify`. `roles` is empty and every role is in `unmapped_roles`, even though `roles.toml` still has your mapping. The case fans you had mapped have no `curve_duty` any more (the BIOS has them).

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

**Pass:** the first line still says the board changed; after accepting and restarting, `degraded` is false, `board` is your real board, and `roles` is the mapping from step 5 again.

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
