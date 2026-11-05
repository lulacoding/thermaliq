# Gate G1 (SPEC §8), run elevated on the Windows PC with the NVIDIA GPU. Not for CI.
# Builds the daemon with fault injection (debug only), installs it as the real service with the
# bundled `balanced` profile (its GPU roles mapped to this PC's first NVIDIA GPU), then:
# running + GPU fan controlled -> 97 C holds the latch and 100% for 30 s -> latch releases after
# >= 30 s -> hard kill restarts the service within 5 s and the engine controls the fan again ->
# stop returns the fan to automatic within 5 s -> uninstall. Always leaves the fan on automatic.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\debug\thermaliq-daemon.exe'
$dir = Join-Path $env:ProgramData 'ThermalIQ'
$inject = Join-Path $dir 'inject_cpu_temp.txt'
$roles = Join-Path $dir 'roles.toml'
$rolesBak = Join-Path $dir 'roles.toml.g1bak'
$cfgBak = Join-Path $dir 'config.toml.g1bak'
$api = 'http://127.0.0.1:7272'
$svcKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\ThermalIQ'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole('Administrators')) {
    throw 'run this from an elevated (Administrator) PowerShell'
}

$env:PATH += ";$env:ProgramFiles\NVIDIA Corporation\NVSMI"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Nvml {
    [DllImport("nvml.dll")] static extern int nvmlInit_v2();
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetCount_v2(out uint n);
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetHandleByIndex_v2(uint i, out IntPtr dev);
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetNumFans(IntPtr dev, out uint n);
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetFanControlPolicy_v2(IntPtr dev, uint fan, out int policy);
    [DllImport("nvml.dll")] static extern int nvmlDeviceSetDefaultFanSpeed_v2(IntPtr dev, uint fan);
    static void Ok(int r, string what) { if (r != 0) throw new Exception(what + " failed: NVML error " + r); }
    // One entry per fan: true = automatic policy (0), false = manual (1).
    public static bool[] Auto() {
        var res = new System.Collections.Generic.List<bool>();
        Each((dev, fan) => { int p; Ok(nvmlDeviceGetFanControlPolicy_v2(dev, fan, out p), "GetFanControlPolicy"); res.Add(p == 0); });
        return res.ToArray();
    }
    public static void RestoreAll() { Each((dev, fan) => Ok(nvmlDeviceSetDefaultFanSpeed_v2(dev, fan), "SetDefaultFanSpeed")); }
    static void Each(Action<IntPtr, uint> f) {
        Ok(nvmlInit_v2(), "nvmlInit");
        uint gpus; Ok(nvmlDeviceGetCount_v2(out gpus), "GetCount");
        for (uint g = 0; g < gpus; g++) {
            IntPtr dev; Ok(nvmlDeviceGetHandleByIndex_v2(g, out dev), "GetHandle");
            uint fans; Ok(nvmlDeviceGetNumFans(dev, out fans), "GetNumFans");
            for (uint i = 0; i < fans; i++) f(dev, i);
        }
    }
}
'@

function Wait-Until([scriptblock]$Cond, [double]$Seconds, [string]$What) {
    $end = [DateTime]::UtcNow.AddSeconds($Seconds)
    $last = ''
    while ([DateTime]::UtcNow -lt $end) {
        try { if (& $Cond) { return }; $last = 'condition false' } catch { $last = $_.Exception.Message }
        Start-Sleep -Milliseconds 250
    }
    throw "timed out after ${Seconds}s waiting for $What (last: $last)"
}
function Get-Status { Invoke-RestMethod "$api/status" -TimeoutSec 2 }
function Get-GpuFans { @(Invoke-RestMethod "$api/fans" -TimeoutSec 2 | Where-Object { $_.id -like 'nvml_*' }) }
function Test-FansAtLeast98 { $f = Get-GpuFans; $f.Count -gt 0 -and -not ($f | Where-Object { $_.duty_percent -lt 98 }) }
function Set-Injection([string]$Value) { Set-Content -Path $inject -Value $Value -Encoding ascii }
function Test-AllManual { $a = [Nvml]::Auto(); $a.Count -gt 0 -and -not ($a -contains $true) }
function Test-AllAuto { $a = [Nvml]::Auto(); $a.Count -gt 0 -and -not ($a -contains $false) }
function Wait-Controlled { Wait-Until { (Get-Status).engine_running -and (Get-GpuFans).Count -gt 0 -and (Test-AllManual) } 15 'the engine to take manual control of the GPU fan' }

$script:wroteCfg = $false
$script:failed = $false
$script:results = @()
function Step([string]$Name, [scriptblock]$Body) {
    if ($script:failed) { $script:results += "SKIP  $Name"; return }
    try { & $Body; $script:results += "PASS  $Name" }
    catch { $script:failed = $true; $script:results += "FAIL  $Name  -- $($_.Exception.Message)" }
    Write-Host $script:results[-1]
}

try {
    Step 'build with gate-tests' {
        Push-Location $root
        try { cargo build -p thermaliq-daemon --features gate-tests; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } } finally { Pop-Location }
    }
    Step 'install and start the service' {
        & $exe install-service
        if ($LASTEXITCODE -ne 0) { throw 'install-service failed (an existing ThermalIQ install may need uninstall-service first)' }
        # The bundled `balanced` profile names roles (SPEC 5.2). Map the GPU fan and temperature to
        # this PC's first NVIDIA GPU, as `thermaliq-daemon identify` would, and run that profile.
        # Its case-fan roles stay unmapped, so those controls are skipped and logged, which is
        # the behaviour on a PC with only a GPU fan.
        $uuid = "$(nvidia-smi --query-gpu=uuid --format=csv,noheader | Select-Object -First 1)".Trim()
        if (-not $uuid) { throw 'nvidia-smi gave no GPU UUID' }
        $cfg = Join-Path $dir 'config.toml'
        if (Test-Path $cfg) { Copy-Item $cfg $cfgBak -Force }
        if (Test-Path $roles) { Copy-Item $roles $rolesBak -Force }
        $script:wroteCfg = $true
        Set-Content -Path $roles -Encoding ascii -Value "[roles]`ngpu_fan = `"nvml_${uuid}_fan0`"`ngpu_temp = `"nvml_${uuid}_temp`"`n"
        Set-Content -Path $cfg -Encoding ascii -Value "default_profile = `"balanced`"`n"
        Set-Injection ''   # armed, not injecting
        Set-ItemProperty -Path $svcKey -Name Environment -Type MultiString -Value @("THERMALIQ_INJECT_CPU_TEMP=$inject")
        Start-Service ThermalIQ
        Wait-Until { (Get-Status).engine_running } 10 '/status to report the engine running'
    }
    Step '/status running, GPU fan controlled' {
        $s = Get-Status
        if (-not $s.engine_running) { throw 'engine_running is false' }
        Wait-Controlled
        $t = (Get-Status).ticks
        Wait-Until { (Get-Status).ticks -gt $t } 5 'ticks to advance'
    }
    Step 'agent running; pause and resume work' {
        if ((Get-Status).agent -ne 'running') { throw "agent is '$((Get-Status).agent)', not running" }
        $auth = @{ Authorization = "Bearer $((Get-Content (Join-Path $dir 'token.txt') -Raw).Trim())" }
        Invoke-RestMethod -Method Post "$api/agent/pause" -Headers $auth -TimeoutSec 5 | Out-Null
        if ((Get-Status).agent -ne 'paused') { throw 'pause did not show in /status' }
        Invoke-RestMethod -Method Post "$api/agent/resume" -Headers $auth -TimeoutSec 5 | Out-Null
        if ((Get-Status).agent -ne 'running') { throw 'resume did not show in /status' }
    }
    Step 'inject 97 C: latched, GPU fan >= 98% for 30 s' {
        Set-Injection '97'
        Wait-Until { (Get-Status).latched } 10 'the latch'
        # NVML's readback follows the fan's slew (M10: 75% took ~9 s), so wait for it, then hold.
        Wait-Until { (Get-Status).latched -and (Test-FansAtLeast98) } 20 'the GPU fan to read >= 98%'
        $end = [DateTime]::UtcNow.AddSeconds(30)
        while ([DateTime]::UtcNow -lt $end) {
            if (-not (Get-Status).latched) { throw 'latch dropped inside 30 s' }
            $low = Get-GpuFans | Where-Object { $_.duty_percent -lt 98 }
            if ($low) { throw "fan $($low[0].id) at $($low[0].duty_percent)%, below 98%" }
            Start-Sleep -Milliseconds 500
        }
    }
    Step 'clear injection: latch releases after >= 30 s' {
        Set-Injection ''
        $t0 = [DateTime]::UtcNow
        Wait-Until { -not (Get-Status).latched } 120 'the latch to release'
        $held = ([DateTime]::UtcNow - $t0).TotalSeconds
        if ($held -lt 30) { throw "released after only $([math]::Round($held, 1)) s" }
        Write-Host "      released after $([math]::Round($held, 1)) s"
    }
    Step 'taskkill /F: service running again within 5 s' {
        $pid_ = (Get-CimInstance Win32_Service -Filter "Name='ThermalIQ'").ProcessId
        if (-not $pid_) { throw 'service has no process' }
        $t0 = [DateTime]::UtcNow
        taskkill /F /PID $pid_ | Out-Null
        Wait-Until { (Get-Service ThermalIQ).Status -eq 'Running' -and (Get-CimInstance Win32_Service -Filter "Name='ThermalIQ'").ProcessId -ne $pid_ } 5 'the service to run again'
        Write-Host "      running again after $([math]::Round(([DateTime]::UtcNow - $t0).TotalSeconds, 1)) s"
    }
    Step 'after the kill the engine controls the fan' {
        # The fan stays manual across a kill, so prove control by commanding it: 97 C must give 100%.
        Wait-Until { (Get-Status).engine_running } 10 'the API'
        if ((Get-Status).agent -ne 'running') { throw "agent is '$((Get-Status).agent)' after the restart" }
        Set-Injection '97'
        Wait-Until { (Get-Status).latched -and (Test-FansAtLeast98) } 20 'latch and the GPU fan >= 98%'
        Set-Injection ''
    }
    Step 'stop service: NVML automatic fan policy within 5 s' {
        $t0 = [DateTime]::UtcNow
        Stop-Service ThermalIQ -ErrorAction Stop
        Wait-Until { Test-AllAuto } 5 'automatic fan policy'
        Write-Host "      automatic after $([math]::Round(([DateTime]::UtcNow - $t0).TotalSeconds, 1)) s"
    }
    Step 'uninstall' {
        & $exe uninstall-service
        if ($LASTEXITCODE -ne 0) { throw 'uninstall-service failed' }
        if (Get-Service ThermalIQ -ErrorAction SilentlyContinue) { throw 'service still registered' }
    }
}
finally {
    # Whatever happened: no service, no injection, and the fan back on the driver's own curve.
    if (Test-Path $exe) { try { & $exe uninstall-service 2>$null | Out-Null } catch { } }
    Remove-Item $inject -ErrorAction SilentlyContinue
    if ($script:wroteCfg) {   # only undo our own config.toml and roles.toml
        if (Test-Path $cfgBak) { Move-Item $cfgBak (Join-Path $dir 'config.toml') -Force }
        else { Remove-Item (Join-Path $dir 'config.toml') -ErrorAction SilentlyContinue }
        if (Test-Path $rolesBak) { Move-Item $rolesBak $roles -Force }
        else { Remove-Item $roles -ErrorAction SilentlyContinue }
    }
    try {
        [Nvml]::RestoreAll()
        if (Test-AllAuto) { Write-Host 'fan: automatic' } else { Write-Host 'FAN NOT AUTOMATIC: set it back in your GPU tool' -ForegroundColor Red; $script:failed = $true }
    }
    catch { Write-Host "COULD NOT RESTORE THE FAN: $($_.Exception.Message)" -ForegroundColor Red; $script:failed = $true }
    Write-Host ''
    $script:results | ForEach-Object { Write-Host $_ }
    if ($script:failed) { Write-Host 'G1: FAIL' -ForegroundColor Red; exit 1 } else { Write-Host 'G1: PASS' -ForegroundColor Green }
}
