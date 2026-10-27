; Hooks into Tauri's NSIS installer (tauri.windows.conf.json, bundle.windows.nsis.installerHooks).

; After copying the new files, the installer deletes the previous install's main binary when its
; name changed (the uninstall key's MainBinaryName). Builds before the app became brigadier-app.exe
; installed it as brigadier.exe, which is now the command's name: that cleanup would delete the
; command just copied. Forgetting the old name first keeps it; the copy overwrites the old app.
!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R9 SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $R9 == "brigadier.exe"
    DeleteRegValue SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${EndIf}
!macroend
