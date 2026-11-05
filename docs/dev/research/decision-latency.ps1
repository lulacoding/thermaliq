# M20c spike (SPEC 7.2, 7.5): latency of a local decision model on one CPU core at background
# priority. Research only, nothing in the product uses it. Steps: decision-models.md.
#
#   -Count 50                    one server, a warm-up, then 50 timed decisions
#   -EverySeconds 60 -Count 3    SPEC cadence for the FPS test: load, one decision, unload, wait
#   -SelfTest                    checks this script against a fake server, no model needed
#
# The script starts the server itself, because the server is what has to be pinned, and refuses
# to run if something already listens on the port (that server would not be pinned).
param(
    [string]$Exe,
    [string]$ExeArgs = '',
    [string]$Model = '',
    [int]$Port = 11435,
    [int]$Count = 50,
    [int]$EverySeconds = 0,
    [int]$Core = [Environment]::ProcessorCount - 1,
    [int]$TimeoutSec = 300,
    [switch]$SelfTest,
    # Self-test only: a stand-in for the model server.
    [switch]$FakeServer,
    [switch]$BadAnswer,
    [switch]$Escape
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Workloads = [ordered]@{
    idle = 'Nothing demanding is running'; browsing = 'Web browser or light desktop use'
    gaming = 'A game is running'; compiling = 'Building software'
    rendering = 'Rendering, encoding or other long GPU or CPU compute'; other = 'None of these'
}
$Profiles = [ordered]@{
    silent = 'Lowest noise'; efficiency = 'Low noise, low power'; balanced = 'Everyday default'
    gaming = 'Cooling for sustained game load'; performance = 'Maximum cooling'
}
$Exes = 'cyberpunk2077.exe', 'eldenring.exe', 'cs2.exe', 'dota2.exe', 'chrome.exe', 'msedge.exe',
    'firefox.exe', 'code.exe', 'cargo.exe', 'rustc.exe', 'msbuild.exe', 'blender.exe',
    'obs64.exe', 'handbrake.exe', 'premiere.exe', 'discord.exe', 'spotify.exe', 'explorer.exe',
    'excel.exe', 'steam.exe', 'furmark.exe', 'python.exe', 'unknown_tool.exe', 'setup.exe', 'vlc.exe'

# Nearest rank: always a value that was measured. p95 of 50 is the 48th.
function Get-Percentile($values, [int]$p) {
    $sorted = @($values | Sort-Object)
    if ($sorted.Count -eq 0) { throw 'no values' }
    $sorted[[Math]::Max([Math]::Ceiling($p / 100 * $sorted.Count) - 1, 0)]
}

# Numbers and exe names only, the same rule as hosted requests (SPEC 7.2).
function New-Body([int]$i) {
    $rng = [Random]::new($i)
    $state = [ordered]@{
        exe = $Exes[$i % $Exes.Count]; cpu_load_pct = $rng.Next(1, 100); gpu_load_pct = $rng.Next(0, 100)
        cpu_temp_c = $rng.Next(40, 90); gpu_temp_c = $rng.Next(35, 85); gpu_power_w = $rng.Next(10, 115)
    } | ConvertTo-Json -Compress
    $body = [ordered]@{
        state = $state
        questions = [ordered]@{
            workload = [ordered]@{ type = 'choice'; instructions = 'Which workload is this exe, given the readings?'; criteria = $Workloads }
            profile = [ordered]@{ type = 'choice'; instructions = 'Which fan profile suits it?'; criteria = $Profiles }
        }
    }
    if ($Model) { $body.model = $Model }
    $body | ConvertTo-Json -Compress -Depth 6
}

# One request holds both questions: the slow layer asks them together for a new exe.
function Invoke-Decision([int]$i) {
    $bytes = [Text.Encoding]::UTF8.GetBytes((New-Body $i))
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/v1/systemone" -Method Post -Body $bytes `
        -ContentType 'application/json' -TimeoutSec $TimeoutSec -DisableKeepAlive
    $ms = [Math]::Round($sw.Elapsed.TotalMilliseconds)
    $w = $r.answers.workload; $p = $r.answers.profile
    if (-not $w -or -not $p -or -not $Workloads.Contains([string]$w.choice) -or -not $Profiles.Contains([string]$p.choice)) {
        throw "answer outside the schema: $($r | ConvertTo-Json -Compress -Depth 6)"
    }
    Write-Host ("{0,3} {1,-18} {2,8} ms  {3} ({4:N2}), {5} ({6:N2})" -f $i, $Exes[$i % $Exes.Count], $ms, $w.choice, $w.confidence, $p.choice, $p.confidence)
    $ms
}

function Test-Listening { [bool]([Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners() | Where-Object Port -eq $Port) }

# The pip launcher and Ollaya both run the model in a child process.
function Get-Tree([int]$rootPid) {
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId)
    $ids = @($rootPid); $n = 0
    while ($n -lt $ids.Count) {
        $ids += @($all | Where-Object { $_.ParentProcessId -eq $ids[$n] -and $ids -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId })
        $n++
    }
    @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
}

function Set-Pinned([int]$rootPid) {
    foreach ($p in (Get-Tree $rootPid)) {
        try { $p.ProcessorAffinity = [IntPtr]([int64]1 -shl $Core); $p.PriorityClass = 'Idle' } catch { }
    }
}

function Test-Pinned([int]$rootPid) {
    $tree = @(Get-Tree $rootPid)
    if ($tree.Count -eq 0) { return $false }
    foreach ($p in $tree) {
        if ([int64]$p.ProcessorAffinity -ne ([int64]1 -shl $Core) -or $p.PriorityClass -ne 'Idle') {
            Write-Host "NOT PINNED: $($p.ProcessName) pid $($p.Id) affinity $($p.ProcessorAffinity) priority $($p.PriorityClass)"
            return $false
        }
    }
    $true
}

function Start-Server {
    if (Test-Listening) { throw "something already listens on port $Port and would not be pinned: stop it first" }
    $log = Join-Path $env:TEMP "decision-latency-$Port"
    $start = @{ FilePath = $Exe; PassThru = $true; NoNewWindow = $true; RedirectStandardOutput = "$log.out.txt"; RedirectStandardError = "$log.err.txt" }
    if ($ExeArgs) { $start.ArgumentList = $ExeArgs }
    $p = Start-Process @start
    $null = $p.Handle # keeps ExitCode readable after exit
    Set-Pinned $p.Id
    $p
}

function Stop-Server($p) {
    if (-not $p) { return }
    # Through cmd: PowerShell 5.1 turns taskkill's stderr (process already gone) into a throw.
    & cmd.exe /c "taskkill /T /F /PID $($p.Id) >nul 2>&1"
    $deadline = (Get-Date).AddSeconds(5)
    while ((Test-Listening) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
}

# Time from process start to the first valid answer: what an on-demand load really costs.
# Retries only while the server is not up yet (refused, or 503 while llama-server loads).
function Wait-FirstAnswer($p, [int]$i) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        if ($p.HasExited) { throw "server exited with code $($p.ExitCode), see $env:TEMP\decision-latency-$Port.err.txt" }
        if ($sw.Elapsed.TotalSeconds -gt $TimeoutSec) { throw "no answer within $TimeoutSec s" }
        Set-Pinned $p.Id
        try {
            $null = Invoke-Decision $i
            return [Math]::Round($sw.Elapsed.TotalMilliseconds)
        } catch [Net.WebException] {
            $e = $_.Exception
            $notUp = $e.Status -eq 'ConnectFailure' -or ($e.Response -and [int]$e.Response.StatusCode -eq 503)
            if (-not $notUp) { throw }
            Start-Sleep -Milliseconds 500
        }
    }
}

function Get-PeakMb([int]$rootPid) {
    $sum = 0; foreach ($p in (Get-Tree $rootPid)) { $sum += $p.PeakWorkingSet64 }
    [Math]::Round($sum / 1MB)
}

function Invoke-Benchmark {
    if ($Count -lt 1) { throw 'Count must be at least 1' }
    $times = @(); $pinned = $true; $server = $null; $load = $null; $peak = 0
    try {
        if ($EverySeconds -gt 0) {
            for ($i = 0; $i -lt $Count; $i++) {
                $cycle = [Diagnostics.Stopwatch]::StartNew()
                $server = Start-Server
                $times += Wait-FirstAnswer $server $i
                $pinned = (Test-Pinned $server.Id) -and $pinned
                $peak = [Math]::Max($peak, (Get-PeakMb $server.Id))
                Stop-Server $server; $server = $null
                $left = $EverySeconds - $cycle.Elapsed.TotalSeconds
                if ($left -lt 0) { Write-Host 'WARNING: a cycle took longer than the interval' }
                if ($i -lt $Count - 1 -and $left -gt 0) { Start-Sleep -Milliseconds ($left * 1000) }
            }
            $what = 'load + decision'
        } else {
            $server = Start-Server
            $load = Wait-FirstAnswer $server 0
            for ($i = 1; $i -le $Count; $i++) { $times += Invoke-Decision $i }
            $pinned = Test-Pinned $server.Id
            $peak = Get-PeakMb $server.Id
            $what = 'decision'
        }
    } catch {
        Write-Host "FAIL after $($times.Count)/$Count decisions: $_"
        return 1
    } finally {
        Stop-Server $server
    }
    if ($null -ne $load) { Write-Host "first answer after start: $load ms" }
    Write-Host ("{0}/{1} ok ({2}): median {3} ms, p95 {4} ms; core {5}, idle priority: {6}; peak memory {7} MB" -f `
        $times.Count, $Count, $what, (Get-Percentile $times 50), (Get-Percentile $times 95), $Core, $(if ($pinned) { 'pinned=yes' } else { 'pinned=NO' }), $peak)
    if (-not $pinned) { return 1 }
    0
}

function Invoke-FakeServer {
    $listener = [Net.HttpListener]::new()
    $listener.Prefixes.Add("http://127.0.0.1:$Port/")
    $listener.Start()
    $served = 0
    while ($true) {
        $ctx = $listener.GetContext()
        $served++
        $choice = if ($BadAnswer) { 'banana' } else { 'gaming' }
        $json = "{`"model`":`"fake`",`"answers`":{`"workload`":{`"type`":`"choice`",`"choice`":`"$choice`",`"confidence`":0.9},`"profile`":{`"type`":`"choice`",`"choice`":`"gaming`",`"confidence`":0.8}},`"usage`":{`"input_tokens`":1,`"output_tokens`":0}}"
        if ($served -eq 1) { $ctx.Response.StatusCode = 503 } # as llama-server does while loading
        elseif ($Escape) { (Get-Process -Id $PID).ProcessorAffinity = [IntPtr]1 }
        $bytes = [Text.Encoding]::UTF8.GetBytes($json)
        $ctx.Response.ContentType = 'application/json'
        $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
        $ctx.Response.Close()
    }
}

function Invoke-SelfTest {
    $ErrorActionPreference = 'Continue' # a child's stderr is test output here, not a failure
    $failed = 0
    function Check($name, $ok) { if ($ok) { Write-Host "PASS $name" } else { Write-Host "FAIL $name"; $script:failed++ } }
    $one = 1..50
    Check 'p50 of 1..50 is 25' ((Get-Percentile $one 50) -eq 25)
    Check 'p95 of 1..50 is 48' ((Get-Percentile $one 95) -eq 48)
    Check 'p95 of one value' ((Get-Percentile @(7) 95) -eq 7)
    Check 'sorts as numbers' ((Get-Percentile @(1000, 9, 100) 50) -eq 100)
    $b = New-Body 3 | ConvertFrom-Json
    Check 'body has both choice questions' ($b.questions.workload.type -eq 'choice' -and $b.questions.profile.criteria.gaming)
    Check 'state holds numbers and the exe name only' ((($b.state | ConvertFrom-Json).PSObject.Properties | Where-Object { $_.Name -ne 'exe' -and $_.Value -isnot [int] }).Count -eq 0)

    $free = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0); $free.Start()
    $fp = $free.LocalEndpoint.Port; $free.Stop()
    $Port = $fp
    function Run($serverFlags, $extra) {
        $fake = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -FakeServer -Port $fp $serverFlags"
        $out = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Exe powershell.exe -ExeArgs $fake -Port $fp @extra *>&1 | Out-String
        [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out }
    }
    $r = Run '' @('-Count', '5', '-TimeoutSec', '60')
    Check 'batch: 5/5, pinned, survives a 503 while loading' ($r.Code -eq 0 -and $r.Out -match '5/5 ok' -and $r.Out -match 'pinned=yes')
    Check 'server is gone afterwards' (-not (Test-Listening))
    $r = Run '' @('-Count', '2', '-EverySeconds', '1', '-TimeoutSec', '60')
    Check 'interval: reloads per decision, still pinned' ($r.Code -eq 0 -and $r.Out -match '2/2 ok \(load' -and $r.Out -match 'pinned=yes')
    $r = Run '-BadAnswer' @('-Count', '5', '-TimeoutSec', '60')
    Check 'answer outside the schema fails' ($r.Code -eq 1 -and $r.Out -match 'outside the schema')
    $r = Run '-Escape' @('-Count', '2', '-TimeoutSec', '60')
    Check 'server that leaves its core is reported' ($r.Code -eq 1 -and $r.Out -match 'pinned=NO')
    $out = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Exe powershell.exe -ExeArgs '-NoProfile -Command exit 3' -Port $fp -Count 1 -TimeoutSec 60 *>&1 | Out-String
    Check 'server that dies is reported' ($LASTEXITCODE -eq 1 -and $out -match 'exited with code')
    $busy = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $fp); $busy.Start()
    try { $r = Run '' @('-Count', '1') } finally { $busy.Stop() }
    Check 'refuses an already running server' ($r.Code -eq 1 -and $r.Out -match 'already listens')
    if ($failed) { Write-Host "$failed self-test(s) FAILED"; return 1 }
    Write-Host 'self-test passed'
    0
}

if ($FakeServer) { Invoke-FakeServer; exit 0 }
if ($SelfTest) { exit (Invoke-SelfTest) }
if (-not $Exe) { throw '-Exe is required (see decision-models.md)' }

# Inherited by the server: one thread, CPU only. Restored because the script may run in your shell.
$threadEnv = @{ OMP_NUM_THREADS = '1'; MKL_NUM_THREADS = '1'; OLLAYA_THREADS = '1'; OLLAYA_DEVICE = 'cpu'; CUDA_VISIBLE_DEVICES = '-1' }
$saved = @{}
foreach ($k in $threadEnv.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $threadEnv[$k]) }
try { $code = Invoke-Benchmark } finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
exit $code
