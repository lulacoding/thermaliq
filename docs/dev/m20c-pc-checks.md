# M20c checks on the Windows PC

**Results, 2026-11-05, RTX 4060 PC: all passed.**

| Step | Result |
| --- | --- |
| 1. Service and helper | `--service` as SYSTEM, `foreground-helper` as the user, `foreground` true |
| 2. Profiles | Fan followed the rule's profiles on `nvml_gpu0_fan0` |
| 3. Switch and refusal | Switched to `gaming` within 1 s; bundled `silent` refused: `profile 'silent' does not control fan 'nvml_gpu0_fan0'` |
| 4. Key and hosted | Stored in SYSTEM's vault; `msinfo32.exe` idle 0.90 and `windowsterminal.exe` idle 0.83 on real states; a wrong key shows `HTTP 401: User not found.` in `hosted_error` |
| 5. Model and ACL check | `configured` on a real install under `%ProgramFiles%` |
| 6. Local when idle | 23.2 s to answer, including load; peak 7,480 MB; `BelowNormal`; unloaded. `charmap.exe` idle 0.38, so it asked |
| 7. Fullscreen and crash | A borderless fullscreen window stopped the model in 1.0 s; hard-killing the service left 0 `ollaya`, and it restarted |
| 8. G1 | PASS (latch released after 31.3 s, restart 1.6 s, automatic after 0.3 s) |

Not observed: the 6-thread limit itself (set by the code; the run's average was 1.5 cores, mostly loading). These results are from `b5d9ec5` plus the `hosted_error` change.

Do these in order, on branch `m20c-slow-layer`. M20c is ticked only after they pass. These steps control the real GPU fan and spend a little OpenRouter credit. Step 8 removes the service at the end.

Close games and video first. Use an **admin** PowerShell unless a step says otherwise. Apps started from that window run elevated: if one never shows up in the log, open it from the Start menu instead, and tell me, because the helper should be able to read it.

## 1. Build and start the service

Tests: the helper starts in your session, with your rights.

```powershell
cd C:\Users\samjo\dev\thermaliq
git switch m20c-slow-layer; git pull
cargo build -p thermaliq-daemon
$exe = "$PWD\target\debug\thermaliq-daemon.exe"
$dir = "$env:ProgramData\ThermalIQ"
& $exe install-service
Start-Service ThermalIQ
Start-Sleep 8
$api = 'http://127.0.0.1:7272'
$h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
Invoke-RestMethod "$api/agent/decisions" -Headers $h | Select-Object foreground, local, hosted_account, crashed
Get-CimInstance Win32_Process -Filter "Name='thermaliq-daemon.exe'" | ForEach-Object { "{0}  user={1}  {2}" -f $_.ProcessId, (Invoke-CimMethod -InputObject $_ -MethodName GetOwner).User, $_.CommandLine }
```

**Pass:** `foreground` is `True`, `crashed` is `False`, `local` is `not configured`. There are two processes: `--service` with user `SYSTEM`, and `foreground-helper` with your user name.

## 2. Add profiles the rule can use

Tests: the rule's profile names, with this PC's fan and sensor ids. The bundled ones are refused until M13.

```powershell
New-Item -ItemType Directory -Force "$dir\profiles" | Out-Null
$fans = @((Invoke-RestMethod "$api/fans") | Where-Object id -like 'nvml_*').id
$controls = ($fans | ForEach-Object { "[[controls]]`nfan = `"$_`"`ncurve = `"c`"`nstep_up_pct = 100`nstep_down_pct = 100`n" }) -join ''
foreach ($p in @{ silent = 30; efficiency = 35; balanced = 40; gaming = 50; performance = 60 }.GetEnumerator()) {
    Set-Content -Encoding ascii "$dir\profiles\$($p.Key).toml" "name = `"$($p.Key)`"`n[[curves]]`ntype = `"linear`"`nid = `"c`"`nsource = `"nvml_gpu0_temp`"`nmin_temp = 30.0`nmax_temp = 90.0`nmin_pwm = $($p.Value)`nmax_pwm = 100`n$controls"
}
Set-Content -Encoding ascii "$dir\config.toml" 'default_profile = "balanced"'
Restart-Service ThermalIQ
Start-Sleep 8
(Invoke-RestMethod "$api/fans") | Select-Object id, curve_duty, commanded_duty
```

**Pass:** every `nvml_` fan shows a `curve_duty`.

## 3. The switch by the rule, and the refusal

Tests: a rule-selected switch goes through the engine, and a profile that would release the fan is refused. No key is needed.

```powershell
notepad
Start-Sleep 3
Invoke-RestMethod "$api/agent/workloads/notepad.exe" -Method Put -Headers $h -ContentType application/json -Body '{"workload":"gaming"}'
```

Click on Notepad so it's in front, wait 5 s, then come back:

```powershell
(Invoke-RestMethod "$api/profiles") | Where-Object active | Select-Object name
(Invoke-RestMethod "$api/agent/decisions" -Headers $h).decisions | Select-Object -Last 3 | Format-List
```

**Pass:** the active profile is `gaming`, and the last entry for `notepad.exe` has source `user` and profile `gaming`.

Now the refusal. Moving `silent.toml` away leaves the bundled `silent`, which uses role ids:

```powershell
Move-Item "$dir\profiles\silent.toml" "$dir\silent.toml.off"
Restart-Service ThermalIQ
Start-Sleep 8
$h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
Invoke-RestMethod "$api/agent/workloads/notepad.exe" -Method Put -Headers $h -ContentType application/json -Body '{"workload":"idle"}'
```

Click on Notepad, wait 5 s, then come back:

```powershell
(Invoke-RestMethod "$api/profiles") | Where-Object active | Select-Object name
(Invoke-RestMethod "$api/agent/decisions" -Headers $h).decisions | Select-Object -Last 2 | Format-List
(Invoke-RestMethod "$api/fans") | Select-Object id, curve_duty, commanded_duty
Move-Item "$dir\silent.toml.off" "$dir\profiles\silent.toml"
Restart-Service ThermalIQ
```

**Pass:** the active profile is not `silent`, a `note` says `does not control fan` or `controls unknown fan`, and the fan still has a `curve_duty`.

## 4. The key, and a real hosted answer

Tests: storing the key in SYSTEM's vault, and hosted on a real exe-centric state. This costs about $0.00002 per new exe.

A PowerShell profile can hold an old key, and `$env:OPENROUTER_API_KEY` may be it. So this asks for the full key without echoing it, checks it with OpenRouter, and stores it only if it's accepted:

```powershell
$api = 'http://127.0.0.1:7272'
$s = Read-Host -AsSecureString 'OpenRouter key'
$k = [Net.NetworkCredential]::new('', $s).Password
try {
    $info = (Invoke-RestMethod 'https://openrouter.ai/api/v1/key' -Headers @{ Authorization = "Bearer $k" }).data
    "OpenRouter accepts this key: $($info.label)"
    $h = @{ Authorization = "Bearer $((Get-Content "$env:ProgramData\ThermalIQ\token.txt" -Raw).Trim())" }
    Invoke-RestMethod "$api/agent/key" -Method Put -Headers $h -ContentType application/json -Body (@{ provider = 'openrouter'; key = $k } | ConvertTo-Json)
} catch { "NOT stored: $($_.Exception.Message)" }
Remove-Variable k, s
Start-Process winver
```

Keep the new window in front for 15 s (a newly started app comes to the front; a script can't always bring back one already running), then come back:

```powershell
$v = Invoke-RestMethod "$api/agent/decisions" -Headers $h
$v.hosted_account
$v.hosted_error
$v.decisions | Select-Object -Last 3 | Format-Table exe, workload, source, confidence, asked, profile, note
```

**Pass:** `hosted_account` is `openrouter`, `hosted_error` is empty, and the app has source `hosted`. A confidence under 0.7 shows `asked` as `True`, which is also correct. `hosted_error` says why hosted isn't answering, for example `HTTP 401: User not found.` for a wrong key. If the key step returns 500, the SYSTEM vault refused it.

## 5. Put the Strands model where the service reads it

The service reads models from `<data dir>\models` (`C:\ProgramData\ThermalIQ\models`), not from your profile's `~\.ollaya\models`. It only runs an exe under `%ProgramFiles%` that nobody but administrators can replace. You don't need to download the model again.

```powershell
Get-Process ollaya* -ErrorAction SilentlyContinue | Stop-Process -Force
$ollaya = "$env:ProgramFiles\Ollaya"
Copy-Item "$env:LOCALAPPDATA\ollaya-spike\bin" $ollaya -Recurse
# Ownership first: on some PCs a file's creator gets a write entry that only this clears.
icacls $ollaya /setowner "*S-1-5-32-544" /T /C /Q
icacls $ollaya /reset /T /C /Q

robocopy "$env:USERPROFILE\.ollaya\models" "$dir\models" /E /NFL /NDL /NJH /NJS
icacls "$dir\models" /setowner "*S-1-5-32-544" /T /C /Q

# Confirms the copy is complete, and fills any gap.
$env:OLLAYA_MODELS = "$dir\models"; $env:OLLAYA_DEVICE = 'cpu'
& "$ollaya\bin\ollaya.exe" pull strands-decider:2b
& "$ollaya\bin\ollaya.exe" list
Get-Process ollaya* -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item Env:OLLAYA_MODELS, Env:OLLAYA_DEVICE

Set-Content -Encoding ascii "$dir\local-model.toml" "exe = '$ollaya\bin\ollaya.exe'`nmodel = 'strands-decider:2b'`n"
Restart-Service ThermalIQ
Start-Sleep 8
$h = @{ Authorization = "Bearer $((Get-Content "$dir\token.txt" -Raw).Trim())" }
(Invoke-RestMethod "$api/agent/decisions" -Headers $h).local
```

`robocopy` exits with 1 when it copied files, which is success. The `pull` should finish quickly, and `list` must show `strands-decider:2b`.

**Pass:** `local` is `configured`. If it says `unusable: others could replace the exe: ...`, the message names the file and who could write it. Send it to me.

## 6. The local model when idle

Tests: the local model on half the cores at below-normal priority, and that it unloads.

Remove the key, because hosted answers first. Start the watcher in this admin window:

```powershell
Invoke-RestMethod "$api/agent/key" -Method Delete -Headers $h
Start-Process charmap
while ($true) {
    $o = @(Get-Process ollaya* -ErrorAction SilentlyContinue)
    $d = (Invoke-RestMethod "$api/agent/decisions" -Headers $h).decisions | Select-Object -Last 1
    "{0:HH:mm:ss} ollaya={1} mem={2}MB prio={3} | last: {4} {5} {6} {7}" -f (Get-Date), $o.Count, [int](($o | Measure-Object WorkingSet64 -Sum).Sum / 1MB), ($o | Select-Object -First 1).PriorityClass, $d.exe, $d.source, $d.workload, $d.confidence
    Start-Sleep 5
}
```

Click on Character Map and leave the PC alone for at least 5 minutes, then 2 more. Press Ctrl-C in the watcher when `charmap.exe` shows `local`.

**Pass:** `ollaya` appears at `BelowNormal` after about 5 minutes, then goes back to 0 once `charmap.exe` shows source `local`. A low confidence with `asked` true is expected from the spike. **Tell me how long `ollaya` was present** and the peak memory: that's the half-core measure the spike didn't have.

## 7. A game arriving, and a crash

Tests: the local model stops for a game it doesn't recognise, and it can't outlive the daemon. Each part needs a new exe and another 5 idle minutes.

**Game arriving.** Open Edge first, then:

```powershell
Start-Process msinfo32
while (-not (Get-Process ollaya* -ErrorAction SilentlyContinue)) { Start-Sleep 1 }
"LOADING at $(Get-Date -Format HH:mm:ss)"
while (Get-Process ollaya* -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 200 }
"GONE at $(Get-Date -Format HH:mm:ss)"
```

Click on System Information and wait about 5 minutes. When `LOADING` prints, switch to Edge and press F11 at once, which counts as a fullscreen app.

**Pass:** `GONE` follows within about 2 s, well short of the usual load time. Press F11 again afterwards.

**Crash.** Again, in this window:

```powershell
Start-Process dxdiag
while (-not (Get-Process ollaya* -ErrorAction SilentlyContinue)) { Start-Sleep 1 }
Start-Sleep 5
Stop-Process -Id (Get-CimInstance Win32_Service -Filter "Name='ThermalIQ'").ProcessId -Force
Start-Sleep 2
"ollaya left: $(@(Get-Process ollaya* -ErrorAction SilentlyContinue).Count)"
Start-Sleep 5
"service: $((Get-Service ThermalIQ).Status)"
```

Click on DirectX Diagnostic Tool and wait about 5 minutes.

**Pass:** `ollaya left: 0`, and the service is `Running` again. This covers the job object, including Ollaya's runner process, which CI can't.

## 8. Clean up, then G1

G1 installs its own build of the service and removes it when finished.

```powershell
& $exe uninstall-service
Remove-Item "$dir\local-model.toml"
.\scripts\g1.ps1
```

**Pass:** all G1 steps pass. Your profiles and copied model stay in the data folder and do nothing now. Step 6 removed your key from SYSTEM's vault, so none is left there. If you repeat step 4 later, remove the key the same way before uninstalling.
