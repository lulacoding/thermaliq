# Agent hardware run (M20b wiring). NOT a CI script: run it by hand on the Windows PC with the
# NVIDIA GPU while you play a game. It never starts or stops the daemon and never writes a fan.
#
#   1. .\scripts\agent-run.ps1                (sets up a temp data dir, then waits for the daemon)
#   2. start the daemon yourself as the script prints, in a second terminal
#   3. play; Ctrl-C this script when done -> PASS / FAIL / NOT TESTED per check
#
# Once, when the agent has been adjusting the fan for 3 s (and again on key 'p' in this window),
# it POSTs /agent/pause, waits 6 s, then POSTs /agent/resume. Logs 1 row/s to a CSV.
#
#   .\scripts\agent-run.ps1 -SelfTest            check the PASS/FAIL logic on fake data, no daemon
#   .\scripts\agent-run.ps1 -CheckCsv run.csv    re-judge a finished run
param(
    [string]$DataDir = (Join-Path $env:TEMP 'tiq'),
    [int]$Port = 7272,
    [string]$Fan = 'nvml_gpu0_fan0',
    [string]$GpuSensor = 'nvml_gpu0_temp',
    [int]$ProfileMin = 40,
    [string]$Csv = '',
    [int]$WaitMinutes = 10,
    # Quiet curve: duty only climbs from $ProfileMin to 45%, so a GPU stress test runs hot enough
    # (over the agent's 72.5 C target) to make it boost. Use with OCCT/FurMark; watch the temps.
    [switch]$Quiet,
    [switch]$SelfTest,
    [string]$CheckCsv = ''
)
$ErrorActionPreference = 'Stop'
$inv = [Globalization.CultureInfo]::InvariantCulture
$MaxLower = 15      # SPEC 7.2: at most 15 points below the curve
$BoostLimit = 60    # SPEC 7.2: a boost lasts at most 60 s
$BoostSlack = 2     # 1 Hz sampling: a 60 s boost can span 61-62 s of rows
$PauseWithin = 2    # duty back on the curve within 2 s of /agent/pause
$ResumeWithin = 3
$Header = 't,gpu_temp,duty,commanded,curve,agent_duty,boost,agent,latched,event'

# ---------- judging (pure: rows in, verdicts out) ----------

function ConvertTo-Num($v) {
    if ($null -eq $v -or "$v" -eq '') { return $null }
    [double]::Parse("$v", $inv)
}

# Empty cell = "none" (never 0). Rows are what Import-Csv gives, or the fake ones in -SelfTest.
function ConvertTo-Row($r) {
    [pscustomobject]@{
        t = ConvertTo-Num $r.t; gpu = ConvertTo-Num $r.gpu_temp; duty = ConvertTo-Num $r.duty
        cmd = ConvertTo-Num $r.commanded; curve = ConvertTo-Num $r.curve; ask = ConvertTo-Num $r.agent_duty
        agent = "$($r.agent)"; latched = "$($r.latched)"; event = "$($r.event)"
    }
}

# A boost is the agent asking for well above the curve (the optimiser adds 20, capped at 100).
# The ask is computed one tick before the curve is sampled, so a curve that just moved can leave
# a lowering ask 1-4 points above it; that is not a boost.
function Test-Boost($r) {
    $null -ne $r.ask -and $null -ne $r.curve -and ($r.ask -ge $r.curve + 5 -or ($r.ask -eq 100 -and $r.ask -gt $r.curve))
}

function New-Verdict($name, $result, $detail) { [pscustomobject]@{ Name = $name; Result = $result; Detail = $detail } }

function Get-Verdicts($csvRows, [int]$min) {
    $all = @($csvRows | ForEach-Object { ConvertTo-Row $_ })
    $out = @()

    # 1. commanded duty never below max(curve - 15, profile minimum). Uses the duty the engine
    #    commanded: NVML's readback lags a slewing fan by seconds and would give false alarms.
    $n = 0; $lowered = 0; $bad = @()
    foreach ($r in $all) {
        if ($null -eq $r.curve -or $null -eq $r.cmd) { continue }
        $n++
        $floor = [Math]::Max($min, $r.curve - $MaxLower)
        if ($r.cmd -lt $floor) { $bad += $r } elseif ($r.cmd -lt $r.curve) { $lowered++ }
    }
    if ($n -lt 30) { $out += New-Verdict 'floor' 'FAIL' "only $n usable rows (need 30+): the daemon was not running long enough, or /fans lacks curve_duty/commanded_duty" }
    elseif ($bad.Count -gt 0) {
        $w = $bad | Sort-Object { $_.cmd - [Math]::Max($min, $_.curve - $MaxLower) } | Select-Object -First 1
        $out += New-Verdict 'floor' 'FAIL' ("{0} of {1} rows below max(curve-15, {2}); first at t={3}: commanded {4}, curve {5}" -f $bad.Count, $n, $min, $bad[0].t, $bad[0].cmd, $bad[0].curve)
    }
    elseif ($lowered -eq 0) { $out += New-Verdict 'floor' 'NOT TESTED' "$n rows fine, but the agent never lowered the fan, so the limit was never exercised" }
    else { $out += New-Verdict 'floor' 'PASS' "$lowered lowered rows of $n, none below max(curve-15, $min)" }

    # 2. no boost longer than 60 s (consecutive rows where the agent asks for more than the curve)
    $run = $null; $prev = $null; $max = 0; $boosts = 0
    foreach ($r in $all) {
        if (Test-Boost $r) {
            $boosts++
            if ($null -ne $run -and ($r.t - $prev.t) -le 2.5) { $run.end = $r.t } else { $run = @{ start = $r.t; end = $r.t } }
            $max = [Math]::Max($max, $run.end - $run.start + 1)
        } else { $run = $null }
        $prev = $r
    }
    if ($boosts -eq 0) { $out += New-Verdict 'boost' 'NOT TESTED' 'no boost happened (the GPU never went over its target)' }
    elseif ($max -gt $BoostLimit + $BoostSlack) { $out += New-Verdict 'boost' 'FAIL' ("longest boost {0:0.#} s > {1} s" -f $max, $BoostLimit) }
    else { $out += New-Verdict 'boost' 'PASS' ("{0} boost rows, longest {1:0.#} s" -f $boosts, $max) }

    # 3. after /agent/pause the agent shows paused, asks for nothing, and the duty is on the curve within 2 s.
    #    A pause while the agent was not changing the duty proves nothing, so it does not count.
    $tested = 0; $failed = @(); $worst = 0
    for ($i = 0; $i -lt $all.Count; $i++) {
        if ($all[$i].event -ne 'pause') { continue }
        $b = if ($i -gt 0) { $all[$i - 1] } else { $null }
        if ($null -eq $b -or $null -eq $b.ask -or $null -eq $b.curve -or $b.ask -eq $b.curve) { continue }
        $tested++
        $t0 = $all[$i].t; $settled = $null
        for ($j = $i; $j -lt $all.Count -and ($all[$j].t - $t0) -le $PauseWithin; $j++) {
            $x = $all[$j]
            if ($x.agent -eq 'paused' -and $null -eq $x.ask -and $null -ne $x.cmd -and $null -ne $x.curve -and [Math]::Abs($x.cmd - $x.curve) -le 1) { $settled = $x.t - $t0; break }
        }
        if ($null -eq $settled) { $failed += $t0 } else { $worst = [Math]::Max($worst, $settled) }
    }
    if ($tested -eq 0) { $out += New-Verdict 'pause' 'NOT TESTED' 'no pause while the agent was adjusting the fan' }
    elseif ($failed.Count -gt 0) { $out += New-Verdict 'pause' 'FAIL' ("{0} of {1} pauses did not put the duty on the curve within {2} s (at t={3})" -f $failed.Count, $tested, $PauseWithin, ($failed -join ', ')) }
    else { $out += New-Verdict 'pause' 'PASS' ("{0} pause(s), on the curve after at most {1:0.#} s" -f $tested, $worst) }

    # 4. after /agent/resume the agent reports running again
    $rt = 0; $rfail = @()
    for ($i = 0; $i -lt $all.Count; $i++) {
        if ($all[$i].event -ne 'resume') { continue }
        $rt++
        $t1 = $all[$i].t; $ok = $false
        for ($j = $i; $j -lt $all.Count -and ($all[$j].t - $t1) -le $ResumeWithin; $j++) { if ($all[$j].agent -eq 'running') { $ok = $true; break } }
        if (-not $ok) { $rfail += $t1 }
    }
    if ($rt -eq 0) { $out += New-Verdict 'resume' 'NOT TESTED' 'no resume happened' }
    elseif ($rfail.Count -gt 0) { $out += New-Verdict 'resume' 'FAIL' ("agent not running within $ResumeWithin s of resume at t=" + ($rfail -join ', ')) }
    else { $out += New-Verdict 'resume' 'PASS' "$rt resume(s), agent running again" }
    $out
}

function Get-Overall($verdicts) {
    if (@($verdicts | Where-Object Result -eq 'FAIL').Count -gt 0) { return 'FAIL' }
    if (@($verdicts | Where-Object Result -ne 'PASS').Count -gt 0) { return 'INCOMPLETE' }
    'PASS'
}

function Write-Verdicts($csvRows, [int]$min) {
    $v = @(Get-Verdicts $csvRows $min)
    Write-Host ''
    foreach ($x in $v) {
        $c = switch ($x.Result) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
        Write-Host ("{0,-11}{1,-11}{2}" -f $x.Result, $x.Name, $x.Detail) -ForegroundColor $c
    }
    $o = Get-Overall $v
    $c = switch ($o) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host ("RESULT: {0}{1}" -f $o, $(if ($o -eq 'INCOMPLETE') { ' (nothing failed, but not everything was exercised)' } else { '' })) -ForegroundColor $c
    $o
}

# ---------- self test (fake CSV data, no daemon) ----------

function New-Fake([double]$t, $curve, $cmd, $ask, [string]$agent = 'running', [string]$ev = '') {
    [pscustomobject]@{ t = $t; gpu_temp = 70; duty = $cmd; commanded = $cmd; curve = $curve; agent_duty = $ask; boost = ''; agent = $agent; latched = 'False'; event = $ev }
}

# n seconds starting at t0; ask/cmd as given (cmd defaults to ask, or curve when there is no ask)
function New-Span([double]$t0, [int]$n, $curve, $ask, $cmd = $null, [string]$agent = 'running') {
    if ($null -eq $cmd) { $cmd = if ($null -ne $ask) { $ask } else { $curve } }
    0..($n - 1) | ForEach-Object { New-Fake ($t0 + $_) $curve $cmd $ask $agent }
}

function Invoke-SelfTest {
    $fail = 0
    function Expect($name, $rows, $check, $want) {
        $v = @(Get-Verdicts $rows 40 | Where-Object Name -eq $check)[0]
        if ($v.Result -ne $want) { $script:selfFail++; Write-Host "SELFTEST FAIL  $name : $check wanted $want, got $($v.Result) ($($v.Detail))" -ForegroundColor Red }
        else { Write-Host "ok  $name : $check = $want" }
    }
    $script:selfFail = 0
    $curve = 60
    $idle = New-Span 0 40 $curve $null

    # floor
    Expect 'lowered by 15 exactly' ($idle + (New-Span 40 20 $curve 45)) 'floor' 'PASS'
    Expect 'lowered by 16' ($idle + (New-Span 40 20 $curve 45 44)) 'floor' 'FAIL'
    Expect 'below the profile minimum' ($idle + (New-Span 40 20 50 40 38)) 'floor' 'FAIL'
    Expect 'at the profile minimum, curve-15 is lower' ($idle + (New-Span 40 20 50 40 40)) 'floor' 'PASS'
    Expect 'never lowered' ($idle + (New-Span 40 20 $curve 80)) 'floor' 'NOT TESTED'
    Expect 'too few rows' (New-Span 0 10 $curve 45) 'floor' 'FAIL'
    Expect 'one bad row among many' ($idle + (New-Span 40 20 $curve 50) + (New-Span 60 1 $curve 30)) 'floor' 'FAIL'

    # boost
    Expect 'no boost' $idle 'boost' 'NOT TESTED'
    Expect 'a stale lowering ask 1 above a falling curve is not a boost' ($idle + (New-Span 40 5 $curve 61)) 'boost' 'NOT TESTED'
    Expect 'ask 4 above the curve is not a boost' ($idle + (New-Span 40 5 $curve 64)) 'boost' 'NOT TESTED'
    Expect 'ask 5 above the curve is a boost' ($idle + (New-Span 40 5 $curve 65)) 'boost' 'PASS'
    Expect 'a boost to 100 near the top counts' ($idle + (New-Span 40 5 98 100)) 'boost' 'PASS'
    Expect '30 s boost' ($idle + (New-Span 40 30 $curve 80)) 'boost' 'PASS'
    Expect '60 s boost' ($idle + (New-Span 40 60 $curve 80)) 'boost' 'PASS'
    Expect '62 s boost (slack)' ($idle + (New-Span 40 62 $curve 80)) 'boost' 'PASS'
    Expect '65 s boost' ($idle + (New-Span 40 65 $curve 80)) 'boost' 'FAIL'
    Expect 'two 40 s boosts 1 s apart' ($idle + (New-Span 40 40 $curve 80) + (New-Span 80 1 $curve $null) + (New-Span 81 40 $curve 80)) 'boost' 'PASS'
    Expect 'one boost with a 5 s hole in the log' ($idle + (New-Span 40 40 $curve 80) + (New-Span 85 40 $curve 80)) 'boost' 'PASS'

    # pause
    $adj = $idle + (New-Span 40 20 $curve 45)
    $pre = $adj | Select-Object -SkipLast 0
    $good = $adj + @(New-Fake 60 $curve $curve $null 'paused' 'pause') + (New-Span 61 10 $curve $null $null 'paused')
    Expect 'pause, on the curve at once' $good 'pause' 'PASS'
    $slow = { param($lag) $adj + @(1..$lag | ForEach-Object { New-Fake (59 + $_) $curve 45 45 'paused' $(if ($_ -eq 1) { 'pause' } else { '' }) }) + (New-Span (60 + $lag) 10 $curve $null $null 'paused') }
    Expect 'pause, on the curve after 2 s' (& $slow 2) 'pause' 'PASS'
    Expect 'pause, on the curve after 4 s' (& $slow 4) 'pause' 'FAIL'
    $stuck = $adj + @(New-Fake 60 $curve 45 45 'paused' 'pause') + (New-Span 61 10 $curve 45 45 'paused')
    Expect 'pause, agent still asking' $stuck 'pause' 'FAIL'
    $notPaused = $adj + @(New-Fake 60 $curve $curve $null 'running' 'pause') + (New-Span 61 10 $curve $null $null 'running')
    Expect 'pause, status never says paused' $notPaused 'pause' 'FAIL'
    $trivial = $idle + @(New-Fake 40 $curve $curve $null 'paused' 'pause') + (New-Span 41 10 $curve $null $null 'paused')
    Expect 'pause while the agent did nothing' $trivial 'pause' 'NOT TESTED'
    Expect 'no pause at all' $adj 'pause' 'NOT TESTED'

    # resume
    $res = { param($after) $good + @(New-Fake 71 $curve $curve $null 'paused' 'resume'); }
    # the agent reports running exactly $lag s after the resume row (t=71)
    $resumedAt = { param($lag) $good + @(New-Fake 71 $curve $curve $null 'paused' 'resume') + (1..($lag - 1) | ForEach-Object { New-Fake (71 + $_) $curve $curve $null 'paused' }) + (New-Span (71 + $lag) 5 $curve $null $null 'running') }
    Expect 'resume, running at once' ($good + @(New-Fake 71 $curve $curve $null 'running' 'resume') + (New-Span 72 5 $curve $null)) 'resume' 'PASS'
    Expect 'resume, running after 3 s' (& $resumedAt 3) 'resume' 'PASS'
    Expect 'resume, running after 5 s' (& $resumedAt 5) 'resume' 'FAIL'
    Expect 'resume never reported' ($good + @(New-Fake 71 $curve $curve $null 'paused' 'resume') + (New-Span 72 10 $curve $null $null 'paused')) 'resume' 'FAIL'
    Expect 'no resume' $good 'resume' 'NOT TESTED'

    # overall
    $full = $adj + (New-Span 60 30 $curve 80) + @(New-Fake 90 $curve 45 45 'running') + @(New-Fake 91 $curve $curve $null 'paused' 'pause') + (New-Span 92 5 $curve $null $null 'paused') + @(New-Fake 97 $curve $curve $null 'running' 'resume') + (New-Span 98 5 $curve $null)
    $o = Get-Overall (Get-Verdicts $full 40)
    if ($o -ne 'PASS') { $script:selfFail++; Write-Host "SELFTEST FAIL  overall of a good run: wanted PASS, got $o" -ForegroundColor Red } else { Write-Host 'ok  overall of a good run = PASS' }
    $o = Get-Overall (Get-Verdicts ($full + (New-Span 103 3 $curve 40 35)) 40)
    if ($o -ne 'FAIL') { $script:selfFail++; Write-Host "SELFTEST FAIL  overall with one violation: wanted FAIL, got $o" -ForegroundColor Red } else { Write-Host 'ok  overall with one violation = FAIL' }
    $o = Get-Overall (Get-Verdicts (New-Span 0 10 $curve $null) 40)
    if ($o -ne 'FAIL') { $script:selfFail++; Write-Host "SELFTEST FAIL  overall of too little data: wanted FAIL, got $o" -ForegroundColor Red } else { Write-Host 'ok  overall of too little data = FAIL' }

    # round trip through a real CSV file (empty cell = none)
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "agent-run-selftest-$PID.csv"
    $full | Export-Csv $tmp -NoTypeInformation
    $o = Get-Overall (Get-Verdicts (Import-Csv $tmp) 40)
    Remove-Item $tmp -ErrorAction SilentlyContinue
    if ($o -ne 'PASS') { $script:selfFail++; Write-Host "SELFTEST FAIL  CSV round trip: wanted PASS, got $o" -ForegroundColor Red } else { Write-Host 'ok  CSV round trip = PASS' }

    if ($script:selfFail -gt 0) { Write-Host "SELFTEST: FAIL ($($script:selfFail) wrong)" -ForegroundColor Red; return 1 }
    Write-Host 'SELFTEST: PASS' -ForegroundColor Green
    0
}

if ($SelfTest) { exit (Invoke-SelfTest) }
if ($CheckCsv) {
    $o = Write-Verdicts (Import-Csv $CheckCsv) $ProfileMin
    exit $(if ($o -eq 'PASS') { 0 } elseif ($o -eq 'FAIL') { 1 } else { 2 })
}

# ---------- the run ----------

$api = "http://127.0.0.1:$Port"
# Windows PowerShell 5.1 returns a JSON array as ONE object; hand it back element by element.
function Get-Api([string]$path) {
    $res = Invoke-RestMethod "$api$path" -TimeoutSec 2
    if ($res -is [array]) { foreach ($x in $res) { $x } } else { $res }
}
function Send-Api([string]$path, $auth) { Invoke-RestMethod -Method Post "$api$path" -Headers $auth -TimeoutSec 5 | Out-Null }
function Format-Cell($v) {
    if ($null -eq $v) { return '' }
    if ($v -is [double] -or $v -is [single]) { return ([double]$v).ToString('0.###', $inv) }
    "$v"
}

if ($DataDir -like "$env:ProgramData\ThermalIQ*") { throw "refusing to use the real service data dir ($DataDir): use a temp dir" }
if (-not $Csv) { $Csv = Join-Path $DataDir ("agent-run-{0}.csv" -f (Get-Date -Format 'yyyyMMdd-HHmmss')) }

$MaxPwm = if ($Quiet) { 45 } else { 100 }
New-Item -ItemType Directory -Force (Join-Path $DataDir 'profiles') | Out-Null
Set-Content -Path (Join-Path $DataDir 'profiles\agent_run.toml') -Encoding ascii -Value @"
name = "agent_run"
[[curves]]
type = "linear"
id = "c"
source = "$GpuSensor"
min_temp = 30.0
max_temp = 90.0
min_pwm = $ProfileMin
max_pwm = $MaxPwm
[[controls]]
fan = "$Fan"
curve = "c"
min_pwm = $ProfileMin
step_up_pct = 100
step_down_pct = 100
"@
Set-Content -Path (Join-Path $DataDir 'config.toml') -Encoding ascii -Value "api_port = $Port`npolling_interval_ms = 1000`ndefault_profile = `"agent_run`"`n"
Write-Host "Data dir ready: $DataDir  (profile agent_run: linear on $GpuSensor -> $Fan, $ProfileMin-$MaxPwm%$(if ($Quiet) { ', QUIET' }))"
if ($Quiet) { Write-Host 'Quiet curve: the fan stays near 45% until the agent boosts it. Watch the GPU temp; stop the stress test if it passes 85 C (at 90 C the safety latch forces 100%).' -ForegroundColor Yellow }
Write-Host ''
Write-Host 'Start the daemon yourself in another terminal (stop the ThermalIQ service first):'
Write-Host "  `$env:THERMALIQ_DATA_DIR='$DataDir'; cargo run -p thermaliq-daemon" -ForegroundColor Cyan
Write-Host ''

# wait for it: right profile, engine running, agent running, the fan present with the new fields
$deadline = (Get-Date).AddMinutes($WaitMinutes)
$last = 0
while ($true) {
    try {
        $s = Get-Api '/status'
        $f = @(Get-Api '/fans') | Where-Object { $_.id -eq $Fan } | Select-Object -First 1
        if ($s.engine_running -and $s.active_profile -ne 'agent_run') { throw "a daemon is running with profile '$($s.active_profile)', not agent_run: wrong data dir? stop it and start it as above" }
        if ($s.engine_running -and $s.agent -eq 'running' -and $null -ne $f) {
            if (-not ($f.PSObject.Properties.Name -contains 'commanded_duty')) { throw 'this daemon is too old (/fans has no commanded_duty): rebuild it' }
            if ($null -ne $f.commanded_duty) { break }
        }
    } catch {
        if ($_.Exception.Message -like 'a daemon is*' -or $_.Exception.Message -like 'this daemon*') { throw }
    }
    if ((Get-Date) -gt $deadline) { throw "no daemon with the agent running and fan $Fan after $WaitMinutes min" }
    if (((Get-Date) - [DateTime]::MinValue).TotalSeconds - $last -ge 10) { Write-Host 'waiting for the daemon...'; $last = ((Get-Date) - [DateTime]::MinValue).TotalSeconds }
    Start-Sleep -Seconds 1
}
$tokenFile = Join-Path $DataDir 'token.txt'
if (-not (Test-Path $tokenFile)) { throw "no ${tokenFile}: is the daemon using this data dir?" }
$auth = @{ Authorization = "Bearer $((Get-Content $tokenFile -Raw).Trim())" }

Set-Content -Path $Csv -Value $Header -Encoding ascii
Write-Host "Daemon is up. Logging 1 row/s to $Csv"
Write-Host 'Play now. Ctrl-C when done. Press p in this window to run a pause/resume test by hand.'

$sw = [Diagnostics.Stopwatch]::StartNew()
$paused = $false; $pausedAt = 0.0; $pauseTests = 0; $deviating = 0; $event = ''; $misses = 0; $next = 0.0
try {
    while ($true) {
        $t = $sw.Elapsed.TotalSeconds
        try {
            $st = Get-Api '/status'
            $se = @(Get-Api '/sensors') | Where-Object { $_.id -eq $GpuSensor } | Select-Object -First 1
            $fa = @(Get-Api '/fans') | Where-Object { $_.id -eq $Fan } | Select-Object -First 1
            $misses = 0
            $ask = $fa.agent_duty; $curve = $fa.curve_duty
            $boost = if (Test-Boost ([pscustomobject]@{ ask = $ask; curve = $curve })) { 1 } else { 0 }
            $cells = @([Math]::Round($t, 3), $se.temp_celsius, $fa.duty_percent, $fa.commanded_duty, $curve, $ask, $boost, $st.agent, $st.latched, $event)
            Add-Content -Path $Csv -Value (($cells | ForEach-Object { Format-Cell $_ }) -join ',') -Encoding ascii
            $event = ''
            if ($null -ne $ask -and $null -ne $curve -and $ask -ne $curve) { $deviating++ } else { $deviating = 0 }

            $manual = $false
            try { if ([Console]::KeyAvailable -and [Console]::ReadKey($true).KeyChar -eq 'p') { $manual = $true } } catch { }
            if (-not $paused -and ($manual -or ($pauseTests -eq 0 -and $deviating -ge 3 -and $t -ge 10))) {
                Send-Api '/agent/pause' $auth
                $paused = $true; $pausedAt = $sw.Elapsed.TotalSeconds; $pauseTests++; $event = 'pause'
                Write-Host ("[{0:0}s] paused the agent for a test" -f $t) -ForegroundColor Cyan
            } elseif ($paused -and ($sw.Elapsed.TotalSeconds - $pausedAt) -ge 6) {
                Send-Api '/agent/resume' $auth
                $paused = $false; $event = 'resume'
                Write-Host ("[{0:0}s] resumed the agent" -f $t) -ForegroundColor Cyan
            }
        } catch {
            $misses++
            if ($misses -in 1, 5, 30) { Write-Host ("[{0:0}s] API call failed ({1} in a row): {2}" -f $t, $misses, $_.Exception.Message) -ForegroundColor Yellow }
        }
        $next += 1.0
        $wait = $next - $sw.Elapsed.TotalSeconds
        if ($wait -gt 0) { Start-Sleep -Milliseconds ([int]($wait * 1000)) } else { $next = $sw.Elapsed.TotalSeconds }
    }
}
finally {
    if ($paused) {
        try { Send-Api '/agent/resume' $auth; Write-Host 'resumed the agent before exiting' } catch { Write-Host "COULD NOT RESUME THE AGENT: $($_.Exception.Message)" -ForegroundColor Red }
    }
    Write-Host ''
    Write-Host "Log: $Csv"
    $rows = @(Import-Csv $Csv)
    $o = Write-Verdicts $rows $ProfileMin
    Write-Host 'Stop the daemon with Ctrl-C in its terminal: the fans go back to automatic.'
    exit $(if ($o -eq 'PASS') { 0 } elseif ($o -eq 'FAIL') { 1 } else { 2 })
}
