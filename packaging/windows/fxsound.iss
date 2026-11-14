; FxSound Port as a setup program (Inno Setup 6), in English and Russian.
;
; Built by packaging\windows\build-windows.ps1, never by hand: the script stages the files and
; passes what changes between builds on ISCC's command line, so that this file names no version
; and no path of the build machine:
;
;   /DAppVersion=<v>          Cargo.toml's [workspace.package] version, as the program reports it
;   /DAppVersionNumeric=<v>   the same as four numbers, for the setup's own version information
;   /DStageDir=<dir>          the staged tree: fxsound.exe, README.txt, the licences, presets\
;   /DIconFile=<file>         assets\windows\fxsound.ico
;   /DOutputDir=<dir> /DOutputBaseFilename=<name>
;
; Its identity is its own, never the original's: FxSound LLC's FxSound may be installed and
; running beside it. Another AppId, name, publisher, folder and Start menu entry, and no driver:
; the virtual playback device the port plays through belongs to the original FxSound or to
; VB-CABLE, and the last page says so instead of installing one.

#if Ver < EncodeVer(6, 3, 0)
  #error Inno Setup 6.3 or later is needed (x64compatible)
#endif
#ifndef AppVersion
  #error AppVersion is not defined: build with packaging\windows\build-windows.ps1
#endif
#ifndef AppVersionNumeric
  #error AppVersionNumeric is not defined: build with packaging\windows\build-windows.ps1
#endif
#ifndef StageDir
  #error StageDir is not defined: build with packaging\windows\build-windows.ps1
#endif
#ifndef IconFile
  #error IconFile is not defined: build with packaging\windows\build-windows.ps1
#endif
#ifndef OutputDir
  #error OutputDir is not defined: build with packaging\windows\build-windows.ps1
#endif
#ifndef OutputBaseFilename
  #error OutputBaseFilename is not defined: build with packaging\windows\build-windows.ps1
#endif

#define AppName "FxSound Port"
#define AppPublisher "FxSound Linux port contributors"
#define AppURL "https://github.com/blackixxce12/fxsound-linux"
#define AppExeName "fxsound.exe"
; The value FxSound's "Launch on system startup" writes under
; HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
#define AutostartValue "FxSound Port"

[Setup]
; Fixed for good: it is how a later setup finds this installation to replace it, and how
; Apps and Features keys it (the double brace is a literal one).
AppId={{167B19BE-A2D2-4EF0-B5EB-056A5A09480B}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
VersionInfoVersion={#AppVersionNumeric}
VersionInfoProductVersion={#AppVersionNumeric}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoCompany={#AppPublisher}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCopyright=Copyright (C) FxSound Linux port contributors, portions (C) FxSound LLC. GNU AGPL v3 or later.
; Per-machine, like the .msi: one copy for every user, under Program Files, and the uninstaller
; needs the same rights.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
DefaultDirName={autopf}\{#AppName}
; Program Files and nowhere else, so no page to choose another folder on, and PrepareToInstall
; below refuses one given as /DIR=.
DisableDirPage=yes
; The one shortcut goes straight into the Start menu's program list, not into a folder of its own.
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}
LicenseFile={#StageDir}\LICENSE
SetupIconFile={#IconFile}
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
Compression=lzma2/max
SolidCompression=yes
; A running FxSound holds fxsound.exe open; offer to close it rather than fail or ask for a reboot.
; It is not restarted afterwards: the last page offers to start the new one.
CloseApplications=yes
RestartApplications=no
; The language Windows displays in, when it is one of the two, without asking.
ShowLanguageDialog=auto

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Messages]
; The last page, with the one sentence about the device FxSound plays through after the standard
; text, so that the layout makes room for it above the "Launch" box. NoIcons is the same page
; after a /NOICONS install.
english.FinishedLabel=Setup has finished installing [name] on your computer. The application may be launched by selecting the installed shortcut.%n%nFxSound Port needs a virtual playback device, which it does not install: the FxSound Audio Enhancer driver that comes with the original FxSound, or VB-CABLE.
english.FinishedLabelNoIcons=Setup has finished installing [name] on your computer.%n%nFxSound Port needs a virtual playback device, which it does not install: the FxSound Audio Enhancer driver that comes with the original FxSound, or VB-CABLE.
russian.FinishedLabel=Программа [name] установлена на ваш компьютер. Приложение можно запустить с помощью ярлыка в меню «Пуск».%n%nДля работы FxSound Port нужно виртуальное устройство воспроизведения, которое программа не устанавливает сама: драйвер FxSound Audio Enhancer из оригинального FxSound или VB-CABLE.
russian.FinishedLabelNoIcons=Программа [name] установлена на ваш компьютер.%n%nДля работы FxSound Port нужно виртуальное устройство воспроизведения, которое программа не устанавливает сама: драйвер FxSound Audio Enhancer из оригинального FxSound или VB-CABLE.

[CustomMessages]
english.InstalledFromMsi=FxSound Port is already installed from its .msi package. Uninstall it from the list of installed apps first, or install this version from the .msi as well.
russian.InstalledFromMsi=FxSound Port уже установлен из пакета .msi. Сначала удалите его из списка установленных приложений или установите и эту версию из пакета .msi.
english.OnlyProgramFiles=FxSound Port can only be installed in %1. Every account on this computer runs it from its folder, and a folder elsewhere may be one that other accounts can change.
russian.OnlyProgramFiles=FxSound Port можно установить только в папку %1. Все учётные записи этого компьютера запускают программу из её папки, а папку в другом месте могут изменять и другие учётные записи.

[Files]
; The staged tree as it is: the files and folders build-windows.ps1 put there are the package.
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Comment: "System-wide audio enhancement"

[Run]
; As the user who started the setup, not as the administrator the setup runs as.
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Code]
// The .msi installs the same program into the same folder under an entry of its own. Over it,
// this setup would leave two entries in the list of installed apps, each uninstalling the files
// the other one expects, so it stops instead; the .msi refuses the other way round. The .msi is
// found by its upgrade code (packaging\windows\fxsound.wxs), which every version of it shares.
const
  MsiUpgradeCode = '{B9D37BB7-CC1E-4A59-ABE5-4CEAEF9B6496}';
  ErrorSuccess = 0;

function MsiEnumRelatedProducts(UpgradeCode: String; Reserved: Cardinal; Index: Cardinal;
  ProductCode: String): Cardinal;
  external 'MsiEnumRelatedProductsW@msi.dll stdcall delayload setuponly';

function InitializeSetup(): Boolean;
var
  ProductCode: String;
begin
  Result := True;
  // A product code and its terminating null: 39 characters.
  ProductCode := StringOfChar(' ', 39);
  if MsiEnumRelatedProducts(MsiUpgradeCode, 0, 0, ProductCode) = ErrorSuccess then
  begin
    SuppressibleMsgBox(CustomMessage('InstalledFromMsi'), mbCriticalError, MB_OK, IDOK);
    Result := False;
  end;
end;

// Program Files\FxSound Port and nowhere else. Every account on the computer runs FxSound Port
// from its folder — from the Start menu, and at log-in once "Launch on system startup" is on —
// and an administrator runs the uninstaller in it, elevated, so the folder has to be one only
// administrators can change. Under Program Files it is; a folder elsewhere inherits whatever its
// parent grants, and the root of a drive lets every account create folders and change what is in
// them: C:\FxSound Port would let any account replace fxsound.exe or the uninstaller under the
// others. It also keeps the Run values of the other accounts, which an uninstall cannot reach,
// pointing into a folder no account but an administrator can make again. The page is not shown
// (DisableDirPage) and a /DIR= is refused here, which in a silent install is exit code 7.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Only: String;
begin
  Result := '';
  Only := ExpandConstant('{autopf}\{#AppName}');
  if CompareText(RemoveBackslashUnlessRoot(ExpandConstant('{app}')), Only) <> 0 then
    Result := FmtMessage(CustomMessage('OnlyProgramFiles'), [Only]);
end;

// FxSound's "Launch on system startup" is an HKCU Run value the program writes, not the setup,
// so the uninstaller does not know of it. Left behind, it would start a program that is no
// longer there at every login. Only a real uninstall gets here: an upgrade installs over the old
// version without uninstalling it, and the setting survives. HKCU is the uninstalling user's,
// which is the user who turned the setting on unless an administrator's account was used to
// elevate.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run',
      '{#AutostartValue}');
end;
