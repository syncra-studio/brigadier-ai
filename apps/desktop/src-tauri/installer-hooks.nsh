; Hooks into Tauri's NSIS installer (tauri.windows.conf.json, bundle.windows.nsis.installerHooks).

; After copying the new files, the installer deletes the previous install's main binary when its
; name changed (the uninstall key's MainBinaryName). Builds before the app became brigadier-app.exe
; installed it as brigadier.exe, which is now the command's name, so that cleanup deleted the
; command just copied. No name this installer writes may be deleted that way: when the old name is
; one of them, it is forgotten first, and the copy has already replaced the old file. The list is
; every file the installer writes beside the app (installer.rs checks it against the
; configuration), and the nightly installs over each of them (tools/ci/installer-upgrade.ps1).
!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R9 SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $R9 == "brigadierd.exe"
  ${OrIf} $R9 == "brigadier.exe"
  ${OrIf} $R9 == "brigadier-cu.exe"
  ${OrIf} $R9 == "uninstall.exe"
    DeleteRegValue SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${EndIf}
!macroend
