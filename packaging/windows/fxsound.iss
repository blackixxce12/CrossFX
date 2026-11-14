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
; The AppUserModelID FxSound's notifications are shown under, which it registers under
; HKCU\Software\Classes\AppUserModelId before the first one, and their icon, which it writes into
; a folder of its own in %LOCALAPPDATA% (crates/fxsound-app/src/win/toast.rs).
#define AppUserModelId "FxSoundPort.FxSound"
#define NotificationIcon "notification.png"

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
; It is not restarted afterwards: the last page offers to start the new one. This is Setup's only;
; the uninstaller closes it in CurUninstallStepChanged below.
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
english.CloseRunning=FxSound Port is running and will be closed so that it can be uninstalled. Like Exit in its menu, it first gives the default playback device back.%n%nClick OK to go on, or Cancel to leave it running and not uninstall it.
russian.CloseRunning=Программа FxSound Port запущена. Для удаления она будет закрыта — так же, как по команде «Закрыть» в её меню: сначала она вернёт прежнее устройство воспроизведения по умолчанию.%n%nНажмите «OK», чтобы продолжить, или «Отмена», чтобы оставить её работать и не удалять.

[InstallDelete]
; What an earlier version installed and this one may not ship: a preset dropped or renamed since,
; a licence of something no longer built in. Setup installs over the old version instead of
; uninstalling it first, and the stores list every .fac and .toml in these folders, so a file left
; over would be a factory preset for good — or, renamed with the same name inside, the one listed
; in place of the new copy. Cleared first, the folder holds what this version ships, as after an
; upgrade of the .msi. Only the package's own kinds of file in the package's own folders: the
; folder is always Program Files\FxSound Port (PrepareToInstall), and user presets are kept under
; %APPDATA%, never here. The uninstaller and its log are left alone.
Type: files; Name: "{app}\presets\Factsoft\*.fac"
Type: files; Name: "{app}\presets\BonusPresets\*.fac"
Type: files; Name: "{app}\presets\Input\*.toml"
Type: files; Name: "{app}\COPYING.*"
Type: files; Name: "{app}\OFL.*"

[Files]
; The staged tree as it is: the files and folders build-windows.ps1 put there are the package.
; uninsrestartdelete is for a copy of FxSound Port the uninstaller could not close (in another
; user's session, say): the files it holds go when Windows restarts instead of staying for good.
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs uninsrestartdelete

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
  ErrorMoreData = 234;

function MsiEnumRelatedProducts(UpgradeCode: String; Reserved: Cardinal; Index: Cardinal;
  ProductCode: String): Cardinal;
  external 'MsiEnumRelatedProductsW@msi.dll stdcall delayload setuponly';

// The Restart Manager, for the uninstaller (CloseRunningProgram). FileNames is the API's array of
// string pointers, which is how Pascal Script passes a TArrayOfString; Applications, ServiceNames,
// AffectedApps and StatusCallback are pointers passed as null.
function RmStartSession(var SessionHandle: Cardinal; SessionFlags: Cardinal;
  SessionKey: String): Cardinal;
  external 'RmStartSession@rstrtmgr.dll stdcall delayload uninstallonly';
function RmRegisterResources(SessionHandle: Cardinal; FileCount: Cardinal;
  FileNames: TArrayOfString; ApplicationCount: Cardinal; Applications: Cardinal;
  ServiceCount: Cardinal; ServiceNames: Cardinal): Cardinal;
  external 'RmRegisterResources@rstrtmgr.dll stdcall delayload uninstallonly';
function RmGetList(SessionHandle: Cardinal; var ProcInfoNeeded: Cardinal; var ProcInfo: Cardinal;
  AffectedApps: Cardinal; var RebootReasons: Cardinal): Cardinal;
  external 'RmGetList@rstrtmgr.dll stdcall delayload uninstallonly';
function RmShutdown(SessionHandle: Cardinal; ActionFlags: Cardinal;
  StatusCallback: Cardinal): Cardinal;
  external 'RmShutdown@rstrtmgr.dll stdcall delayload uninstallonly';
function RmEndSession(SessionHandle: Cardinal): Cardinal;
  external 'RmEndSession@rstrtmgr.dll stdcall delayload uninstallonly';

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

// A running FxSound Port has fxsound.exe loaded, which cannot be deleted, and it would go on
// playing with its presets deleted from under it: it is closed before anything is removed. With
// the Restart Manager, as Setup's CloseApplications closes it for an upgrade and Windows Installer
// for the .msi: the program is asked to end as at log-off, which FxSound Port answers as Exit in
// its menu does, giving the default playback device back first. The Restart Manager finds it by
// the file, never by the name: FxSound LLC's FxSound.exe, which may be running too, is another
// file. Not forced: a copy that does not end, as one in another user's session cannot be made to,
// keeps its files, which then go when Windows restarts (uninsrestartdelete).
procedure CloseRunningProgram;
var
  Session, Needed, Count, Reasons, Status: Cardinal;
  Files: TArrayOfString;
begin
  // The session key and its terminating null: 33 characters.
  if RmStartSession(Session, 0, StringOfChar(' ', 33)) <> ErrorSuccess then
  begin
    Log('The Restart Manager did not start; a running FxSound Port is not closed');
    Exit;
  end;
  try
    SetArrayLength(Files, 1);
    Files[0] := ExpandConstant('{app}\{#AppExeName}');
    Needed := 0;
    Count := 0;
    Reasons := 0;
    // Asked with no room for the list, which is then "more data" when anything uses the file.
    if (RmRegisterResources(Session, 1, Files, 0, 0, 0, 0) <> ErrorSuccess) or
      (RmGetList(Session, Needed, Count, 0, Reasons) <> ErrorMoreData) then
      Exit;
    if not UninstallSilent and
      (MsgBox(CustomMessage('CloseRunning'), mbConfirmation, MB_OKCANCEL) <> IDOK) then
      Abort;
    Status := RmShutdown(Session, 0, 0);
    if Status = ErrorSuccess then
      Log('FxSound Port was running and is closed')
    else
      Log(Format('FxSound Port did not close (Restart Manager error %d); ' +
        'what it holds goes when Windows restarts', [Status]));
  finally
    RmEndSession(Session);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  case CurUninstallStep of
    // After the user has confirmed the uninstall and before anything is removed; Cancel on the
    // question leaves everything as it was.
    usUninstall:
      CloseRunningProgram;
    // What FxSound Port writes for itself, not the setup, so the uninstaller does not know of it:
    // the "Launch on system startup" Run value, which left behind would start a program that is no
    // longer there at every log-in, and the registration and icon of its notifications, which
    // would keep a sender of that name in Windows' notification settings. Only a real uninstall
    // gets here: an upgrade installs over the old version without uninstalling it, and the
    // setting survives. HKCU and %LOCALAPPDATA% are the uninstalling user's, which is the user who
    // ran FxSound Port unless an administrator's account was used to elevate.
    usPostUninstall:
      begin
        RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run',
          '{#AutostartValue}');
        RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER,
          'Software\Classes\AppUserModelId\{#AppUserModelId}');
        DeleteFile(ExpandConstant('{localappdata}\{#AppName}\{#NotificationIcon}'));
        // Only when that left it empty.
        RemoveDir(ExpandConstant('{localappdata}\{#AppName}'));
      end;
  end;
end;
