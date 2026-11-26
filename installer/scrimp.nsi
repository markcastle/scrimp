; Scrimp's installer: per user, no administrator prompt.
;
; Built by scripts/build-installer.sh, the same way in CI and in releases:
;
;   makensis -DVERSION=0.2.0 -DSTAGE=<folder with the three files> -DOUT=<setup.exe> installer/scrimp.nsi
;
; Installs to %LOCALAPPDATA%\Programs\Scrimp:
;
;   scrimp.exe, scrimpw.exe, uninstall.exe
;   shell\<version>\scrimp_shell.dll   one folder per version, because Explorer keeps the old one loaded
;
; The menu and the PATH entry are added by `scrimpw.exe --register --add-to-path`,
; so those rules live in tested Rust code, never in NSIS's string handling
; (which would cut a long PATH at 1,024 characters).
;
; `/S` installs silently. `/WAITPID=<id>` waits for that process to exit
; first, which is how `scrimp update` hands over.

Unicode true
ManifestDPIAware true

!ifndef VERSION
  !error "Pass -DVERSION=major.minor.patch"
!endif
!ifndef STAGE
  !error "Pass -DSTAGE=<folder holding scrimp.exe, scrimpw.exe and scrimp_shell.dll>"
!endif
!ifndef OUT
  !define OUT "Scrimp-windows-x64-setup.exe"
!endif

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Scrimp"

Name "Scrimp"
OutFile "${OUT}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Scrimp"
SetCompressor /SOLID lzma
BrandingText "Scrimp ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Scrimp"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Scrimp installer"
VIAddVersionKey "LegalCopyright" "MIT licence"

!define MUI_WELCOMEPAGE_TITLE "Install Scrimp ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "Scrimp saves disk space using the compression built into Windows. Your files stay exactly as they are, just smaller on disk, and apps open them as normal.$\r$\n$\r$\nIt adds Scrimp Compress and Scrimp Decompress to Explorer's right-click menu, and the scrimp command to new terminal windows.$\r$\n$\r$\nNo administrator is needed."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "Scrimp is installed"
!define MUI_FINISHPAGE_TEXT "Right-click a file or folder and choose Scrimp Compress. On Windows 11, it's under Show more options.$\r$\n$\r$\nTo remove Scrimp, use Installed apps in Settings."
!insertmacro MUI_PAGE_FINISH

!define MUI_UNCONFIRMPAGE_TEXT_TOP "This removes Scrimp from this computer. Files Scrimp compressed stay compressed and keep working as normal. To make them full size again, use Scrimp Decompress before uninstalling."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ; `scrimp update` passes its own process ID, so its files are free to replace.
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/WAITPID=" $R1
  ${IfNot} ${Errors}
    System::Call 'kernel32::OpenProcess(i 0x00100000, i 0, i $R1) p .R2'
    ${If} $R2 != 0
      System::Call 'kernel32::WaitForSingleObject(p $R2, i 60000)'
      System::Call 'kernel32::CloseHandle(p $R2)'
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Scrimp"
  SetOutPath "$INSTDIR"
  File "${STAGE}\scrimp.exe"
  File "${STAGE}\scrimpw.exe"
  SetOutPath "$INSTDIR\shell\${VERSION}"
  File "${STAGE}\scrimp_shell.dll"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ExecWait '"$INSTDIR\scrimpw.exe" --register --add-to-path --quiet' $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION "Scrimp was installed, but couldn't add itself to Explorer's menu. Running the installer again may help." /SD IDOK
    SetErrorLevel 5
  ${EndIf}

  ; So Scrimp shows in Installed apps, with its version and an uninstaller.
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Scrimp"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Mark Castle"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\scrimpw.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/markcastle/scrimp"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $1 $2 $3
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $1
SectionEnd

Section "Uninstall"
  ExecWait '"$INSTDIR\scrimpw.exe" --unregister --quiet'
  Delete "$INSTDIR\scrimp.exe"
  Delete "$INSTDIR\scrimpw.exe"
  ; Explorer may still have a menu DLL loaded; Windows removes it at the next restart.
  RMDir /r /REBOOTOK "$INSTDIR\shell"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /REBOOTOK "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Settings in %LOCALAPPDATA%\Scrimp are the person's own, so they stay.
SectionEnd
