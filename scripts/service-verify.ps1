$ErrorActionPreference = 'SilentlyContinue'

function Check([string]$label, [bool]$ok) {
    if ($ok) { Write-Host ("  [PASS] " + $label) -ForegroundColor Green }
    else { Write-Host ("  [FAIL] " + $label) -ForegroundColor Red }
}

Write-Host ""
Write-Host "=== ThermalIQ Service Verification ===" -ForegroundColor Cyan
Write-Host ""

$svc = Get-Service ThermalIQ -ErrorAction SilentlyContinue
Check "Service registered" ($null -ne $svc)
Check "Service running" ($svc.Status -eq "Running")
Check "Service auto-start" ($svc.StartType -eq "Automatic")

Start-Sleep 2
$tokenPath = $env:APPDATA + "\thermaliq\api.token"
Check "Token file exists" (Test-Path $tokenPath)

$tok = ""
if (Test-Path $tokenPath) { $tok = (Get-Content $tokenPath -Raw).Trim() }

$health = $null
try { $health = Invoke-RestMethod http://127.0.0.1:7272/health -Headers @{Authorization="Bearer $tok"} } catch {}
Check "Daemon /health responds" ($null -ne $health -and $health.status -eq "ok")

$sensors = $null
try { $sensors = (Invoke-RestMethod http://127.0.0.1:7272/sensors -Headers @{Authorization="Bearer $tok"}).sensors } catch {}
Check "Sensors available" ($null -ne $sensors -and $sensors.Count -gt 0)

$startMenu = Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ThermalIQ.lnk"
Check "Start menu shortcut" $startMenu

$fw = Get-NetFirewallRule -DisplayName "ThermalIQ*" -ErrorAction SilentlyContinue
if ($null -ne $fw) { Write-Host "  [INFO] Firewall rule found" -ForegroundColor Yellow }
else { Write-Host "  [INFO] No firewall rule (OK for loopback)" -ForegroundColor Gray }

Write-Host ""
Write-Host "=== Done ===" -ForegroundColor Cyan
Write-Host ""
