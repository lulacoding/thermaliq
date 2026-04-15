; ThermalIQ custom NSIS installer hooks
; Registers / removes the daemon Windows service around install and uninstall.

; Called after the main installation section completes.
!macro NSIS_HOOK_INSTFILES
  DetailPrint "Registering ThermalIQ daemon as a Windows service..."
  nsExec::ExecToLog '"$INSTDIR\thermaliq-daemon.exe" install-service'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONEXCLAMATION \
      "Service registration returned code $0.$\nThermalIQ will still work but won't start automatically at login.$\nRun '$INSTDIR\thermaliq-daemon.exe install-service' from an elevated prompt to retry."
  ${EndIf}
!macroend

; Called before the main uninstallation section removes files.
!macro NSIS_HOOK_UNINSTFILES
  DetailPrint "Stopping and removing ThermalIQ daemon service..."
  nsExec::ExecToLog '"$INSTDIR\thermaliq-daemon.exe" uninstall-service'
  ; Ignore errors — service may not exist if install-service was never run.
!macroend
