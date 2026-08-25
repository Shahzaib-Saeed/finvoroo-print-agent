; Finvoroo Print Agent — NSIS hooks for the standalone Windows installer.
; Pairing tokens live in %APPDATA%\com.finvoroo.print-agent\ and must survive updates.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Launch the tray agent so pairing can start immediately. Do not ExecWait —
  ; the installer would hang until the user quits the agent.
  Exec '"$INSTDIR\${MAINBINARYNAME}.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Leave %APPDATA%\com.finvoroo.print-agent (token, pairing, default printer).
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
