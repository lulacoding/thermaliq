# M15 checks on the Windows PC

**Not run yet.** M15 stays unticked until these pass, CI is green and the review is clean. Paste each step's output into chat and stop at the first failure.

Branch: `codex/m15` (PR #62). Use an **admin** Windows PowerShell 5.1, and close Fan Control, MSI Center and games first. The service must already be installed and mapped as at the end of M13 (`roles.toml` present, `balanced` active, `tq` installed as in M14).

What this does to hardware: app rules and schedules only switch between the bundled profiles (`balanced`, `performance`, `silent`), so fans follow those profiles' curves through the engine, and the safety rules still win. Step 5 holds one case fan at 100% for 5 s (raise-only, the same probe as M13). Step 6 holds it at 70% for up to a minute with an override, then cancels it. Step 7 is G1 again (it injects a 97 °C reading and expects every fan at 100% for 30 s). Each step that touches a fan says how it gets handed back.

| Step | What it proves |
|---|---|
| 1 | Build, install, back up your config, helper functions |
| 2 | App rule: `notepad.exe` in the foreground wins, backgrounded it lets go |
| 3 | Schedule: a window around now wins, one that does not cover now does not |
| 4 | A manual `tq profile set` while the app rule wins: chosen vs effective |
| 5 | `default_profile` survives a restart |
| 6 | `tq identify --probe` while an app rule is active; the probe never swaps the profile |
| 7 | A probe is refused on a fan with a live override (exit 4) |
| 8 | G1 again |
| 9 | Clean up |

The foreground checks need you to click windows. The commands wait for you, so read the **Do** line of each step first.

## 1. Build, install, back up, helpers

```powershell
cd C:\Users\samjo\dev\thermaliq
git fetch; git switch codex/m15; git pull
cargo build -p thermaliq-daemon -p thermaliq-cli
$pf = "$env:ProgramFiles\ThermalIQ"
$dir = "$env:ProgramData\ThermalIQ"
$tq = "$pf\tq.exe"
$api = 'http://127.0.0.1:7272'
Stop-Service ThermalIQ
Wait-Process -Name thermaliq-daemon -Timeout 30 -ErrorAction SilentlyContinue
if (Get-Process thermaliq-daemon -ErrorAction SilentlyContinue) { throw 'thermaliq-daemon is still running: wait or end it, then run this step again' }
Copy-Item target\debug\thermaliq-daemon.exe $pf -Force
Copy-Item target\debug\tq.exe $pf -Force

# Back up your config once (a missing file is fine: step 9 then deletes ours).
if (Test-Path "$dir\config.toml") { Copy-Item "$dir\config.toml" "$dir\config.toml.m15bak" -Force }
$base = if (Test-Path "$dir\config.toml.m15bak") { (Get-Content "$dir\config.toml.m15bak" -Raw) } else { "default_profile = `"balanced`"`n" }

function T { & $tq @args; "exit=$LASTEXITCODE" }
# One line of /status: effective (source: rule); chosen.
function S {
    $s = Invoke-RestMethod "$api/status"
    $rule = if ($s.profile_rule_id) { ": $($s.profile_rule_id)" } else { '' }
    "{0} ({1}{2}); chosen: {3}" -f $s.active_profile, $s.profile_source, $rule, $s.chosen_profile
}
# Waits up to $secs for the effective profile to become $want, then prints S.
function WaitFor($want, $secs = 60) {
    $end = (Get-Date).AddSeconds($secs)
    while ((Get-Date) -lt $end) {
        if ((Invoke-RestMethod "$api/status").active_profile -eq $want) { return (S) }
        Start-Sleep 1
    }
    "TIMED OUT waiting for $want; now: $(S)"
}
# Writes config.toml = your own settings + the extra tables, then restarts the service.
function SetCfg($extra) {
    Set-Content "$dir\config.toml" ($base.TrimEnd() + "`n" + $extra) -Encoding ascii
    Restart-Service ThermalIQ; Start-Sleep 20
}
Start-Service ThermalIQ
Start-Sleep 20
T status
S
```

**Pass:** `T status` prints `state: ok`, `engine: running`, and a `profile:` line `balanced (fallback); chosen: balanced`, `exit=0`. `S` prints `balanced (fallback); chosen: balanced`. (If your `default_profile` is not `balanced`, use that name wherever the steps say `balanced`.)

## 2. App rule: foreground, then background

Do: when the command says so, click the Notepad window and leave it in front. This writes one rule: `notepad.exe` selects `performance` (the exe is matched whole, ignoring case; a Notepad that is only running in the background must not match).

```powershell
SetCfg @"
[[app_rule]]
id = "notepad"
exe = "notepad.exe"
profile = "performance"
"@
T config show
S
Start-Process notepad
Start-Sleep 3
@((Get-Process notepad -ErrorAction SilentlyContinue)).Count
"Click the Notepad window now and leave it in front."
WaitFor performance 60
T status
```

**Pass:** `T config show` prints an `app_rule = ...` line with the rule. Before you click, `S` is `balanced (fallback); chosen: balanced`. After you click Notepad, `WaitFor` prints `performance (app: notepad); chosen: balanced` within about 15 s (the daemon waits for a steady snapshot before it switches), and `T status` shows the same on its `profile:` line. The Notepad count is at least `1`.

Now put it in the background (Notepad stays running):

```powershell
"Click this PowerShell window now (Notepad stays open)."
WaitFor balanced 60
@((Get-Process notepad -ErrorAction SilentlyContinue)).Count
```

**Pass:** `WaitFor` prints `balanced (fallback); chosen: balanced` within about 15 s, and the Notepad count is still at least `1`: a background process does not hold the rule. Leave Notepad open for step 4.

## 3. Schedule: covering now, then not

No app rule here, so nothing else competes. The window is in your local time (the daemon reads the Windows clock each second).

```powershell
$now = Get-Date
$in = "$($now.AddMinutes(-5).ToString('HH:mm'))-$($now.AddMinutes(5).ToString('HH:mm'))"
"covering window: $in"
SetCfg @"
[[schedule]]
id = "around-now"
time = "$in"
profile = "silent"
"@
WaitFor silent 60

$now = Get-Date
$out = "$($now.AddMinutes(30).ToString('HH:mm'))-$($now.AddMinutes(40).ToString('HH:mm'))"
"window that does not cover now: $out"
SetCfg @"
[[schedule]]
id = "later"
time = "$out"
profile = "silent"
"@
WaitFor balanced 60
```

**Pass:** the first `WaitFor` prints `silent (schedule: around-now); chosen: balanced`. After the second restart the second `WaitFor` prints `balanced (fallback); chosen: balanced`. If you run this within 40 minutes of midnight the second window wraps past midnight; that is fine, it still does not cover now.

## 4. Manual `tq profile set` while the app rule wins

Do: after you start the command, click Notepad within 10 s and leave it in front; the output appears in the PowerShell window by itself.

```powershell
SetCfg @"
[[app_rule]]
id = "notepad"
exe = "notepad.exe"
profile = "performance"
"@
"Click the Notepad window within 10 s and leave it in front."
Start-Sleep 10
WaitFor performance 60
T profile set silent
S
Get-Content "$dir\config.toml" | Select-String default_profile
```

**Pass:** `T profile set silent` prints two lines, `chosen: silent` and `effective: performance (app: notepad)`, `exit=0` (the manual choice did not preempt the rule). `S` prints `performance (app: notepad); chosen: silent`. `config.toml` already says `default_profile = "silent"`.

Now let go of the rule:

```powershell
"Click this PowerShell window now."
WaitFor silent 60
T status
```

**Pass:** `WaitFor` prints `silent (fallback); chosen: silent`: with the app rule gone, your choice is what is in effect.

## 5. `default_profile` survives a restart

```powershell
Get-Content "$dir\config.toml" | Select-String default_profile
Restart-Service ThermalIQ; Start-Sleep 20
S
T profile set balanced
Get-Content "$dir\config.toml" | Select-String default_profile
Restart-Service ThermalIQ; Start-Sleep 20
S
```

**Pass:** the first line shows `default_profile = "silent"`; after the restart `S` is `silent (fallback); chosen: silent`. After `T profile set balanced` (it prints `chosen: balanced` and `effective: balanced (fallback)`) the file says `balanced`, and after the second restart `S` is `balanced (fallback); chosen: balanced`. The `notepad` rule from step 4 is still in `config.toml` (`T config show`).

## 6. `identify --probe` while an app rule is active

The rule must stay in effect for the whole probe, so the status is read inside the same command, while Notepad stays in front. Pick System Fan #1 (`lhm_nct6687d_0_fan_2`), a case fan.

```powershell
$fan = 'lhm_nct6687d_0_fan_2'
"Click the Notepad window within 10 s and leave it in front until the output appears."
Start-Sleep 10
"before: $(S)"
T identify --probe $fan
"after:  $(S)"
```

**Pass:** `before:` is `performance (app: notepad); chosen: balanced`. The probe prints `Running 'System Fan #1' (lhm_nct6687d_0_fan_2) at 100% for 5 s...`, a pairing line (probably "no tach moved" on this PC), whether the write landed, and `Nothing was saved.`, `exit=0`. `after:` is again `performance (app: notepad); chosen: balanced`: the probe did not swap the profile, and the app rule was not undone. Check later with the shell in front that the fan is back on its profile curve:

```powershell
Start-Sleep 5
T fans
```

**Pass:** `SENT%` for the fan equals `CURVE%` (or the profile's floor).

## 7. A probe is refused on a fan with a live override (exit 4)

```powershell
T override $fan 70 --secs 60
Start-Sleep 3
T fans
T identify --probe $fan
Start-Sleep 3
T fans
T override $fan --cancel
Start-Sleep 3
T fans
```

**Pass:** the override prints `lhm_nct6687d_0_fan_2: 70% for 60 s`, `exit=0`, and `fans` shows `SENT%` 70. The probe prints an error that contains `fan has a live override: 'lhm_nct6687d_0_fan_2'` and ends with `exit=4`; it prints no `Running ... at 100%` line. `fans` still shows `SENT%` 70 (the fan never went to 100%). After `--cancel` it prints `override cancelled` and three seconds later the fan is back on its curve. If the override is still held after the minute, run `T override $fan --cancel` again; the daemon also expires it by itself.

## 8. G1 again

`g1.ps1` installs its own service, so remove ours first. It writes its own `roles.toml` and `config.toml` and puts yours back at the end. Put your own config back first, so it backs up the right file:

```powershell
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process
if (Test-Path "$dir\config.toml.m15bak") { Copy-Item "$dir\config.toml.m15bak" "$dir\config.toml" -Force } else { Remove-Item "$dir\config.toml" -ErrorAction SilentlyContinue }
Stop-Service ThermalIQ
& "$pf\thermaliq-daemon.exe" uninstall-service
Wait-Process -Name thermaliq-daemon -Timeout 30 -ErrorAction SilentlyContinue
.\scripts\g1.ps1
Get-Content "$dir\roles.toml"
```

**Pass:** the script ends `G1: PASS`, every step `PASS`, and the last line before the summary is `fan: automatic`. `roles.toml` is yours again afterwards.

## 9. Clean up

Reinstall the service as it was (G1 removed it), restore your config, and check nothing is left held.

```powershell
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process
& "$pf\thermaliq-daemon.exe" install-service
Start-Sleep 5
if (Test-Path "$dir\config.toml.m15bak") { Copy-Item "$dir\config.toml.m15bak" "$dir\config.toml" -Force; Remove-Item "$dir\config.toml.m15bak" } else { Remove-Item "$dir\config.toml" -ErrorAction SilentlyContinue }
Restart-Service ThermalIQ; Start-Sleep 20
S
T override $fan --cancel
@((Get-Process lhm-bridge -ErrorAction SilentlyContinue)).Count
Get-Service ThermalIQ
```

**Pass:** `S` is `balanced (fallback); chosen: balanced` (or your own `default_profile`). `T override $fan --cancel` prints an error saying the fan has no active override, `exit=4` (nothing is held). The service is `Running`, and every fan is on its profile curve. If `install-service` complains that the service exists, G1 did not remove it: skip that line.
