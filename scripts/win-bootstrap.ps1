#Requires -RunAsAdministrator
param()

$ErrorActionPreference = 'Stop'
$LogFile = "C:\dev\thermaliq-bootstrap-" + (Get-Date -Format 'yyyyMMdd-HHmmss') + ".log"

function Log([string]$msg) {
    $line = "[" + (Get-Date -Format 'HH:mm:ss') + "] " + $msg
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
}

function Stage([int]$n, [string]$label) {
    $bar = "=" * 60
    Log ""
    Log $bar
    Log ("STAGE " + $n + " --- " + $label)
    Log $bar
}

function Assert-Cmd([string]$cmd) {
    $found = Get-Command $cmd -ErrorAction SilentlyContinue
    if (-not $found) { throw ("MISSING: " + $cmd + " not on PATH after install") }
    try { $ver = (& $cmd --version 2>$null) | Select-Object -First 1 } catch { $ver = "?" }
    Log ("  OK: " + $cmd + " -> " + $ver)
}

function Reload-Path {
    $machine = [System.Environment]::GetEnvironmentVariable('PATH', 'Machine')
    $user    = [System.Environment]::GetEnvironmentVariable('PATH', 'User')
    $env:PATH = $machine + ";" + $user
}

New-Item -ItemType Directory -Force -Path "C:\dev" | Out-Null
Log ("Log file: " + $LogFile)
Log ("Bootstrap started at " + (Get-Date))

# ---------------------------------------------------------------------------
Stage 1 "Rust toolchain"
# ---------------------------------------------------------------------------

if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    Log "Downloading rustup-init.exe..."
    $rustupPath = $env:TEMP + "\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupPath
    Log "Running rustup-init (stable MSVC toolchain)..."
    & $rustupPath -y --default-toolchain stable --default-host x86_64-pc-windows-msvc
    Reload-Path
} else {
    Log "  rustc already installed, running rustup update..."
    rustup update stable
}

Assert-Cmd "rustc"
Assert-Cmd "cargo"
Log "STAGE 1 COMPLETE"

# ---------------------------------------------------------------------------
Stage 2 "Visual Studio Build Tools (MSVC linker)"
# ---------------------------------------------------------------------------

if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    Log "Installing VS 2022 Build Tools (10-15 min)..."
    $args_str = "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
    winget install --id Microsoft.VisualStudio.2022.BuildTools --accept-package-agreements --accept-source-agreements --override $args_str
    Reload-Path

    # Try to source vcvars64 for this session
    $paths = @(
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    )
    foreach ($p in $paths) {
        if (Test-Path $p) {
            Log ("  Sourcing " + $p)
            $envdump = cmd /c ('"' + $p + '" && set')
            foreach ($line in $envdump) {
                if ($line -match '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
                    [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
                }
            }
            break
        }
    }
} else {
    Log "  link.exe already on PATH."
}

if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    throw "link.exe still not found. Restart PowerShell after VS Build Tools finishes, then re-run."
}
Log "  OK: link.exe found"
Log "STAGE 2 COMPLETE"

# ---------------------------------------------------------------------------
Stage 3 "Node.js 20 LTS"
# ---------------------------------------------------------------------------

$nodeOk = $false
try { $v = node --version; $nodeOk = ($v -match '^v2') } catch {}

if (-not $nodeOk) {
    Log "Installing Node.js LTS..."
    winget install --id OpenJS.NodeJS.LTS --accept-package-agreements --accept-source-agreements
    Reload-Path
} else {
    Log ("  Node.js already installed: " + $v)
}

Assert-Cmd "node"
Assert-Cmd "npm"
Log "STAGE 3 COMPLETE"

# ---------------------------------------------------------------------------
Stage 4 "Git"
# ---------------------------------------------------------------------------

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Log "Installing Git..."
    winget install --id Git.Git --accept-package-agreements --accept-source-agreements
    Reload-Path
}
Assert-Cmd "git"
Log "STAGE 4 COMPLETE"

# ---------------------------------------------------------------------------
Stage 5 "GitHub CLI"
# ---------------------------------------------------------------------------

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Log "Installing GitHub CLI..."
    winget install --id GitHub.cli --accept-package-agreements --accept-source-agreements
    Reload-Path
}
Assert-Cmd "gh"
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
    Log "  Repo already present at C:\dev\thermaliq -- pulling latest..."
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

Log "Installing UI deps (npm install)..."
Set-Location "C:\dev\thermaliq\thermaliq-ui"
npm install
Set-Location "C:\dev\thermaliq"

Log "Checking icon assets..."
$iconDir = "thermaliq-ui\src-tauri\icons"
$icons = @("32x32.png","128x128.png","128x128@2x.png","icon.ico","icon.icns")
$missing = $icons | Where-Object { -not (Test-Path ($iconDir + "\" + $_)) }
if ($missing) {
    Log ("  Missing icons: " + ($missing -join ", ") + " -- generating from icon.png...")
    Set-Location "C:\dev\thermaliq\thermaliq-ui"
    npx @tauri-apps/cli icon "src-tauri\icons\icon.png"
    Set-Location "C:\dev\thermaliq"
    $stillMissing = $icons | Where-Object { -not (Test-Path ($iconDir + "\" + $_)) }
    if ($stillMissing) {
        throw ("Icon generation failed. Still missing: " + ($stillMissing -join ", "))
    }
    Log "  Icons generated OK."
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
    Log "  WARNING: native build failed -- see output above."
    Log "  Common fixes:"
    Log "    wmi link error      -> confirm MSVC on PATH (Stage 2)"
    Log "    nvml-wrapper error  -> harmless without NVIDIA driver; links at runtime"
    Log "    windows-sys mismatch -> run: cargo update -p windows-sys"
}

Log "Running: npm run tauri build (15-25 min)..."
Set-Location "C:\dev\thermaliq\thermaliq-ui"
$env:PATH = ".\node_modules\.bin;" + $env:PATH
npm run tauri build
Set-Location "C:\dev\thermaliq"

$nsis = Get-ChildItem "thermaliq-ui\src-tauri\target\release\bundle\nsis\*.exe" -ErrorAction SilentlyContinue
if ($nsis) {
    $mb = [math]::Round($nsis.Length / 1MB, 1)
    Log ("  NSIS installer: " + $nsis.FullName)
    Log ("  Size: " + $mb + " MB")
} else {
    throw "NSIS installer not found. Check Tauri build output above."
}
Log "STAGE 8 COMPLETE"

# ---------------------------------------------------------------------------
Stage 9 "Smoke test"
# ---------------------------------------------------------------------------

Log "Running smoke test..."
& "C:\dev\thermaliq\scripts\win-smoke.ps1"
Log "STAGE 9 COMPLETE"

# ---------------------------------------------------------------------------
$bar = "=" * 60
Log ""
Log $bar
Log "ALL STAGES COMPLETE"
Log ("Log saved to: " + $LogFile)
Log "Paste this log back to Claude if anything went wrong."
Log $bar
