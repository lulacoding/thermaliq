# One-time machine setup for ThermalIQ v2 (Windows, PowerShell).
$ErrorActionPreference = "Stop"

function Has($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

if (-not (Has cargo)) { Write-Error "Install Rust first: https://rustup.rs" }
if (-not (Has node))  { Write-Warning "node not on PATH - the Ponytail plugin's hooks need it" }

# RTK (Rust Token Killer). Native Windows hooks work from rtk v0.37+.
$rtkOk = $false
if (Has rtk) { rtk gain *> $null; $rtkOk = ($LASTEXITCODE -eq 0) }
if (-not $rtkOk) { cargo install --git https://github.com/rtk-ai/rtk }
rtk init -g
rtk init --show

# Reference worktree for the v0 code.
Set-Location (git rev-parse --show-toplevel)
if (-not (Test-Path ..\thermaliq-v0)) {
    git fetch origin tag v0-archive --no-tags
    git worktree add ..\thermaliq-v0 v0-archive
}

Write-Host ""
Write-Host "Done. Restart Claude Code in this folder."
Write-Host "Ponytail: accept the plugin prompt on first start, or run:"
Write-Host "  /plugin marketplace add DietrichGebert/ponytail"
Write-Host "  /plugin install ponytail@ponytail"
