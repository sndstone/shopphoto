!include "MUI2.nsh"
Name "ShopPhoto"
!ifndef VERSION
  !define VERSION "0.3.1"
!endif
OutFile "ShopPhoto-${VERSION}-Setup.exe"
InstallDir "$PROGRAMFILES64\ShopPhoto"
RequestExecutionLevel admin
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Section
  SetOutPath "$INSTDIR"
  File "..\..\target\x86_64-pc-windows-msvc\release\shopphoto.exe"
  CreateShortcut "$DESKTOP\ShopPhoto.lnk" "$INSTDIR\shopphoto.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCR ".3fr" "ShopPhoto.Raw"
  WriteRegStr HKCR ".arw" "ShopPhoto.Raw"
  WriteRegStr HKCR ".fff" "ShopPhoto.Raw"
  WriteRegStr HKCR ".dng" "ShopPhoto.Raw"
  WriteRegStr HKCR "ShopPhoto.Raw\shell\open\command" "" '"$INSTDIR\shopphoto.exe" "%1"'
SectionEnd
Section "Uninstall"
  Delete "$DESKTOP\ShopPhoto.lnk"
  Delete "$INSTDIR\shopphoto.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCR "ShopPhoto.Raw"
  DeleteRegValue HKCR ".3fr"
  DeleteRegValue HKCR ".arw"
  DeleteRegValue HKCR ".fff"
  DeleteRegValue HKCR ".dng"
SectionEnd
