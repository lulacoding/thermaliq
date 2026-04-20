param(
    [Parameter(Mandatory)][string]$FanId,
    [int]$TargetDuty = 50,
    [int]$HoldSeconds = 5
)

# fan-test-safe.ps1 -auto-revert fan write wrapper
# Usage: .\scripts\fan-test-safe.ps1 -FanId "nvml_gpu0_fan0" -TargetDuty 50 -HoldSeconds 5

$ErrorActionPreference = 'Stop'

$tok = (Get-Content "$env:APPDATA\thermaliq\api.token" -Raw).Trim()
$hdr = @{ Authorization = "Bearer $tok" }
$base = "http://127.0.0.1:7272"

Write-Host ""
Write-Host "=== Fan Test (auto-revert) ===" -ForegroundColor Cyan
Write-Host ("  Fan:    " + $FanId)
Write-Host ("  Target: " + $TargetDuty + "%")
Write-Host ("  Hold:   " + $HoldSeconds + "s")
Write-Host ""

# 1. Snapshot current state
$fans = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
$fan = $fans | Where-Object { $_.id -eq $FanId }
if (-not $fan) {
    Write-Host "[ERROR] Fan '$FanId' not found. Available:" -ForegroundColor Red
    $fans | ForEach-Object { Write-Host ("  " + $_.id) }
    exit 1
}

$originalDuty = $fan.duty_percent
$originalRpm = $fan.rpm
Write-Host ("  Before: " + $originalDuty + "% duty, " + $originalRpm + " RPM") -ForegroundColor Green

# 2. Write new duty
Write-Host ("  Writing " + $TargetDuty + "%...") -ForegroundColor Yellow
$body = @{ duty_percent = $TargetDuty } | ConvertTo-Json
try {
    Invoke-RestMethod -Uri "$base/fans/$FanId/pwm" -Method POST -Headers $hdr -ContentType "application/json" -Body $body | Out-Null
    Write-Host "  [OK] Write accepted" -ForegroundColor Green
} catch {
    Write-Host ("[ERROR] Write failed: " + $_.Exception.Message) -ForegroundColor Red
    exit 1
}

# 3. Hold and observe
Write-Host ("  Holding for " + $HoldSeconds + "s -watch HWiNFO64 RPM...") -ForegroundColor Yellow
for ($i = 1; $i -le $HoldSeconds; $i++) {
    Start-Sleep -Seconds 1
    $snap = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
    $f = $snap | Where-Object { $_.id -eq $FanId }
    if ($f) {
        Write-Host ("    t+" + $i + "s: " + $f.duty_percent + "% duty, " + $f.rpm + " RPM")
    }
}

# 4. Restore original duty
Write-Host ("  Restoring to " + $originalDuty + "%...") -ForegroundColor Yellow
$body = @{ duty_percent = [int]$originalDuty } | ConvertTo-Json
try {
    Invoke-RestMethod -Uri "$base/fans/$FanId/pwm" -Method POST -Headers $hdr -ContentType "application/json" -Body $body | Out-Null
    Write-Host "  [OK] Restored" -ForegroundColor Green
} catch {
    Write-Host ("[WARN] Restore failed: " + $_.Exception.Message) -ForegroundColor Red
    Write-Host "  Kill daemon (Ctrl-C) to let BIOS reclaim fan control." -ForegroundColor Red
}

# 5. Final snapshot
Start-Sleep -Seconds 2
$snap = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
$f = $snap | Where-Object { $_.id -eq $FanId }
if ($f) {
    Write-Host ("  After:  " + $f.duty_percent + "% duty, " + $f.rpm + " RPM") -ForegroundColor Green
}

Write-Host ""
Write-Host "=== Test complete ===" -ForegroundColor Cyan
Write-Host ""
