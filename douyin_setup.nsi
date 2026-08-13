; DouyinRecorder 安装脚本（NSIS 3.x，复刻原版 douyin-recorder-setup 行为）
; 按用户安装到 %LOCALAPPDATA%\Programs\DouyinRecorder，无需管理员权限。
; 用法（在项目根目录）：
;   makensis.exe douyin_setup.nsi
Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!define APP_NAME "DouyinRecorder"
!define APP_VERSION "3.9"
!define APP_EXE "DouyinRecorder.exe"
!define APP_REG "Software\${APP_NAME}"
!define UNINST_REG "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "douyin-recorder-setup-v3.9.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${APP_REG}" "InstallDir"
VIProductVersion "3.9.0.0"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey "FileVersion" "3.9.0.0"
VIAddVersionKey "ProductVersion" "3.9"

!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 ${APP_NAME}"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

Section "安装" SecMain
  SetOutPath "$INSTDIR"
  File /r "dist_v39\DouyinRecorder\*.*"

  ; 卸载器
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; 快捷方式
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"

  ; 注册表（卸载信息 + 安装目录）
  WriteRegStr HKCU "${APP_REG}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${APP_REG}" "Version" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_REG}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_REG}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_REG}" "Publisher" "DouyinRecorder"
  WriteRegStr HKCU "${UNINST_REG}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_REG}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${UNINST_REG}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD HKCU "${UNINST_REG}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_REG}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR"

  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  DeleteRegKey HKCU "${APP_REG}"
  DeleteRegKey HKCU "${UNINST_REG}"
SectionEnd
