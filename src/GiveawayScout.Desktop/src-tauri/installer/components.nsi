; Based on the NSIS Handlebars template embedded in @tauri-apps/cli 2.12.1.
; Project changes: optional GUI/CLI components, safe handoff, profile integration.
Unicode true
ManifestDPIAware true
; Add in `dpiAwareness` `PerMonitorV2` to manifest for Windows 10 1607+ (note this should not affect lower versions since they should be able to ignore this and pick up `dpiAware` `true` set by `ManifestDPIAware true`)
; Currently undocumented on NSIS's website but is in the Docs folder of source tree, see
; https://github.com/kichik/nsis/blob/5fc0b87b819a9eec006df4967d08e522ddd651c9/Docs/src/attributes.but#L286-L300
; https://github.com/tauri-apps/tauri/pull/10106
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  ; Set the compression algorithm. We default to LZMA.
  SetCompressor /SOLID "{{compression}}"
!endif

; Keep above !include to stay ahead of any plugin command
; see https://github.com/tauri-apps/tauri/pull/15422#discussion_r3289239624
{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include Sections.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
; The packaging overlay permits exactly this one resource.
{{#each resources}}
!define CLIBINARYSRCPATH "{{no-escape @key}}"
{{/each}}
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var ScoutComponents
Var ScoutGuiSelected
Var ScoutCliSelected
Var ScoutProcessSession
Var ScoutWaitPid
Var ScoutWaitHandle
Var ScoutWaitPath
Var ScoutWaitSize
Var ScoutWaitResult

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

; We don't actually use this value as default install path,
; it's just for nsis to append the product name folder in the directory selector
; https://nsis.sourceforge.io/Reference/InstallDir
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

# additional plugins
!addplugindir "${ADDITIONALPLUGINSPATH}"

; Uninstaller signing command
!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel admin
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; Installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; Enable header images for installer and uninstaller pages when either image is configured.
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif

; Installer header image
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
!endif

; Uninstaller header image
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!endif

; Uninstaller icon
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; Installer pages, must be ordered as they appear
; 1. Welcome Page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME

; 2. License Page (if defined)
!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; 3. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif

; 5. Choose install directory page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY

; Optional components; every install must retain at least one.
; Keep full component names visible, with a short description below the list.
!define MUI_COMPONENTSPAGE_SMALLDESC
!define MUI_COMPONENTSPAGE_TEXT_TOP "$(ScoutComponentsText)"
!define MUI_COMPONENTSPAGE_TEXT_COMPLIST "$(ScoutComponentsList)"
!define MUI_COMPONENTSPAGE_TEXT_DESCRIPTION_INFO "$(ScoutComponentsDescriptionInfo)"
!define MUI_PAGE_CUSTOMFUNCTION_PRE ScoutComponentsPre
!define MUI_PAGE_CUSTOMFUNCTION_SHOW ScoutComponentsShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE ScoutComponentsLeave
!insertmacro MUI_PAGE_COMPONENTS

; 6. Start menu shortcut page
Var AppStartMenuFolder
!if "${STARTMENUFOLDER}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !define MUI_STARTMENUPAGE_DEFAULTFOLDER "${STARTMENUFOLDER}"
!else
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
!endif
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

; 7. Installation page
!insertmacro MUI_PAGE_INSTFILES

; 8. Finish page
;
; Don't auto jump to finish page after installation page,
; because the installation page has useful info that can be used debug any issues with the installer.
!define MUI_FINISHPAGE_NOAUTOCLOSE
; Use show readme button in the finish page as a button create a desktop shortcut
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(createDesktop)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateOrUpdateDesktopShortcut
; Show run app after installation.
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!define MUI_PAGE_CUSTOMFUNCTION_SHOW ScoutFinishShow
!insertmacro MUI_PAGE_FINISH

Function RunMainBinary
  ${IfThen} $ScoutGuiSelected != 1 ${|} Return ${|}
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; Uninstaller Pages
; 1. Confirm uninstall page
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
!define /ifndef WS_EX_LAYOUTRTL         0x00400000
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
Function un.ConfirmShow ; Add add a `Delete app data` check box
  ; $1 inner dialog HWND
  ; $2 window DPI
  ; $3 style
  ; $4 x
  ; $5 y
  ; $6 width
  ; $7 height
  FindWindow $1 "#32770" "" $HWNDPARENT ; Find inner dialog
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  ${If} $(^RTL) = 1
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE} | ${WS_EX_LAYOUTRTL}"
    IntOp $4 50 * $2
  ${Else}
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE}"
    IntOp $4 0 * $2
  ${EndIf}
  IntOp $5 100 * $2
  IntOp $6 400 * $2
  IntOp $7 25 * $2
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  IntOp $7 $7 / 96
  System::Call 'user32::CreateWindowEx(i r3, w "${__NSD_CheckBox_CLASS}", w "$(deleteAppData)", i ${__NSD_CheckBox_STYLE}, i r4, i r5, i r6, i r7, p r1, i0, i0, i0) i .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM

; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

Section "-EarlyChecks"
  Call ScoutReadComponents
  Call ScoutValidateComponents
  Call ScoutCheckProcesses
  Call ScoutCheckProfile
  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif
  ; Compare the installed version directly; component maintenance does not run
  ; an old uninstaller or depend on a reinstall page populating scratch registers.
  !if "${ALLOWDOWNGRADES}" == "false"
  ReadRegStr $1 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${If} $1 != ""
    nsis_tauri_utils::SemverCompare "${VERSION}" $1
    Pop $0
    ${If} $0 = -1
      Push "$(silentDowngrades)"
      Call ScoutFail
    ${EndIf}
  ${EndIf}
  !endif

SectionEnd

Section "$(ScoutGuiComponent)" SEC_GUI
  SetOutPath $INSTDIR
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ; Webview2 installation
    ;
    ; Skip if updating
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif

      Goto webview2_done

      install_webview2:
        DetailPrint "$(installingWebview2)"
        ; $6 holds the path to the webview2 installer
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ; Chromium updater docs: https://source.chromium.org/chromium/chromium/src/+/main:docs/updater/user_manual.md
            ; Modified from "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView\ModifyPath"
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}

  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Create file associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  ; Register deep links
  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

SectionEnd

Section /o "$(ScoutCliComponent)" SEC_CLI
  SetOutPath $INSTDIR
  File /oname=giveaway-scout-cli.exe "${CLIBINARYSRCPATH}"
SectionEnd

; MUI invokes this callback after changing the text and its enabled state.
!define MUI_CUSTOMFUNCTION_ONMOUSEOVERSECTION ScoutDescriptionColors
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_GUI} "$(ScoutGuiDescription)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLI} "$(ScoutCliDescription)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "-Complete"
  ${If} $ScoutGuiSelected != 1
    Call ScoutRemoveGui
    Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  ${EndIf}
  ${If} $ScoutCliSelected != 1
    Delete "$INSTDIR\giveaway-scout-cli.exe"
  ${EndIf}

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Save $INSTDIR in registry for future installations
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
  !endif

  ; MainBinaryName is diagnostic; component reconciliation owns file deletion.
  ${If} $ScoutGuiSelected = 1
    WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"
  ${Else}
    WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "giveaway-scout-cli.exe"
  ${EndIf}

  ; Registry information for add/remove programs
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  Call ScoutMaintainInstallation
  WriteRegDWORD SHCTX "${UNINSTKEY}" "Components" $ScoutComponents

  ; Create start menu shortcut
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END

  ; Create desktop shortcut for silent and passive installers
  ; because finish page will be skipped
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; Auto close this page for passive mode
  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function .onInstSuccess
  ${IfThen} $ScoutGuiSelected != 1 ${|} Return ${|}
  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  !insertmacro MUI_UNGETLANGUAGE

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd

Section Uninstall

  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  Call un.ScoutCheckProcesses
  Call un.ScoutMaintainInstallation

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  Delete "$INSTDIR\giveaway-scout-cli.exe"

  ; Delete app associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  ; Delete deep links
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}


  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ; Remove shortcuts if not updating
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    ; Remove start menu shortcut
    !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Remove desktop shortcuts
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  ; Delete app data if the checkbox is selected
  ; and if not updating
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Clear the install location $INSTDIR from registry
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"

    ; Clear the install language from registry
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    SetShellVarContext current
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; Auto close if passive mode or updating
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ${IfThen} $ScoutGuiSelected != 1 ${|} Return ${|}
  ${If} $UpdateMode = 1
  ${OrIf} $NoShortcutMode = 1
    Return
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  ${IfThen} $ScoutGuiSelected != 1 ${|} Return ${|}
  ${If} $UpdateMode = 1
  ${OrIf} $NoShortcutMode = 1
    Return
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd

Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT
  !endif

  Call ScoutLoadComponents
  Call ScoutWaitForParent
  Call ScoutCheckProcesses
FunctionEnd

; These handlers follow the section declarations because NSIS section IDs are
; preprocessor definitions, not runtime lookups.
Function ScoutReadComponents
  SectionGetFlags ${SEC_GUI} $0
  IntOp $ScoutGuiSelected $0 & ${SF_SELECTED}
  SectionGetFlags ${SEC_CLI} $0
  IntOp $ScoutCliSelected $0 & ${SF_SELECTED}
  IntOp $ScoutComponents $ScoutCliSelected << 1
  IntOp $ScoutComponents $ScoutComponents | $ScoutGuiSelected
FunctionEnd

Function ScoutValidateComponents
  ${If} $ScoutComponents < 1
  ${OrIf} $ScoutComponents > 3
    Push "$(ScoutChooseComponent)"
    Call ScoutFail
  ${EndIf}
FunctionEnd

Function ScoutLoadComponents
  ; Restore the complete selection before maintenance can remove registry keys.
  ReadRegDWORD $ScoutComponents SHCTX "${UNINSTKEY}" "Components"
  ${If} ${Errors}
  ${OrIf} $ScoutComponents < 1
  ${OrIf} $ScoutComponents > 3
    StrCpy $ScoutComponents 0
    ReadRegStr $0 SHCTX "${UNINSTKEY}" "InstallLocation"
    ${If} $0 != ""
      ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
        IntOp $ScoutComponents $ScoutComponents | 1
      ${EndIf}
      ${If} ${FileExists} "$INSTDIR\giveaway-scout-cli.exe"
        IntOp $ScoutComponents $ScoutComponents | 2
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ${If} $UpdateMode = 1
  ${AndIf} $ScoutComponents = 0
    Push "$(ScoutInvalidComponents)"
    Call ScoutFail
  ${EndIf}
  ${IfThen} $ScoutComponents = 0 ${|} StrCpy $ScoutComponents 1 ${|}

  ClearErrors
  ${GetOptions} $CMDLINE "/COMPONENTS=" $0
  ${IfNot} ${Errors}
    ${If} $UpdateMode = 1
      Push "$(ScoutInvalidComponents)"
      Call ScoutFail
    ${EndIf}
    ${If} $0 == "gui"
      StrCpy $ScoutComponents 1
    ${ElseIf} $0 == "cli"
      StrCpy $ScoutComponents 2
    ${ElseIf} $0 == "gui,cli"
      StrCpy $ScoutComponents 3
    ${Else}
      Push "$(ScoutInvalidComponents)"
      Call ScoutFail
    ${EndIf}
  ${EndIf}
  IntOp $ScoutGuiSelected $ScoutComponents & 1
  IntOp $ScoutCliSelected $ScoutComponents >> 1
  SectionSetFlags ${SEC_GUI} $ScoutGuiSelected
  SectionSetFlags ${SEC_CLI} $ScoutCliSelected
FunctionEnd

Function .onSelChange
  Call ScoutReadComponents
FunctionEnd

Function ScoutComponentsLeave
  Call ScoutReadComponents
  ${If} $ScoutComponents = 0
    MessageBox MB_OK|MB_ICONEXCLAMATION "$(ScoutChooseComponent)"
    Abort
  ${EndIf}
FunctionEnd

Function ScoutComponentsPre
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    Abort
  ${EndIf}
FunctionEnd

Function ScoutComponentsShow
  ; A disabled Static can inherit near-white COLOR_GRAYTEXT from a dark Windows
  ; theme while this NSIS dialog remains light. Keep the hint enabled and muted.
  EnableWindow $mui.ComponentsPage.DescriptionText 1
  SetCtlColors $mui.ComponentsPage.DescriptionText "666666" "F0F0F0"
FunctionEnd

Function ScoutDescriptionColors
  EnableWindow $mui.ComponentsPage.DescriptionText 1
  ${If} $0 == -1
    SetCtlColors $mui.ComponentsPage.DescriptionText "666666" "F0F0F0"
  ${Else}
    SetCtlColors $mui.ComponentsPage.DescriptionText "000000" "F0F0F0"
  ${EndIf}
  System::Call 'user32::InvalidateRect(p $mui.ComponentsPage.DescriptionText, p0, i1)'
FunctionEnd

Function ScoutFinishShow
  ${If} $ScoutGuiSelected != 1
    ShowWindow $mui.FinishPage.Run ${SW_HIDE}
    ShowWindow $mui.FinishPage.ShowReadme ${SW_HIDE}
  ${EndIf}
FunctionEnd

Function ScoutWaitForParent
  ClearErrors
  ${GetOptions} $CMDLINE "/WAITPID=" $ScoutWaitPid
  ${If} ${Errors}
    Return
  ${EndIf}
  ; Never wait on or terminate an arbitrary process supplied on the command line.
  System::Call 'shlwapi::StrToIntExW(w "$ScoutWaitPid", i0, *i .r0)i.r1'
  ${If} $1 = 0
  ${OrIf} $0 <= 0
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
  ; System cleanup can overwrite the thread's last error before another Call.
  ; ?e captures OpenProcess's error immediately and pushes only that error here.
  System::Call 'kernel32::OpenProcess(i0x101000, i0, i r0)p.r1?e'
  Pop $ScoutWaitResult
  StrCpy $ScoutWaitHandle $1
  ${If} $ScoutWaitHandle = 0
    ; ERROR_INVALID_PARAMETER means the parent already exited.
    ${IfThen} $ScoutWaitResult = 87 ${|} Return ${|}
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
  ; A retained process object can already be signaled even though its PID opens.
  System::Call 'kernel32::WaitForSingleObject(p $ScoutWaitHandle, i0)i.r0'
  ${If} $0 = 0
    System::Call 'kernel32::CloseHandle(p $ScoutWaitHandle)'
    Return
  ${ElseIf} $0 != 258
    System::Call 'kernel32::CloseHandle(p $ScoutWaitHandle)'
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
  StrCpy $ScoutWaitSize ${NSIS_MAX_STRLEN}
  System::Call 'kernel32::QueryFullProcessImageNameW(p $ScoutWaitHandle, i0, w .r1, *i $ScoutWaitSize)i.r2'
  StrCpy $ScoutWaitPath $1
  ${If} $2 = 0
    ; The parent may exit between the zero-time wait and the image query.
    ; Accept only a signaled object on this same handle, never a new PID lookup.
    System::Call 'kernel32::WaitForSingleObject(p $ScoutWaitHandle, i0)i.r0'
    StrCpy $ScoutWaitResult $0
    System::Call 'kernel32::CloseHandle(p $ScoutWaitHandle)'
    ${IfThen} $ScoutWaitResult = 0 ${|} Return ${|}
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
  ${If} $ScoutWaitPath != "$INSTDIR\${MAINBINARYNAME}.exe"
  ${AndIf} $ScoutWaitPath != "$INSTDIR\giveaway-scout-cli.exe"
    System::Call 'kernel32::CloseHandle(p $ScoutWaitHandle)'
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
  System::Call 'kernel32::WaitForSingleObject(p $ScoutWaitHandle, i30000)i.r0'
  StrCpy $ScoutWaitResult $0
  System::Call 'kernel32::CloseHandle(p $ScoutWaitHandle)'
  ${If} $ScoutWaitResult != 0
    Push "$(ScoutInvalidParent)"
    Call ScoutFail
  ${EndIf}
FunctionEnd

; Both installer and uninstaller use exact-path Restart Manager checks. They
; never call RmShutdown/TerminateProcess; watch must finish its own writes.
!macro ScoutProcessFunctions Prefix
Function ${Prefix}ScoutFail
  Pop $0
  DetailPrint "$0"
  ${If} $PassiveMode != 1
  ${AndIfNot} ${Silent}
    MessageBox MB_OK|MB_ICONSTOP "$0"
  ${EndIf}
  SetErrorLevel 2
  Quit
FunctionEnd

Function ${Prefix}ScoutCheckProcesses
  scout_process_retry:
    !insertmacro RestartManager_StartSession $ScoutProcessSession
    ${If} $ScoutProcessSession == ""
      Push "$(ScoutProcessCheckFailed)"
      Call ${Prefix}ScoutFail
    ${EndIf}
    !insertmacro RestartManager_RegisterFile $ScoutProcessSession "$INSTDIR\${MAINBINARYNAME}.exe"
    ${If} $0 = 0
      !insertmacro RestartManager_RegisterFile $ScoutProcessSession "$INSTDIR\giveaway-scout-cli.exe"
    ${EndIf}
    ${If} $0 = 0
      System::Call 'RSTRTMGR::RmGetList(i $ScoutProcessSession, *i .r1, *i 0, p0, *i .r3)i.r0'
    ${EndIf}
    !insertmacro RestartManager_EndSession $ScoutProcessSession
    ${IfThen} $0 = 0 ${|} Return ${|}
    ${If} $0 != 234
      Push "$(ScoutProcessCheckFailed)"
      Call ${Prefix}ScoutFail
    ${EndIf}
    ${If} $PassiveMode = 1
    ${OrIf} ${Silent}
      Push "$(ScoutRunning)"
      Call ${Prefix}ScoutFail
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(ScoutRunning)" IDRETRY scout_process_retry
    Push "$(ScoutRunning)"
    Call ${Prefix}ScoutFail
FunctionEnd
!macroend
!insertmacro ScoutProcessFunctions ""
!insertmacro ScoutProcessFunctions "un."

Function ScoutMaintainInstallation
  ; Extract a private helper even for GUI-only, without installing the CLI.
  InitPluginsDir
  SetOutPath $PLUGINSDIR
  File /oname=giveaway-scout-maintenance.exe "${CLIBINARYSRCPATH}"
  nsExec::ExecToStack /TIMEOUT=30000 '"$PLUGINSDIR\giveaway-scout-maintenance.exe" __maintenance sync-autostart --components $ScoutComponents --install-dir "$INSTDIR"'
  Pop $0
  Pop $1
  DetailPrint "$1"
  SetOutPath $INSTDIR
  ${If} $0 != "0"
    Push "$(ScoutMaintenanceFailed)"
    Call ScoutFail
  ${EndIf}
FunctionEnd

Function ScoutCheckProfile
  ; A portable/development executable can own the same production profile while
  ; the installed executable paths themselves are idle. Check before file writes.
  InitPluginsDir
  SetOutPath $PLUGINSDIR
  File /oname=giveaway-scout-maintenance.exe "${CLIBINARYSRCPATH}"
  nsExec::ExecToStack /TIMEOUT=30000 '"$PLUGINSDIR\giveaway-scout-maintenance.exe" __maintenance check-profile'
  Pop $0
  Pop $1
  DetailPrint "$1"
  ${If} $0 == "3"
    ${If} $PassiveMode != 1
    ${AndIfNot} ${Silent}
      MessageBox MB_OK|MB_ICONEXCLAMATION "$(ScoutProfileBusy)"
    ${EndIf}
    SetErrorLevel 3
    Quit
  ${ElseIf} $0 != "0"
    Push "$(ScoutMaintenanceFailed)"
    Call ScoutFail
  ${EndIf}
FunctionEnd

Function un.ScoutMaintainInstallation
  ReadRegDWORD $ScoutComponents SHCTX "${UNINSTKEY}" "Components"
  ${If} ${Errors}
  ${OrIf} $ScoutComponents < 1
  ${OrIf} $ScoutComponents > 3
    StrCpy $ScoutComponents 3
  ${EndIf}
  InitPluginsDir
  SetOutPath $PLUGINSDIR
  File /oname=giveaway-scout-maintenance.exe "${CLIBINARYSRCPATH}"
  nsExec::ExecToStack /TIMEOUT=30000 '"$PLUGINSDIR\giveaway-scout-maintenance.exe" __maintenance sync-autostart --components $ScoutComponents --install-dir "$INSTDIR" --remove'
  Pop $0
  Pop $1
  DetailPrint "$1"
  ${If} $0 != "0"
    Push "$(ScoutMaintenanceFailed)"
    Call un.ScoutFail
  ${EndIf}
FunctionEnd

Function ScoutRemoveGui
  ; Remove only integrations whose target still belongs to this installation.
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}
  !insertmacro DeleteAppUserModelId
  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $0
  ${If} $0 = 1
    !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  ${EndIf}
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $0
  ${If} $0 = 1
    !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
    Delete "$DESKTOP\${PRODUCTNAME}.lnk"
  ${EndIf}
FunctionEnd
