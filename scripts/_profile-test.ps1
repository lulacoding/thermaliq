$tok = (Get-Content "$env:APPDATA\thermaliq\api.token" -Raw).Trim()
$hdr = @{ Authorization = "Bearer $tok" }
$ct = "application/json"
$base = "http://127.0.0.1:7272"

Write-Host "Activating silent..."
Invoke-RestMethod -Uri "$base/api/profiles/active" -Method POST -Headers $hdr -ContentType $ct -Body '{"name":"silent"}'
Start-Sleep 5

Write-Host "Current fans:"
$fans = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
foreach ($f in $fans) { Write-Host ("  " + $f.id + ": " + $f.duty_percent + "% " + $f.rpm + " RPM") }

Write-Host "Switching to balanced..."
Invoke-RestMethod -Uri "$base/api/profiles/active" -Method POST -Headers $hdr -ContentType $ct -Body '{"name":"balanced"}'
Start-Sleep 5

Write-Host "Current fans:"
$fans = (Invoke-RestMethod -Uri "$base/fans" -Headers $hdr).fans
foreach ($f in $fans) { Write-Host ("  " + $f.id + ": " + $f.duty_percent + "% " + $f.rpm + " RPM") }

Write-Host "Done."
