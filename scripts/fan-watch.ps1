param(
    [int]$IntervalSeconds = 5,
    [int]$Count = 12
)

# fan-watch.ps1 — polls /fans every N seconds, prints RPM/duty table
# Usage: .\scripts\fan-watch.ps1 -IntervalSeconds 5 -Count 12

$ErrorActionPreference = 'Stop'

$tok = (Get-Content "$env:APPDATA\thermaliq\api.token" -Raw).Trim()
$hdr = @{ Authorization = "Bearer $tok" }
$base = "http://127.0.0.1:7272"

Write-Host ""
Write-Host "=== Fan Watch ===" -ForegroundColor Cyan
Write-Host ("  Interval: " + $IntervalSeconds + "s, Samples: " + $Count)
Write-Host ""

for ($i = 1; $i -le $Count; $i++) {
    $ts = Get-Date -Format 'HH:mm:ss'
    $fans = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
    Write-Host ("  [$ts] Sample $i/$Count") -ForegroundColor White
    foreach ($f in $fans) {
        Write-Host ("    " + $f.id.PadRight(35) + $f.rpm.ToString().PadLeft(6) + " RPM   " + $f.duty_percent.ToString().PadLeft(3) + "% duty")
    }
    if ($i -lt $Count) { Start-Sleep -Seconds $IntervalSeconds }
}

Write-Host ""
Write-Host "=== Watch complete ===" -ForegroundColor Cyan
Write-Host ""
