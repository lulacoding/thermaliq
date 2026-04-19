<#
.SYNOPSIS
    ThermalIQ Session 1 — Daemon smoke test (step 1.4).
    Starts mock daemon, verifies auth enforcement and all core endpoints, then stops it.
.USAGE
    cd C:\dev\thermaliq
    .\scripts\win-smoke.ps1
#>

$ErrorActionPreference = 'Stop'
$LogFile = "C:\dev\thermaliq-smoke-$(Get-Date -Format 'yyyyMMdd-HHmmss').log"

function Log {
    param([string]$msg)
    $ts = Get-Date -Format 'HH:mm:ss'
    $line = "[$ts] $msg"
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
}

function Check {
    param([string]$label, [scriptblock]$test)
    try {
        & $test
        Log "  [PASS] $label"
    } catch {
        Log "  [FAIL] $label -- $_"
        throw
    }
}

Log "Smoke test started. Log: $LogFile"
Set-Location "C:\dev\thermaliq"

# ---------------------------------------------------------------------------
# 1. Start daemon in background
# ---------------------------------------------------------------------------

Log "Starting mock daemon..."
$daemonJob = Start-Job -ScriptBlock {
    Set-Location "C:\dev\thermaliq"
    cargo run -p thermaliq-daemon --features mock-hardware 2>&1
}

# Wait for daemon to be ready (poll /api/health without auth for up to 30s)
$ready = $false
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 800
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:7272/api/health" -UseBasicParsing -ErrorAction SilentlyContinue
        # 401 means daemon is up (auth rejected), 200 would also be fine
        if ($r.StatusCode -in 200,401) { $ready = $true; break }
    } catch {
        # Connection refused — daemon still starting
    }
}

if (-not $ready) {
    $output = Receive-Job $daemonJob
    Log "Daemon output so far:`n$output"
    throw "Daemon did not become ready within 30 seconds."
}
Log "  Daemon is up."

# ---------------------------------------------------------------------------
# 2. Unauthenticated request must return 401
# ---------------------------------------------------------------------------

Check "Unauthenticated /api/health returns 401" {
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:7272/api/health" -UseBasicParsing
        throw "Expected 401 but got $($r.StatusCode)"
    } catch [System.Net.WebException] {
        $code = [int]$_.Exception.Response.StatusCode
        if ($code -ne 401) { throw "Expected 401 but got $code" }
    }
}

# ---------------------------------------------------------------------------
# 3. Read token
# ---------------------------------------------------------------------------

$tokenPath = "$env:APPDATA\thermaliq\api.token"
if (-not (Test-Path $tokenPath)) {
    throw "Token file not found at $tokenPath. Did the daemon create it on first start?"
}
$tok = (Get-Content $tokenPath).Trim()
if ($tok.Length -ne 64) {
    throw "Token looks wrong (expected 64 hex chars, got $($tok.Length)). Check $tokenPath"
}
Log "  Token: $($tok.Substring(0,8))...[redacted]"

$headers = @{ Authorization = "Bearer $tok" }

# ---------------------------------------------------------------------------
# 4. Authenticated health check
# ---------------------------------------------------------------------------

Check "GET /api/health returns 200 with status:ok" {
    $r = Invoke-RestMethod -Uri "http://127.0.0.1:7272/api/health" -Headers $headers
    if ($r.status -ne "ok") { throw "status field is '$($r.status)', expected 'ok'" }
    if (-not $r.version)    { throw "missing version field" }
    Log "    version=$($r.version)  uptime=$($r.uptime_seconds)s"
}

# ---------------------------------------------------------------------------
# 5. Core endpoints return valid JSON
# ---------------------------------------------------------------------------

$endpoints = @(
    "/api/sensors",
    "/api/fans",
    "/api/profiles"
)

foreach ($ep in $endpoints) {
    Check "GET $ep returns valid JSON" {
        $r = Invoke-RestMethod -Uri "http://127.0.0.1:7272$ep" -Headers $headers
        if ($null -eq $r) { throw "Response was null or empty" }
        Log "    $ep -> OK ($(($r | ConvertTo-Json -Compress).Length) chars)"
    }
}

# ---------------------------------------------------------------------------
# 6. Stop daemon
# ---------------------------------------------------------------------------

Log "Stopping daemon..."
Stop-Job $daemonJob
Remove-Job $daemonJob
Log "  Daemon stopped."

# ---------------------------------------------------------------------------

Log ""
Log ("=" * 50)
Log "SMOKE TEST PASSED"
Log "Log saved to: $LogFile"
Log ("=" * 50)
