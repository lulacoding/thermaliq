# M14 checks on the Windows PC

**Not run yet.** Do these after reviewing the M14 PR. Paste each step's output into chat and stop at the first failure.

Branch: `m14-cli`. Use an **admin** PowerShell (step 4 also uses a normal one), and close Fan Control, MSI Center and games first. The service must already be installed and mapped as at the end of M13 (`roles.toml` present, `balanced` active).

What this does to hardware: `tq` only calls the API. Steps 1 to 5 move no fan except through the engine's normal curve (the agent is paused for a few seconds in step 5). Step 6 holds one case fan at a chosen duty for 20 s through the engine (safety rules still win) and then cancels it. Step 7 runs one fan at 100% for 5 s, raise-only, as M13's probe did. Each step that touches a fan says how it gets handed back.

| Step | What it proves |
|---|---|
| 1 | Build, install `tq` and the rebuilt daemon |
| 2 | Every read command, with `--json` |
| 3 | Usage errors (exit 1) and an unreachable daemon (exit 2) |
| 4 | The token: file, env, bad env, wrong token, a normal (non-admin) shell, never on the command line |
| 5 | Writes that change nothing lasting: profile, agent pause/resume/decisions, key and workload refusals, inventory |
| 6 | `override`: set, shown by the daemon, capped, cancelled |
| 7 | `identify`: nothing saved on `n`, one probe, and the token is checked before a fan runs |
| 8 | Clean up |

## 1. Build and install

```powershell
cd C:\Users\samjo\dev\thermaliq
git fetch; git switch m14-cli; git pull
cargo build -p thermaliq-daemon -p thermaliq-cli
$pf = "$env:ProgramFiles\ThermalIQ"
$dir = "$env:ProgramData\ThermalIQ"
$tq = "$pf\tq.exe"
Stop-Service ThermalIQ
Wait-Process -Name thermaliq-daemon -Timeout 30 -ErrorAction SilentlyContinue
if (Get-Process thermaliq-daemon -ErrorAction SilentlyContinue) { throw 'thermaliq-daemon is still running: wait or end it, then run this step again' }
Copy-Item target\debug\thermaliq-daemon.exe $pf -Force
Copy-Item target\debug\tq.exe $pf -Force
Start-Service ThermalIQ
Start-Sleep 20
function T { & $tq @args; "exit=$LASTEXITCODE" }
& "$pf\thermaliq-daemon.exe" identify; "exit=$LASTEXITCODE"
T status
```

**Pass:** the daemon's `identify` now prints `thermaliq-daemon: usage: thermaliq-daemon [install-service [--force] | uninstall-service]` (the subcommand is gone), `exit=1`. `T status` prints `state: ok`, `engine: running`, `profile: balanced`, `agent: running`, and `exit=0`.

## 2. Reads

```powershell
T status
T sensors
T fans
T metrics
T profile list
T identity
T config show
$cpu = (Invoke-RestMethod http://127.0.0.1:7272/sensors | Where-Object { $_.kind -eq 'cpu' } | Select-Object -First 1).id
T history $cpu --mins 2
T history $cpu --mins 2 --json
T status --json
T --json fans
```

**Pass:** each prints a table or `key: value` lines and `exit=0`. `fans` has a row per fan with `RPM`, `DUTY%`, `CURVE%`, `AGENT%`, `SENT%` (`-` where there is none); `profile list` marks `balanced` with `*`; `identity` shows your board, the `case_fans` list and an empty "inventory" section (nothing new, missing or changed); `history` shows rows of "N s ago" within 2 minutes; the `--json` runs print JSON that matches the table values, and the flag works before or after the command.

## 3. Usage errors, and an unreachable daemon

```powershell
T help
T
T frobnicate
T status --nope
T override
T history
Stop-Service ThermalIQ
T status
T fans
Start-Service ThermalIQ
Start-Sleep 20
T status
```

**Pass:** `help` prints the command list and `exit=0`. The next five print `tq: ...` plus the usage text and `exit=1`. With the service stopped, `status` and `fans` print `tq: cannot reach the daemon at http://127.0.0.1:7272: ... Is the ThermalIQ service running?` and `exit=2`. After the start, `status` is `exit=0` again.

## 4. The token

```powershell
# Admin shell, nothing set: reads token.txt.
Remove-Item Env:THERMALIQ_TOKEN -ErrorAction SilentlyContinue
T agent pause
T agent resume

# Env token beats the file.
$env:THERMALIQ_TOKEN = (Get-Content "$dir\token.txt" -Raw).Trim()
T agent pause
T agent resume

# An env value that is not a token: exit 3, and the file is not tried instead.
$env:THERMALIQ_TOKEN = 'nonsense'
T agent pause

# A well-formed token the daemon does not know: exit 3 from the daemon's 401.
$env:THERMALIQ_TOKEN = '0' * 64
T agent pause

# Reads never need a token.
T status

# The token is never an argument.
Remove-Item Env:THERMALIQ_TOKEN
T --token abc agent pause
T agent pause --token abc

# A data directory with no token.txt: exit 3 with the fix, and nothing is sent.
$env:THERMALIQ_DATA_DIR = "$env:TEMP\tq-empty"
New-Item -ItemType Directory -Force $env:THERMALIQ_DATA_DIR | Out-Null
T agent pause
Remove-Item Env:THERMALIQ_DATA_DIR
Remove-Item "$env:TEMP\tq-empty" -ErrorAction SilentlyContinue
```

Then, in a **normal (non-admin) PowerShell** (open a second window; paste `$pf`, `$tq`, `function T` from step 1):

```powershell
$pf = "$env:ProgramFiles\ThermalIQ"; $tq = "$pf\tq.exe"
function T { & $tq @args; "exit=$LASTEXITCODE" }
T status
T agent pause
```

**Pass (admin shell):** the first four writes print `agent: paused` / `agent: running` and `exit=0`. `nonsense` prints `tq: THERMALIQ_TOKEN is not a 64-digit hex token`, `exit=3`. The all-zero token prints the daemon's `missing or wrong bearer token (token.txt may be stale ...)`, `exit=3`. `T status` is `exit=0` with the bad token set or not. `--token` is refused with `tq: unknown option --token` plus the usage, `exit=1`, and `agent pause` is not sent. The empty directory prints `tq: cannot read ...\token.txt (...): run tq elevated or as the installing user, or set THERMALIQ_TOKEN`, `exit=3`.

**Pass (normal shell):** `T status` works (`exit=0`). `T agent pause` either works (the token file is readable by your user) or prints the "run tq elevated or as the installing user" message with `exit=3`. Paste which one you got: it tells M18 whether a standard user can read the token (PORTING M18 carries this question).

Make sure the agent is running again afterwards: `T status` shows `agent: running`.

## 5. Writes that change nothing lasting

```powershell
Remove-Item Env:THERMALIQ_TOKEN -ErrorAction SilentlyContinue
T profile set balanced
T profile set nosuchprofile
T agent pause
T status
T agent resume
T agent decisions
T agent decisions --json
"x" | & $tq agent key set nosuchprovider; "exit=$LASTEXITCODE"
"" | & $tq agent key set openrouter; "exit=$LASTEXITCODE"
T agent workload nosuch.exe napping
T identity
T inventory accept
T config show
```

**Pass:** `profile set balanced` prints `active profile: balanced`, `exit=0`; the unknown profile prints `tq: no profile 'nosuchprofile'`, `exit=4`. `agent pause` then `status` shows `agent: paused` (exit 0: pausing is not unhealthy), `resume` brings `running`. `agent decisions` prints the account, local model state, a questions table and a decisions table (possibly empty); `--json` is the daemon's JSON. The unknown provider prints `tq: unknown provider`, `exit=4`, and nothing is stored; the empty key prints `tq: no key on stdin: ...`, `exit=1`, and nothing is sent. The unknown workload prints `tq: unknown workload`, `exit=4`. `inventory accept` prints `inventory saved`, `exit=0`: only run it if `identity` showed nothing new, missing or changed (it is harmless then; if something was listed, skip it and paste the `identity` output).

**Echo off on a console (the review's must-fix).** Run this in a real console window (Windows Terminal or the PowerShell window itself, not the VS Code output pane), and type the made-up text `abc-not-a-key` when asked, then Enter. The provider is invalid on purpose, so nothing is stored:

```powershell
& $tq agent key set nosuchprovider; "exit=$LASTEXITCODE"
"typed text shows now:"; Read-Host "type abc and Enter"
```

**Pass:** it prints `Paste the key and press Enter (not shown): ` and **nothing you type appears**; after Enter it prints `tq: unknown provider` and `exit=4`. The `Read-Host` line then shows what you type (echo is back on). Repeat once and press **Ctrl-C** while it waits for the key; then type something at the prompt: it must show (echo restored when the process is killed). Pipe check: `"abc" | & $tq agent key set nosuchprovider` prints no prompt, `tq: unknown provider`, `exit=4`.

Do **not** run `agent key set` with a real key here: that would store it. Do **not** run `agent key clear` if you have stored a key you want to keep.

## 6. Override

This holds one case fan at a duty for 20 s through the engine. Pick System Fan #1 (`lhm_nct6687d_0_fan_2`) and a duty well above its curve, so it is clearly audible and harmless.

```powershell
$fan = 'lhm_nct6687d_0_fan_2'
T fans
T override $fan 70 --secs 20
Start-Sleep 3
T fans
T override $fan 70 --secs 900
T override $fan --cancel
Start-Sleep 3
T fans
T override $fan 101
T override $fan -5
T override $fan abc
T override $fan 50 --secs 0
T override $fan 50 --cancel
T override nosuchfan 50
```

**Pass:** the first override prints `lhm_nct6687d_0_fan_2: 70% for 20 s`, `exit=0`. Three seconds later `fans` shows that fan with `SENT%` 70 (you can hear it go up). The 900 s override prints `... for 300 s` (the daemon's cap), still `exit=0`. `--cancel` prints `override cancelled`, and three seconds later the fan is back on its curve (`SENT%` equals `CURVE%`, or the 30% floor). The five bad inputs print an error and `exit=1` without sending anything (check the fan did not move). The unknown fan prints the daemon's own error and `exit=4`. If an override is held back (a higher priority owns the fan, or the latch), it prints `not in effect: <reason>` and `exit=4`: paste that line. If anything is still held after 5 minutes, run `T override $fan --cancel` once more; the daemon also expires it by itself.

## 7. Identify

```powershell
# A token that is not a token: refused before any request, so no fan can move.
$env:THERMALIQ_TOKEN = 'nonsense'
T identify --probe $fan
T identify
# A well-formed token the daemon does not know: its first write is refused (401).
$env:THERMALIQ_TOKEN = '0' * 64
T identify --probe $fan
Remove-Item Env:THERMALIQ_TOKEN
"n" | & $tq identify; "exit=$LASTEXITCODE"
Test-Path "$dir\roles.toml"
Get-Content "$dir\roles.toml" > "$env:TEMP\roles.before"
T identify --probe $fan
(Compare-Object (Get-Content "$env:TEMP\roles.before") (Get-Content "$dir\roles.toml")).Count
T identify --json
```

**Pass:** with `nonsense`, both commands print `tq: THERMALIQ_TOKEN is not a 64-digit hex token` and `exit=3` with **no fan moving** (the token is checked before anything runs). With the all-zero token the probe prints the daemon's `missing or wrong bearer token ...` and `exit=3`, again with no fan moving. `"n" | tq identify` prints the board and, if everything is already mapped, "Nothing changed." (`exit=0`); `roles.toml` still exists. The probe prints `Running 'System Fan #1' (lhm_nct6687d_0_fan_2) at 100% for 5 s...`, a pairing line (probably "no tach moved" on this PC), whether the write landed, and `Nothing was saved.` (about 5 s; the fan goes up and comes back), `exit=0`. The `Compare-Object` count is `0`. `identify --json` prints `tq: identify has no --json mode` plus the usage, `exit=1`.

## 8. Clean up

```powershell
Remove-Item Env:THERMALIQ_TOKEN -ErrorAction SilentlyContinue
Remove-Item Env:THERMALIQ_DATA_DIR -ErrorAction SilentlyContinue
Remove-Item "$env:TEMP\roles.before" -ErrorAction SilentlyContinue
T status
```

**Pass:** `status` is `state: ok`, `exit=0`, `agent: running`, and no override is held (`T fans` shows `SENT%` equal to `CURVE%` or the floor). The service stays installed; nothing else was changed.
