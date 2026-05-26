; LC-131: Windows installer for lets-chat-desktop, compiled FROM LINUX with
; makensis (the `nsis` package ships a native Linux makensis - no Windows host,
; no wine, none of the tauri-cli host-gating that blocked the MSI path).
;
; Build (in CI, over the cross-built mingw binary):
;   makensis -DAPP_EXE=/path/to/lets-chat-desktop.exe \
;            -DAPP_VERSION=1.2.3 \
;            -DOUT_FILE=lets-chat-desktop-setup-x86_64.exe \
;            desktop/installer/lets-chat.nsi
;
; Every -D has a default so the script also compiles standalone for the spike.

;--------------------------------- parameters
!ifndef APP_NAME
  !define APP_NAME "lets-chat"
!endif
!ifndef APP_VERSION
  !define APP_VERSION "0.0.0-dev"
!endif
!ifndef APP_PUBLISHER
  !define APP_PUBLISHER "a8n-tools"
!endif
; Source path of the cross-built desktop binary. CI overrides this; the spike
; default points at a stub so the script compiles without the real build.
!ifndef APP_EXE
  !define APP_EXE "stub-lets-chat-desktop.exe"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "lets-chat-desktop-setup-x86_64.exe"
!endif
; The installed binary's filename (what the shortcut + ARP point at).
!define APP_EXE_NAME "lets-chat-desktop.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"

;--------------------------------- installer metadata
Name "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_FILE}"
Unicode true
; Per-machine install under Program Files needs elevation. (A per-user variant
; - InstallDir $LOCALAPPDATA, RequestExecutionLevel user, no admin - is the
; obvious follow-up for unprivileged installs.)
RequestExecutionLevel admin
InstallDir "$PROGRAMFILES64\${APP_NAME}"
InstallDirRegKey HKLM "Software\${APP_NAME}" "InstallDir"

;--------------------------------- UI (MUI2 ships with the nsis package)
!include "MUI2.nsh"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
; Offer to launch on finish.
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE_NAME}"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

;--------------------------------- install
Section "Install"
  SetOutPath "$INSTDIR"
  File "/oname=${APP_EXE_NAME}" "${APP_EXE}"

  ; WebView2 runtime. Tauri's Windows webview requires the Evergreen runtime.
  ; Detect via the EdgeUpdate client key (per-machine, then per-user).
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" 0 webview2_ok
  ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" 0 webview2_ok
!ifdef WEBVIEW2_BOOTSTRAPPER
  ; Absent: run the bundled Evergreen bootstrapper (it fetches + installs the
  ; runtime online). Staged into the temp $PLUGINSDIR (auto-cleaned on exit).
  ; CI bundles it via -DWEBVIEW2_BOOTSTRAPPER; with no define (the standalone
  ; spike build) the script falls through to the warn path below.
  InitPluginsDir
  File "/oname=$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2_BOOTSTRAPPER}"
  DetailPrint "Installing the Microsoft Edge WebView2 Runtime..."
  ExecWait '"$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install' $1
  StrCmp $1 "0" webview2_ok 0
  MessageBox MB_OK|MB_ICONEXCLAMATION "The WebView2 Runtime installer exited with code $1. If ${APP_NAME} shows a blank window, install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703."
  Goto webview2_ok
!else
  MessageBox MB_OK|MB_ICONEXCLAMATION "Microsoft Edge WebView2 Runtime was not detected. ${APP_NAME} needs it to display its UI. Install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703 if the app shows a blank window."
!endif
webview2_ok:

  ; Start Menu shortcut.
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE_NAME}"

  ; Uninstaller + Add/Remove Programs entry.
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

;--------------------------------- optional autostart (unchecked by default)
Section /o "Start ${APP_NAME} at login" SEC_AUTOSTART
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_NAME}" "$\"$INSTDIR\${APP_EXE_NAME}$\""
SectionEnd

;--------------------------------- uninstall
Section "Uninstall"
  Delete "$INSTDIR\${APP_EXE_NAME}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_NAME}"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\${APP_NAME}"
SectionEnd
