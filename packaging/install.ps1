# ThermalIQ Windows installer bootstrap
# Usage: irm https://thermaliq.dev/install.ps1 | iex
#
# Downloads the latest NSIS installer, verifies SHA256, runs it silently.
# Requires PowerShell 5.1+ (built into Windows 10/11).

#Requires -Version 5.1
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$REPO     = 'lulacoding/thermaliq'
$API_URL  = "https://api.github.com/repos/$REPO/releases/latest"
$INSTALL_DIR = "$env:ProgramFiles\ThermalIQ"

function Write-Step([string]$msg) {
    Write-Host "  ==> $msg" -ForegroundColor Cyan
}

function Write-Success([string]$msg) {
    Write-Host "  OK  $msg" -ForegroundColor Green
}

function Write-Fail([string]$msg) {
    Write-Host " FAIL $msg" -ForegroundColor Red
    exit 1
}

# ── Check elevation ───────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal] `
    [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    Write-Host ""
    Write-Host "  ThermalIQ requires Administrator privileges to install." -ForegroundColor Yellow
    Write-Host "  Re-launching as Administrator..." -ForegroundColor Yellow
    Write-Host ""
    $scriptPath = $MyInvocation.MyCommand.Path
    if ($scriptPath) {
        Start-Process powershell -Verb RunAs -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$scriptPath`""
    } else {
        Write-Fail "Cannot re-launch: script was piped. Run in an elevated PowerShell session."
    }
    exit 0
}

Write-Host ""
Write-Host "  ThermalIQ Installer" -ForegroundColor White
Write-Host "  ===================" -ForegroundColor White
Write-Host ""

# ── Fetch latest release ──────────────────────────────────────────────────────

Write-Step "Fetching latest release from GitHub..."

try {
    $release = Invoke-RestMethod -Uri $API_URL -Headers @{ 'User-Agent' = 'thermaliq-installer' }
} catch {
    Write-Fail "Failed to fetch release info: $_"
}

$version = $release.tag_name
Write-Success "Latest version: $version"

# Find NSIS installer asset
$asset = $release.assets | Where-Object { $_.name -match '_x64-setup\.exe$' } | Select-Object -First 1
if (-not $asset) {
    Write-Fail "No Windows installer found in release $version"
}

# Find matching SHA256 checksum file
$shaAsset = $release.assets | Where-Object { $_.name -match '\.sha256$' -and $_.name -match 'windows' } | Select-Object -First 1

# ── Download ──────────────────────────────────────────────────────────────────

$tmpDir   = Join-Path $env:TEMP "thermaliq-install"
$tmpExe   = Join-Path $tmpDir $asset.name
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

Write-Step "Downloading $($asset.name)..."

try {
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmpExe -UseBasicParsing
} catch {
    Write-Fail "Download failed: $_"
}

Write-Success "Downloaded to $tmpExe"

# ── Verify checksum ───────────────────────────────────────────────────────────

if ($shaAsset) {
    Write-Step "Verifying SHA256 checksum..."
    $shaFile = Join-Path $tmpDir "$($asset.name).sha256"
    Invoke-WebRequest -Uri $shaAsset.browser_download_url -OutFile $shaFile -UseBasicParsing
    $expected = (Get-Content $shaFile -Raw).Trim().Split(' ')[0].ToUpper()
    $actual   = (Get-FileHash -Algorithm SHA256 $tmpExe).Hash.ToUpper()
    if ($actual -ne $expected) {
        Remove-Item $tmpExe -Force
        Write-Fail "SHA256 mismatch!`n  Expected: $expected`n  Got:      $actual"
    }
    Write-Success "Checksum verified"
} else {
    Write-Host "  (!) No checksum file found — skipping verification" -ForegroundColor Yellow
}

# ── Install ───────────────────────────────────────────────────────────────────

Write-Step "Installing ThermalIQ $version (silent, per-machine)..."

$proc = Start-Process -FilePath $tmpExe `
    -ArgumentList "/S /D=$INSTALL_DIR" `
    -Wait -PassThru

if ($proc.ExitCode -ne 0) {
    Write-Fail "Installer exited with code $($proc.ExitCode)"
}

Write-Success "Installed to $INSTALL_DIR"

# ── Register Windows service ──────────────────────────────────────────────────

$daemonExe = Join-Path $INSTALL_DIR 'thermaliq-daemon.exe'
if (Test-Path $daemonExe) {
    Write-Step "Registering ThermalIQ daemon as Windows service..."
    & $daemonExe install-service 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Start-Service ThermalIQ -ErrorAction SilentlyContinue
        Write-Success "Service registered and started"
    } else {
        Write-Host "  (!) Service registration failed — start manually: thermaliq-daemon.exe install-service" -ForegroundColor Yellow
    }
}

# ── Add to PATH ───────────────────────────────────────────────────────────────

$machinePath = [System.Environment]::GetEnvironmentVariable('PATH', 'Machine')
if ($machinePath -notlike "*$INSTALL_DIR*") {
    Write-Step "Adding $INSTALL_DIR to system PATH..."
    [System.Environment]::SetEnvironmentVariable(
        'PATH',
        "$machinePath;$INSTALL_DIR",
        'Machine'
    )
    Write-Success "PATH updated (restart shell to take effect)"
}

# ── Cleanup ───────────────────────────────────────────────────────────────────

Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "  ThermalIQ $version installed successfully!" -ForegroundColor Green
Write-Host ""
Write-Host "  Get started:" -ForegroundColor White
Write-Host "    tq status          # check daemon status"
Write-Host "    tq sensors         # live sensor readings"
Write-Host "    tq fans            # fan speeds"
Write-Host "    tq profile list    # available profiles"
Write-Host ""
Write-Host "  Requires HWiNFO64 for full sensor coverage:" -ForegroundColor Yellow
Write-Host "    https://www.hwinfo.com/download/"
Write-Host ""
