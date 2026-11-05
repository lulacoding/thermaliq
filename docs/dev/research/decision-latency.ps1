# M20c spike (SPEC 7.2, 7.5): latency of a local decision model on one CPU core at background
# priority. Research only, nothing in the product uses it. Steps: decision-models.md.
#
#   -Count 50                    one server, a warm-up, then 50 timed decisions
#   -EverySeconds 60 -Count 3    SPEC cadence for the FPS test: load, one decision, unload, wait
#   -Hosted                      the same 50 decisions to Jev 1.14 on OpenRouter, nothing pinned
#   -SelfTest                    checks this script against a fake server, no model and no key
#
# The script starts the server itself, because the server is what has to be pinned, and refuses
# to run if something already listens on the port (that server would not be pinned).
# It also refuses to run elevated. -Hosted reads the key from $env:OPENROUTER_API_KEY only and
# never writes it anywhere; local servers don't inherit it.
param(
    [string]$Exe,
    [string]$ExeArgs = '',
    [string]$Model = '',
    [int]$Port = 11435,
    [int]$Count = 50,
    [int]$EverySeconds = 0,
    [int]$Core = [Environment]::ProcessorCount - 1,
    [int]$TimeoutSec = 300,
    [switch]$Hosted,
    [switch]$SelfTest,
    # Self-test only: a stand-in for the model server, or for OpenRouter.
    [switch]$FakeServer,
    [switch]$FakeHosted,
    [switch]$BadAnswer,
    [switch]$Escape,
    [switch]$HostedToFake
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# .NET's default waits for a reply before sending the body, which curl doesn't do: it would inflate latency.
[Net.ServicePointManager]::Expect100Continue = $false

$HostedUrl = 'https://openrouter.ai/api/v1/systemone'
$HostedModel = 'typesafe/jev-1.14'
$script:outside = 0
$script:cost = 0.0
$script:servedBy = ''

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

function Hide-Key([string]$text) {
    if ($env:OPENROUTER_API_KEY) { $text.Replace($env:OPENROUTER_API_KEY, '[key]') } else { $text }
}

# One request holds both questions: the slow layer asks them together for a new exe.
function Invoke-Decision([int]$i) {
    $bytes = [Text.Encoding]::UTF8.GetBytes((New-Body $i))
    $request = @{ Uri = "http://127.0.0.1:$Port/v1/systemone"; Headers = @{} }
    if ($Hosted) {
        if (-not $HostedToFake) { $request.Uri = $HostedUrl }
        $request.Headers = @{ Authorization = "Bearer $env:OPENROUTER_API_KEY" }
    }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    # No redirects: the key must only ever reach the URL above.
    $r = Invoke-RestMethod @request -Method Post -Body $bytes -ContentType 'application/json' `
        -TimeoutSec $TimeoutSec -DisableKeepAlive -MaximumRedirection 0
    $ms = [Math]::Round($sw.Elapsed.TotalMilliseconds)
    $w = $r.answers.workload; $p = $r.answers.profile
    if (-not $w -or -not $p -or -not $Workloads.Contains([string]$w.choice) -or -not $Profiles.Contains([string]$p.choice)) {
        $bad = "answer outside the schema: $(Hide-Key ($r | ConvertTo-Json -Compress -Depth 6))"
        if (-not $Hosted) { throw $bad }
        $script:outside++
        Write-Host ("{0,3} {1,-18} {2,8} ms  {3}" -f $i, $Exes[$i % $Exes.Count], $ms, $bad)
        return $ms
    }
    if ($r.model) { $script:servedBy = [string]$r.model }
    if ($r.usage.cost) { $script:cost += [double]$r.usage.cost }
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

# Exactly the decisions a local run times, no warm-up. Every call opens a new TLS connection,
# as a call every 60 s would. Over 2 s is when SPEC 7.2 falls back to the local model.
function Invoke-HostedBenchmark {
    if ($EverySeconds -gt 0 -or $Exe) { throw '-Hosted takes no -Exe or -EverySeconds' }
    if ($Count -lt 1) { throw 'Count must be at least 1' }
    # A new powershell.exe runs the profile first, and a profile can replace the window's key.
    $launch = [Environment]::GetCommandLineArgs()
    if (($launch -match '^-f(ile)?$') -and -not ($launch -match '^-nop')) {
        Write-Host 'FAIL: started without -NoProfile, so your PowerShell profile may have replaced the key from this window. Add -NoProfile.'
        return 1
    }
    if (-not $env:OPENROUTER_API_KEY) { Write-Host 'FAIL: $env:OPENROUTER_API_KEY is not set'; return 1 }
    if (-not $Model) { $script:Model = $HostedModel }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $times = @(); $code = 0
    try {
        for ($i = 1; $i -le $Count; $i++) { $times += Invoke-Decision $i }
    } catch {
        $resp = $_.Exception.Response
        $status = if ($resp) { "HTTP $([int]$resp.StatusCode)" } else { 'no HTTP status' }
        Write-Host "FAIL after $($times.Count)/$Count decisions: $status`: $(Hide-Key "$($_.Exception.Message)")"
        # PowerShell 5.1 leaves ErrorDetails empty for some responses, OpenRouter's 401 included.
        $body = $_.ErrorDetails.Message
        if (-not $body -and $resp) {
            $stream = $resp.GetResponseStream()
            if ($stream.CanSeek) { $stream.Position = 0 }
            $body = [IO.StreamReader]::new($stream).ReadToEnd()
        }
        if ($body) { Write-Host "response body: $(Hide-Key $body)" }
        $code = 1
    }
    if ($times.Count) {
        $slow = @($times | Where-Object { $_ -gt 2000 }).Count
        Write-Host ("{0}/{1} answered, {2}: median {3} ms, p95 {4} ms; over 2 s: {5}; outside the schema: {6}; served by {7}; cost `${8:N6}" -f `
            $times.Count, $Count, $Model, (Get-Percentile $times 50), (Get-Percentile $times 95), $slow, $script:outside, $script:servedBy, $script:cost)
    }
    if ($script:outside) { $code = 1 }
    $code
}

function Invoke-FakeServer {
    $listener = [Net.HttpListener]::new()
    $listener.Prefixes.Add("http://127.0.0.1:$Port/")
    $listener.Start()
    $served = 0
    while ($true) {
        $ctx = $listener.GetContext()
        $served++
        $auth = $ctx.Request.Headers['Authorization']
        $bad = if ($FakeHosted) { $BadAnswer -and $served % 2 -eq 0 } else { $BadAnswer }
        $choice = if ($bad) { 'banana' } else { 'gaming' }
        # A bad hosted answer echoes the header, to prove that output hides the key.
        $echo = if ($bad -and $FakeHosted) { $auth } else { '' }
        $json = "{`"model`":`"fake`",`"echo`":`"$echo`",`"answers`":{`"workload`":{`"type`":`"choice`",`"choice`":`"$choice`",`"confidence`":0.9},`"profile`":{`"type`":`"choice`",`"choice`":`"gaming`",`"confidence`":0.8}},`"usage`":{`"input_tokens`":1,`"output_tokens`":0,`"cost`":0.00001}}"
        if ($FakeHosted) {
            if ($auth -ne "Bearer $env:OPENROUTER_API_KEY") {
                $ctx.Response.StatusCode = 401
                $json = "{`"error`":{`"message`":`"User not found for $auth`",`"code`":401}}"
            }
        } elseif ($env:OPENROUTER_API_KEY) { $ctx.Response.StatusCode = 500 } # a local server must not inherit the key
        elseif ($served -eq 1) { $ctx.Response.StatusCode = 503 } # as llama-server does while loading
        elseif ($Escape) { (Get-Process -Id $PID).ProcessorAffinity = [IntPtr]1 }
        $bytes = [Text.Encoding]::UTF8.GetBytes($json)
        $ctx.Response.ContentType = 'application/json'
        $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
        $ctx.Response.Close()
    }
}

function Invoke-SelfTest {
    $ErrorActionPreference = 'Continue' # a child's stderr is test output here, not a failure
    # A dummy key for every child; the real one is never used and comes back at the end.
    $testKey = 'selftest-not-a-real-key'
    $realKey = $env:OPENROUTER_API_KEY
    $env:OPENROUTER_API_KEY = $testKey
    try { Invoke-SelfTestChecks } finally { [Environment]::SetEnvironmentVariable('OPENROUTER_API_KEY', $realKey) }
}

function Invoke-SelfTestChecks {
    $script:failed = 0
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

    function RunHosted($flags, $keyForClient) {
        $server = Start-Process powershell.exe "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -FakeServer -FakeHosted -Port $fp $flags" -PassThru -NoNewWindow
        try {
            $deadline = (Get-Date).AddSeconds(30)
            while (-not (Test-Listening) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
            [Environment]::SetEnvironmentVariable('OPENROUTER_API_KEY', $keyForClient)
            $out = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Hosted -HostedToFake -Port $fp -Count 5 -TimeoutSec 30 *>&1 | Out-String
            [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out }
        } finally {
            [Environment]::SetEnvironmentVariable('OPENROUTER_API_KEY', $testKey)
            Stop-Server $server
        }
    }
    $r = RunHosted '' $testKey
    Check 'hosted: 5/5, sends the key and the model' ($r.Code -eq 0 -and $r.Out -match '5/5 answered, typesafe/jev-1\.14' -and $r.Out -match 'outside the schema: 0')
    Check 'hosted: the key is not in the output' ($r.Out -notmatch $testKey)
    $r = RunHosted '-BadAnswer' $testKey
    Check 'hosted: counts answers outside the schema and keeps going' ($r.Code -eq 1 -and $r.Out -match '5/5 answered' -and $r.Out -match 'outside the schema: 2')
    Check 'hosted: an echoed key is hidden' ($r.Out -match '\[key\]' -and $r.Out -notmatch $testKey)
    $r = RunHosted '' 'selftest-wrong-key'
    Check 'hosted: a rejected key fails without printing it' ($r.Code -eq 1 -and $r.Out -match 'HTTP 401' -and $r.Out -notmatch 'selftest-wrong-key')
    Check 'hosted: prints the error body, key hidden' ($r.Out -match 'response body: .*User not found for Bearer \[key\]')
    $r = RunHosted '' $null
    Check 'hosted: no key, no request' ($r.Code -eq 1 -and $r.Out -match 'is not set')
    $out = & powershell.exe -ExecutionPolicy Bypass -File $PSCommandPath -Hosted -HostedToFake -Port $fp -Count 1 -TimeoutSec 5 *>&1 | Out-String
    Check 'hosted: refuses a start that reloads the profile' ($LASTEXITCODE -eq 1 -and $out -match 'without -NoProfile')
    if ($script:failed) { Write-Host "$script:failed self-test(s) FAILED"; return 1 }
    Write-Host 'self-test passed'
    0
}

if ($FakeServer) { Invoke-FakeServer; exit 0 }
$me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if ($me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'FAIL: this window is elevated. Run this script from a normal window (only PresentMon needs admin).'
    exit 1
}
if ($SelfTest) { exit (Invoke-SelfTest) }
if ($Hosted) { exit (Invoke-HostedBenchmark) }
if (-not $Exe) { throw '-Exe or -Hosted is required (see decision-models.md)' }

# Inherited by the server: one thread, CPU only, no key. Restored because the script may run in your shell.
$threadEnv = @{ OMP_NUM_THREADS = '1'; MKL_NUM_THREADS = '1'; OLLAYA_THREADS = '1'; OLLAYA_DEVICE = 'cpu'; CUDA_VISIBLE_DEVICES = '-1'; OPENROUTER_API_KEY = $null }
$saved = @{}
foreach ($k in $threadEnv.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $threadEnv[$k]) }
try { $code = Invoke-Benchmark } finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
exit $code
