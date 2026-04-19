param()

$ErrorActionPreference = 'Stop'
$LogFile = "C:\dev\thermaliq-smoke-" + (Get-Date -Format 'yyyyMMdd-HHmmss') + ".log"

function Log([string]$msg) {
    $line = "[" + (Get-Date -Format 'HH:mm:ss') + "] " + $msg
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
}

function Pass([string]$label) { Log ("  [PASS] " + $label) }
function Fail([string]$label, [string]$detail) { throw ("[FAIL] " + $label + " -- " + $detail) }

Log ("Smoke test started. Log: " + $LogFile)
Set-Location "C:\dev\thermaliq"

# ---------------------------------------------------------------------------
# 1. Daemon management — attach to running instance or start a new one
# ---------------------------------------------------------------------------

$daemonJob = $null
$weStartedDaemon = $false

# Check if port 7272 is already accepting connections (daemon up = any HTTP response)
$alreadyUp = $false
try {
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect("127.0.0.1", 7272)
    $tcp.Close()
    $alreadyUp = $true
} catch { }

if ($alreadyUp) {
    Log "Daemon already running on :7272 -- attaching (will not stop it at end)."
} else {
    $releaseBin = "C:\dev\thermaliq\target\release\thermaliq-daemon.exe"
    if (Test-Path $releaseBin) {
        Log "Starting REAL daemon (release binary, Windows HAL)..."
        $daemonJob = Start-Job -ScriptBlock {
            & "C:\dev\thermaliq\target\release\thermaliq-daemon.exe" 2>&1
        }
    } else {
        Log "Release binary not found -- using mock daemon (cargo run)..."
        $daemonJob = Start-Job -ScriptBlock {
            Set-Location "C:\dev\thermaliq"
            $env:PATH = [System.Environment]::GetEnvironmentVariable('PATH','Machine') + ";" +
                        [System.Environment]::GetEnvironmentVariable('PATH','User')
            cargo run -p thermaliq-daemon --features mock-hardware 2>&1
        }
    }
    $weStartedDaemon = $true

    Log "  Waiting for daemon to be ready (up to 60s)..."
    $ready = $false
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 1000
        try {
            $r = Invoke-WebRequest -Uri "http://127.0.0.1:7272/api/health" -UseBasicParsing -ErrorAction SilentlyContinue
            if ($r.StatusCode -in @(200, 401)) { $ready = $true; break }
        } catch { }
    }
    if (-not $ready) {
        $out = Receive-Job $daemonJob
        Log ("Daemon output:`n" + $out)
        throw "Daemon did not become ready within 60 seconds."
    }
}
Log "  Daemon is up."

# ---------------------------------------------------------------------------
# 2. Unauthenticated request must return 401
# ---------------------------------------------------------------------------

Log "Checking auth enforcement..."
try {
    $r = Invoke-WebRequest -Uri "http://127.0.0.1:7272/health" -UseBasicParsing -ErrorAction Stop
    Fail "Unauthed /health" ("Expected 401 but got " + $r.StatusCode)
} catch [System.Net.WebException] {
    $code = [int]$_.Exception.Response.StatusCode
    if ($code -ne 401) { Fail "Unauthed /health" ("Expected 401 but got " + $code) }
} catch {
    if (-not ($_.Exception.Message -match '401')) {
        Fail "Unauthed /health" $_.Exception.Message
    }
}
Pass "Unauthenticated /health returns 401"

# ---------------------------------------------------------------------------
# 3. Read token
# ---------------------------------------------------------------------------

$tokenPath = $env:APPDATA + "\thermaliq\api.token"
if (-not (Test-Path $tokenPath)) {
    throw ("Token file not found: " + $tokenPath)
}
$tok = (Get-Content $tokenPath -Raw).Trim()
if ($tok.Length -ne 64) {
    throw ("Token length is " + $tok.Length + ", expected 64 hex chars. Check " + $tokenPath)
}
Log ("  Token: " + $tok.Substring(0, 8) + "...[redacted]")

$headers = @{ Authorization = "Bearer " + $tok }

# ---------------------------------------------------------------------------
# 4. Authenticated health check
# ---------------------------------------------------------------------------

$health = Invoke-RestMethod -Uri "http://127.0.0.1:7272/health" -Headers $headers
if ($health.status -ne "ok") { Fail "GET /health" ("status=" + $health.status) }
Pass "GET /health returns 200 with status:ok"

$ver = Invoke-RestMethod -Uri "http://127.0.0.1:7272/api/version" -Headers $headers
Log ("    version=" + $ver.version)
Pass "GET /api/version returns version string"

# ---------------------------------------------------------------------------
# 5. Core endpoints return valid JSON
# ---------------------------------------------------------------------------

$endpoints = @("/sensors", "/fans", "/api/profiles")
foreach ($ep in $endpoints) {
    $r = Invoke-RestMethod -Uri ("http://127.0.0.1:7272" + $ep) -Headers $headers
    if ($null -eq $r) { Fail ("GET " + $ep) "null response" }
    $json = $r | ConvertTo-Json -Compress
    Log ("    " + $ep + " -> OK (" + $json.Length + " chars)")
    Pass ("GET " + $ep + " returns valid JSON")
}

# ---------------------------------------------------------------------------
# 6. Stop daemon only if we started it
# ---------------------------------------------------------------------------

if ($weStartedDaemon -and $daemonJob) {
    Log "Stopping daemon (we started it)..."
    Stop-Job $daemonJob
    Remove-Job $daemonJob
    Log "  Daemon stopped."
} else {
    Log "Leaving daemon running (was already up before test)."
}

$bar = "=" * 50
Log ""
Log $bar
Log "SMOKE TEST PASSED"
Log ("Log: " + $LogFile)
Log $bar
