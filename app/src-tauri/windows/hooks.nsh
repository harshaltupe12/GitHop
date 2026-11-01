; GitHop NSIS installer hooks (Phase 6 §5). Included by Tauri's installer.nsi
; at global scope, before Tauri's own defines and variables. So:
;   * Functions here may only use their own names (no ${PRODUCTNAME},
;     $PassiveMode, ... — those are declared later).
;   * The NSIS_HOOK_* macros run inside Tauri's sections and may use them.
; Per-user install (installMode = currentUser): HKCU only, never machine-wide.
;
; Install (NSIS_HOOK_POSTINSTALL):
;   1. If the previous install was in another folder, take that folder off
;      PATH and delete its old program files.
;   2. `githop.exe installer path-add "$INSTDIR"`: PATH is edited by githop.exe,
;      not NSIS strings (no 1024-char limit; type and every entry kept), then
;      WM_SETTINGCHANGE is broadcast (SMTO_ABORTIFHUNG, so it's quick).
;   3. `githop.exe hooks install --yes --if-installed`: hooks embed the exe
;      path, so rewrite them for this copy when they were on.
;   4. Silent/passive installs don't add a new desktop shortcut (Start menu only).
;
; Uninstall (NSIS_HOOK_PREUNINSTALL):
;   Skipped when this is part of an update or reinstall: /UPDATE, or the
;   GITHOP_NSIS_REINSTALL variable that the installer sets before its
;   "already installed" page may run the old uninstaller (passive /P, GUI
;   upgrade or reinstall). Otherwise:
;   1. Close GitHopApp.exe first, so it can't undo the wipe.
;   2. `githop.exe wipe --from-uninstaller` (restores git config, removes
;      hooks, keeps SSH keys). Adds --keep-data unless the user ticked "Delete
;      my GitHop data" or passed /DELETEAPPDATA (silent and passive uninstalls
;      keep data by default).
;   3. `githop.exe installer path-remove "$INSTDIR"`, then broadcast.
; POSTUNINSTALL removes Tauri's HKCU\Software\GitHop\GitHop (old install folder).

!define /ifndef GITHOP_REINSTALL_ENV "GITHOP_NSIS_REINSTALL"

Var GitHopOldInstDir
Var GitHopHadDesktopLnk
Var GitHopKeepSetup

; ---------------------------------------------------------------------------
; Text (Design §9: sentence case, plain words). Predefined MUI language
; strings win over the MUI language file (LANGFILE_SETSTRING is !ifndef).
; Tauri's own strings are in windows/English.nsh (customLanguageFiles).
; ---------------------------------------------------------------------------
!define MUI_TEXT_WELCOME_INFO_TITLE "Welcome to GitHop setup"
!define MUI_TEXT_WELCOME_INFO_TEXT "This installs GitHop for your Windows user. You don't need admin rights.$\r$\n$\r$\nGitHop works offline. It never connects to the internet.$\r$\n$\r$\nClick Next to continue."
!define MUI_UNTEXT_WELCOME_INFO_TITLE "Welcome to GitHop uninstall"
!define MUI_UNTEXT_WELCOME_INFO_TEXT "This removes GitHop and restores your git settings.$\r$\n$\r$\nClick Next to continue."
!define MUI_TEXT_LICENSE_TITLE "License agreement"
!define MUI_UNTEXT_LICENSE_TITLE "License agreement"
!define MUI_TEXT_COMPONENTS_TITLE "Choose components"
!define MUI_UNTEXT_COMPONENTS_TITLE "Choose components"
!define MUI_TEXT_DIRECTORY_TITLE "Choose install location"
!define MUI_TEXT_DIRECTORY_SUBTITLE "Choose the folder to install GitHop in."
!define MUI_UNTEXT_DIRECTORY_TITLE "Choose uninstall location"
!define MUI_TEXT_INSTALLING_SUBTITLE "Please wait while GitHop is installed."
!define MUI_TEXT_FINISH_TITLE "Installation complete"
!define MUI_TEXT_FINISH_SUBTITLE "GitHop is installed."
!define MUI_TEXT_ABORT_TITLE "Installation stopped"
!define MUI_TEXT_ABORT_SUBTITLE "Setup didn't finish."
!define MUI_UNTEXT_UNINSTALLING_SUBTITLE "Please wait while GitHop is removed."
!define MUI_UNTEXT_FINISH_TITLE "Uninstall complete"
!define MUI_UNTEXT_FINISH_SUBTITLE "GitHop was removed."
!define MUI_UNTEXT_ABORT_TITLE "Uninstall stopped"
!define MUI_UNTEXT_ABORT_SUBTITLE "Uninstall didn't finish."
!define MUI_TEXT_FINISH_INFO_TITLE "GitHop is installed"
!define MUI_TEXT_FINISH_INFO_TEXT "Open GitHop from the Start menu to set up your profiles.$\r$\n$\r$\nTo use githop in a terminal, open a new terminal window.$\r$\n$\r$\nClick Finish to close setup."
!define MUI_UNTEXT_FINISH_INFO_TITLE "GitHop is uninstalled"
!define MUI_UNTEXT_FINISH_INFO_TEXT "GitHop was removed and your git settings were restored.$\r$\n$\r$\nClick Finish to close."
!define MUI_TEXT_FINISH_RUN "&Open GitHop"
!define MUI_TEXT_STARTMENU_TITLE "Choose Start menu folder"
!define MUI_TEXT_STARTMENU_SUBTITLE "Choose a Start menu folder for the GitHop shortcuts."
!define MUI_UNTEXT_CONFIRM_TITLE "Uninstall GitHop"
!define MUI_UNTEXT_CONFIRM_SUBTITLE "Remove GitHop from your computer."
!define MUI_TEXT_ABORTWARNING "Are you sure you want to quit GitHop setup?"
!define MUI_UNTEXT_ABORTWARNING "Are you sure you want to quit GitHop uninstall?"
!define MULTIUSER_TEXT_INSTALLMODE_TITLE "Choose users"
!define LANGDLL_WINDOWTITLE "Installer language"
; Page-specific (consumed by the matching page macro).
!define MUI_DIRECTORYPAGE_TEXT_DESTINATION "Install folder"
!define MUI_UNCONFIRMPAGE_TEXT_TOP "GitHop will restore your git settings and remove its hooks. Your SSH keys are kept."

; ---------------------------------------------------------------------------
; Reinstall detection (F1). .onGUIInit runs before Tauri's "already
; installed" page, in GUI and passive (/P) mode (not in /S, which never runs
; the old uninstaller). That page may run the old uninstall.exe, which
; inherits this variable and then keeps the user's setup.
; ---------------------------------------------------------------------------
!define MUI_CUSTOMFUNCTION_GUIINIT GitHopGuiInit
Function GitHopGuiInit
  System::Call 'kernel32::SetEnvironmentVariable(t "${GITHOP_REINSTALL_ENV}", t "1") i .r0'
FunctionEnd

; Tell Explorer and open apps that the environment changed. SMTO_ABORTIFHUNG
; with a 1 s timeout: a hung window can't stall setup (F3).
!macro GITHOP_BROADCAST_FUNC UN
Function ${UN}GitHopBroadcastEnv
  Push $0
  System::Call 'user32::SendMessageTimeout(p 0xffff, i 0x1A, p 0, t "Environment", i 0x2, i 1000, *p .r0)'
  Pop $0
FunctionEnd
!macroend
!insertmacro GITHOP_BROADCAST_FUNC ""
!insertmacro GITHOP_BROADCAST_FUNC "un."

!macro NSIS_HOOK_PREINSTALL
  ; Previous install folder, read before Tauri overwrites it (F4).
  ReadRegStr $GitHopOldInstDir SHCTX "${MANUPRODUCTKEY}" ""
  StrCpy $GitHopHadDesktopLnk 0
  ${If} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
    StrCpy $GitHopHadDesktopLnk 1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} ${FileExists} "$INSTDIR\githop.exe"
    ; F4: installed to a different folder than last time.
    ${If} $GitHopOldInstDir != ""
    ${AndIf} $GitHopOldInstDir != $INSTDIR
    ${AndIf} $GitHopOldInstDir != "$INSTDIR\"
      GetFullPathName /SHORT $1 "$GitHopOldInstDir"
      GetFullPathName /SHORT $2 "$INSTDIR"
      ${If} $1 == ""
      ${OrIf} $1 != $2
        DetailPrint "Taking the old GitHop folder off your PATH: $GitHopOldInstDir"
        nsExec::ExecToLog '"$INSTDIR\githop.exe" installer path-remove "$GitHopOldInstDir"'
        Pop $0
        ${If} $1 != ""
        ${AndIf} ${FileExists} "$GitHopOldInstDir\uninstall.exe"
          Delete "$GitHopOldInstDir\${MAINBINARYNAME}.exe"
          Delete "$GitHopOldInstDir\githop.exe"
          Delete "$GitHopOldInstDir\uninstall.exe"
          RMDir "$GitHopOldInstDir"
        ${EndIf}
      ${EndIf}
    ${EndIf}

    ; F2: githop.exe edits PATH (any length; type and other entries kept).
    DetailPrint "Adding GitHop to your PATH..."
    nsExec::ExecToLog '"$INSTDIR\githop.exe" installer path-add "$INSTDIR"'
    Pop $0
    ${If} $0 == 0
      Call GitHopBroadcastEnv
    ${Else}
      DetailPrint "Your PATH wasn't changed. To use githop in terminals, add $INSTDIR to your PATH by hand."
    ${EndIf}

    ; F1/F4: hooks call githop.exe by its full path. If they were on, point
    ; them at this copy (does nothing on a new PC).
    nsExec::ExecToLog '"$INSTDIR\githop.exe" hooks install --yes --if-installed'
    Pop $0
    ${If} $0 != 0
      DetailPrint "GitHop couldn't update its hooks (exit $0). Open a new terminal and run: githop hooks install"
    ${EndIf}
  ${Else}
    DetailPrint "githop.exe is missing from $INSTDIR, so PATH and hooks weren't set up. Run setup again."
  ${EndIf}

  ; N4: silent and passive installs: Start menu shortcut only.
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
    ${If} $GitHopHadDesktopLnk = 0
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $GitHopKeepSetup 0
  ${If} $UpdateMode = 1
    StrCpy $GitHopKeepSetup 1
  ${EndIf}
  ReadEnvStr $0 "${GITHOP_REINSTALL_ENV}"
  ${If} $0 == "1"
    StrCpy $GitHopKeepSetup 1
  ${EndIf}

  ${If} $GitHopKeepSetup = 1
    DetailPrint "Updating GitHop. Your profiles, hooks and git settings are kept."
  ${Else}
    ; F7: close the app before touching its setup, so it can't re-install
    ; the hooks, and so cancelling here changes nothing. The tray app hides
    ; instead of quitting, so Restart Manager (inside CheckIfAppIsRunning) can't
    ; close it and the uninstall would abort: end it (and its WebView2
    ; children) first. Fixed image name, no user input on the command line.
    nsExec::Exec '"$SYSDIR\taskkill.exe" /F /T /IM "${MAINBINARYNAME}.exe"'
    Pop $0
    ${If} $0 = 0
      Sleep 1500
    ${EndIf}
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

    ; F5: silent/passive uninstalls keep data unless /DELETEAPPDATA is given.
    ${GetOptions} $CMDLINE "/DELETEAPPDATA" $0
    ${IfNot} ${Errors}
      StrCpy $DeleteAppDataCheckboxState 1
    ${EndIf}

    ${If} ${FileExists} "$INSTDIR\githop.exe"
      DetailPrint "Restoring your git settings and removing GitHop's hooks..."
      ${If} $DeleteAppDataCheckboxState = 1
        nsExec::ExecToLog '"$INSTDIR\githop.exe" wipe --from-uninstaller'
      ${Else}
        nsExec::ExecToLog '"$INSTDIR\githop.exe" wipe --from-uninstaller --keep-data'
      ${EndIf}
      Pop $0
      ${If} $0 != 0
        DetailPrint "GitHop couldn't restore everything (exit $0). Download githop.exe and run: githop wipe --yes. Backups are in %APPDATA%\githop\backups."
      ${EndIf}
      nsExec::ExecToLog '"$INSTDIR\githop.exe" installer path-remove "$INSTDIR"'
      Pop $0
      ${If} $0 == 0
        Call un.GitHopBroadcastEnv
      ${Else}
        DetailPrint "If $INSTDIR is on your PATH, remove it by hand."
      ${EndIf}
    ${Else}
      DetailPrint "githop.exe is missing, so your git settings weren't restored. To restore them, download githop.exe and run: githop wipe --yes"
      DetailPrint "If $INSTDIR is on your PATH, remove it by hand."
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; L1: Tauri keeps the old install folder here unless app data is deleted.
  ${If} $GitHopKeepSetup <> 1
    DeleteRegKey HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
    ; WebView2 browser cache the app creates next to its exe (no user data).
    RMDir /r "$INSTDIR\${MAINBINARYNAME}.exe.WebView2"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
