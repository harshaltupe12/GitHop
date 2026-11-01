; GitHop NSIS installer hooks (Phase 6 §5). Included by Tauri's installer.nsi
; at global scope; the NSIS_HOOK_* macros run inside the install/uninstall
; sections. Per-user install (installMode = currentUser), so everything here
; touches HKCU only — never machine-wide settings.
;
; Install:   add $INSTDIR to the user PATH (so terminals and git hooks find
;            githop.exe), then broadcast WM_SETTINGCHANGE.
; Uninstall: run `githop.exe wipe --from-uninstaller` (restores git config,
;            removes hooks; adds --keep-data unless the user ticked "Delete the
;            application data"), then remove the PATH entry.
;            Skipped entirely in /UPDATE mode.

!define /ifndef GITHOP_HWND_BROADCAST 0xFFFF
!define /ifndef GITHOP_WM_SETTINGCHANGE 0x001A

; Rebuild the user PATH without $INSTDIR, then (mode "add") append it once.
; Leaves PATH untouched when it is too long for this NSIS build's string size,
; so a long PATH is never truncated.
;   Push "add" | "remove"
;   Call GitHopUserPath / un.GitHopUserPath
!macro GITHOP_USER_PATH_FUNC UN
Function ${UN}GitHopUserPath
  Exch $9          ; mode
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6

  StrCpy $0 "$INSTDIR"
  ClearErrors
  ReadRegStr $1 HKCU "Environment" "Path"
  StrLen $5 $1
  IntOp $6 ${NSIS_MAX_STRLEN} - 2
  ${If} $5 >= $6
    DetailPrint "Your PATH is too long to change safely. Add $0 to PATH by hand to use githop in terminals."
    Goto githop_path_end
  ${EndIf}

  StrCpy $2 ""     ; result
  StrCpy $3 ""     ; current entry
  StrCpy $4 0      ; index
  githop_path_next:
    IntCmp $4 $5 githop_path_entry 0 githop_path_entry
    StrCpy $6 $1 1 $4
    IntOp $4 $4 + 1
    StrCmp $6 ";" githop_path_entry
    StrCpy $3 "$3$6"
    Goto githop_path_next
  githop_path_entry:
    ; StrCmp is case-insensitive, like Windows paths.
    StrCmp $3 "" githop_path_skip
    StrCmp $3 "$0" githop_path_skip
    StrCmp $3 "$0\" githop_path_skip
    StrCmp $2 "" 0 githop_path_join
      StrCpy $2 "$3"
      Goto githop_path_skip
    githop_path_join:
    StrCpy $2 "$2;$3"
  githop_path_skip:
    StrCpy $3 ""
    IntCmp $4 $5 githop_path_done 0 githop_path_done
    Goto githop_path_next
  githop_path_done:

  StrCmp $9 "add" 0 githop_path_write
    StrCmp $2 "" 0 githop_path_append
      StrCpy $2 "$0"
      Goto githop_path_write
    githop_path_append:
    StrCpy $2 "$2;$0"

  githop_path_write:
  StrCmp $2 $1 githop_path_end
  ${If} $2 == ""
    DeleteRegValue HKCU "Environment" "Path"
  ${Else}
    WriteRegExpandStr HKCU "Environment" "Path" "$2"
  ${EndIf}
  ; Tell Explorer and open apps that the environment changed.
  SendMessage ${GITHOP_HWND_BROADCAST} ${GITHOP_WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000

  githop_path_end:
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
  Pop $9
FunctionEnd
!macroend

!insertmacro GITHOP_USER_PATH_FUNC ""
!insertmacro GITHOP_USER_PATH_FUNC "un."

!macro NSIS_HOOK_POSTINSTALL
  Push "add"
  Call GitHopUserPath
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} ${FileExists} "$INSTDIR\githop.exe"
      DetailPrint "Restoring your git settings and removing GitHop's hooks..."
      ${If} $DeleteAppDataCheckboxState = 1
        nsExec::ExecToLog '"$INSTDIR\githop.exe" wipe --from-uninstaller'
      ${Else}
        nsExec::ExecToLog '"$INSTDIR\githop.exe" wipe --from-uninstaller --keep-data'
      ${EndIf}
      Pop $0
      ${If} $0 != 0
        DetailPrint "GitHop couldn't restore everything (exit $0). Run githop doctor after reinstalling, or restore from %APPDATA%\githop\backups."
      ${EndIf}
    ${EndIf}
    Push "remove"
    Call un.GitHopUserPath
  ${EndIf}
!macroend
