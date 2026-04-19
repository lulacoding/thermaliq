#Requires -RunAsAdministrator
<#
.SYNOPSIS
    ThermalIQ Session 1 — Windows dev environment bootstrap.
    Runs steps 1.1 through 1.4 with error trapping and progress logging.
.USAGE
    # Open PowerShell as Administrator, then:
    cd C:\dev\thermaliq
    .\scripts\win-bootstrap.ps1
#>

$ErrorActionPreference = 'Stop'
$LogFile = "C:\dev\thermaliq-bootstrap-$(Get-Date -Format 'yyyyMMdd-HHmmss').log"

function Log {
    param([string]$msg)
    $ts = Get-Date -Format 'HH:mm:ss'
    $line = "[$ts] $msg"
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
}

function Stage {
    param([int]$n, [string]$label)
    $banner = "=" * 60
    Log ""
    Log $banner
    Log "STAGE $n — $label"
    Log $banner
}

function Assert-Command {
    param([string]$cmd, [string]$minVersion = "")
    try {
        $out = & $cmd --version 2>&1 | Select-Object -First 1
        Log "  OK: $cmd -> $out"
    } catch {
        throw "MISSING: $cmd not found on PATH after install."
    }
}

New-Item -ItemType Directory -Force -Path "C:\dev" | Out-Null
Log "Log file: $LogFile"
Log "Bootstrap started at $(Get-Date)"

# ---------------------------------------------------------------------------
Stage 1 "Rust toolchain"
# ---------------------------------------------------------------------------

if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    Log "Downloading rustup-init.exe..."
    $rustupPath = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupPath
    Log "Running rustup-init (default install, MSVC toolchain)..."
    & $rustupPath -y --default-toolchain stable --default-host x86_64-pc-windows-msvc
    # Reload PATH
    $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("PATH","User")
} else {
    Log "  rustc already installed, running rustup update..."
    rustup update stable
}

Assert-Command "rustc"
Assert-Command "cargo"
Log "STAGE 1 COMPLETE"

# ---------------------------------------------------------------------------
Stage 2 "Visual Studio Build Tools (MSVC linker)"
# ---------------------------------------------------------------------------

if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    Log "Installing VS 2022 Build Tools..."
    winget install --id Microsoft.VisualStudio.2022.BuildTools --accept-package-agreements --accept-source-agreements `
        --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
    # Add MSVC to PATH for this session
    $vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    if (-not (Test-Path $vcvars)) {
        $vcvars = "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    }
    if (Test-Path $vcvars) {
        Log "  Sourcing vcvars64.bat..."
        cmd /c "`"$vcvars`" && set" | ForEach-Object {
            if ($_ -match "^([^=]+)=(.*)$") {
                [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
            }
        }
    }
} else {
    Log "  link.exe already on PATH — skipping VS Build Tools install."
}

if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    throw "link.exe still not found. Restart PowerShell and re-run after VS Build Tools install completes."
}
Log "  OK: link.exe found"
Log "STAGE 2 COMPLETE"

# ---------------------------------------------------------------------------
Stage 3 "Node.js 20 LTS"
# ---------------------------------------------------------------------------

$nodeOk = $false
try { $nodeVer = node --version; $nodeOk = $nodeVer -match "^v20\." } catch {}

if (-not $nodeOk) {
    Log "Installing Node.js 20 LTS..."
    winget install --id OpenJS.NodeJS.LTS --accept-package-agreements --accept-source-agreements
    $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("PATH","User")
} else {
    Log "  Node.js already installed: $nodeVer"
}

Assert-Command "node"
Assert-Command "npm"
Log "STAGE 3 COMPLETE"

# ---------------------------------------------------------------------------
Stage 4 "Git"
# ---------------------------------------------------------------------------

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Log "Installing Git..."
    winget install --id Git.Git --accept-package-agreements --accept-source-agreements
    $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("PATH","User")
}
Assert-Command "git"
Log "STAGE 4 COMPLETE"

# ---------------------------------------------------------------------------
Stage 5 "GitHub CLI"
# ---------------------------------------------------------------------------

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Log "Installing GitHub CLI..."
    winget install --id GitHub.cli --accept-package-agreements --accept-source-agreements
    $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("PATH","User")
}
Assert-Command "gh"
Log "NOTE: run 'gh auth login' manually if not already authenticated."
Log "STAGE 5 COMPLETE"

# ---------------------------------------------------------------------------
Stage 6 "Clone repo (if not already present)"
# ---------------------------------------------------------------------------

if (-not (Test-Path "C:\dev\thermaliq\.git")) {
    Log "Cloning thermaliq..."
    Set-Location "C:\dev"
    git clone https://github.com/lulacoding/thermaliq.git
} else {
    Log "  Repo already present at C:\dev\thermaliq — pulling latest..."
    Set-Location "C:\dev\thermaliq"
    git pull
}
Set-Location "C:\dev\thermaliq"
Log "STAGE 6 COMPLETE"

# ---------------------------------------------------------------------------
Stage 7 "Mock build + tests"
# ---------------------------------------------------------------------------

Log "Running: cargo build --features mock-hardware"
cargo build --features mock-hardware
Log "  cargo build (mock) succeeded."

Log "Running: cargo test --features mock-hardware"
cargo test --features mock-hardware
Log "  cargo test (mock) passed."

Log "Installing UI deps (npm ci)..."
Set-Location "C:\dev\thermaliq\thermaliq-ui"
npm ci
Set-Location "C:\dev\thermaliq"

Log "Checking icon assets..."
$icons = @("32x32.png","128x128.png","128x128@2x.png","icon.ico","icon.icns")
$iconDir = "thermaliq-ui\src-tauri\icons"
$missing = @()
foreach ($icon in $icons) {
    if (-not (Test-Path "$iconDir\$icon")) { $missing += $icon }
}
if ($missing.Count -gt 0) {
    Log "  WARNING: missing icons: $($missing -join ', ')"
    Log "  Fix: npx @tauri-apps/cli icon <path-to-1024x1024.png>"
} else {
    Log "  All icon assets present."
}
Log "STAGE 7 COMPLETE"

# ---------------------------------------------------------------------------
Stage 8 "Native (real HAL) build"
# ---------------------------------------------------------------------------

Log "Running: cargo build (native HAL)"
try {
    cargo build
    Log "  cargo build (native) succeeded."
} catch {
    Log "  WARNING: native build failed — see output above."
    Log "  Common fixes:"
    Log "    - wmi link error    -> confirm MSVC on PATH (Stage 2)"
    Log "    - nvml-wrapper error -> harmless if no NVIDIA driver; links at runtime"
    Log "    - windows-sys mismatch -> run: cargo update -p windows-sys"
    Log "  Continuing to Tauri build — it may also fail but gives better diagnostics."
}

Log "Running: npm run tauri build (this takes 15-25 min)..."
Set-Location "C:\dev\thermaliq\thermaliq-ui"
npm run tauri build
Set-Location "C:\dev\thermaliq"

$nsis = Get-ChildItem "thermaliq-ui\src-tauri\target\release\bundle\nsis\*.exe" -ErrorAction SilentlyContinue
if ($nsis) {
    Log "  NSIS installer: $($nsis.FullName)"
    Log "  Size: $([math]::Round($nsis.Length / 1MB, 1)) MB"
} else {
    throw "NSIS installer not found. Check Tauri build output above."
}
Log "STAGE 8 COMPLETE"

# ---------------------------------------------------------------------------
Stage 9 "Smoke test (win-smoke.ps1)"
# ---------------------------------------------------------------------------

Log "Running smoke test script..."
& "C:\dev\thermaliq\scripts\win-smoke.ps1"
Log "STAGE 9 COMPLETE"

# ---------------------------------------------------------------------------
Log ""
Log ("=" * 60)
Log "ALL STAGES COMPLETE"
Log "Log saved to: $LogFile"
Log "Paste this file back to Claude if anything went wrong."
Log ("=" * 60)
