# CI smoke test (Windows, elevated): install the mock-hardware daemon as a real service,
# start it, stop it through the SCM, and check the stop was clean and quick. Always uninstalls.
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot '..\target\debug\thermaliq-daemon.exe' | Resolve-Path
$dir = Join-Path $env:ProgramData 'ThermalIQ'

& $exe install-service
if ($LASTEXITCODE -ne 0) { throw "install-service failed" }
try {
    # G1: any crash restarts the service after 1 s, first, second and later failures.
    $fail = sc.exe qfailure ThermalIQ | Out-String
    if ([regex]::Matches($fail, 'RESTART -- Delay = 1000 milliseconds').Count -ne 3) { throw "recovery actions wrong:`n$fail" }

    Start-Service ThermalIQ
    $up = $false
    foreach ($i in 1..50) {
        try { Invoke-RestMethod http://127.0.0.1:7272/status | Out-Null; $up = $true; break } catch { Start-Sleep -Milliseconds 200 }
    }
    if (-not $up) { throw "API did not come up" }

    $acl = (icacls $dir) -join "`n"
    if ($acl -match 'BUILTIN\\Users:\((OI|CI|I)*\)*\((F|M|W|WD|AD)') { throw "Users can write the data dir:`n$acl" }
    if ((icacls (Join-Path $dir 'token.txt')) -match 'Users|Everyone') { throw "token.txt is open to Users" }

    $t = Measure-Command { Stop-Service ThermalIQ -ErrorAction Stop; (Get-Service ThermalIQ).WaitForStatus('Stopped', '00:00:20') }
    if ($t.TotalSeconds -gt 10) { throw "stop took $($t.TotalSeconds)s" }
    $q = sc.exe query ThermalIQ | Out-String
    if ($q -notmatch 'STOPPED' -or $q -notmatch 'WIN32_EXIT_CODE\s*:\s*0\b') { throw "unclean stop:`n$q" }
    "stopped cleanly in $($t.TotalSeconds)s"
}
catch {
    Get-Content (Join-Path $dir 'service.log') -ErrorAction SilentlyContinue
    throw
}
finally {
    & $exe uninstall-service
}
