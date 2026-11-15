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

; Every file this version ships in the folders CurStepChanged below cleans up after an upgrade, as
; "|COPYING.rnnoise|...|presets\Factsoft\1.fac|...|": read from the staged tree as the script is
; compiled, the tree [Files] packs, so that the list and the package cannot disagree. The four
; folders here and there are the same four.
#define Shipped "|"
#define ShippedIn ""
#define FindHandle
#define FindResult
#sub AddShipped
  #expr Shipped = Shipped + ShippedIn + FindGetFileName(FindHandle) + "|"
#endsub
#sub ListShipped
  #for {FindHandle = FindResult = FindFirst(StageDir + "\" + ShippedIn + "*", 0); FindResult; FindResult = FindNext(FindHandle)} AddShipped
  #if FindHandle
    #expr FindClose(FindHandle)
  #endif
#endsub
#expr ShippedIn = "", ListShipped
#expr ShippedIn = "presets\Factsoft\", ListShipped
#expr ShippedIn = "presets\BonusPresets\", ListShipped
#expr ShippedIn = "presets\Input\", ListShipped

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
; The uninstaller's question before it removes anything (CloseRunningProgram): CloseRunning when
; it is about to close FxSound Port in this session, InUseElsewhere when fxsound.exe is in use
; where it cannot close it, either or both, then GoOn.
english.CloseRunning=FxSound Port is running and will be closed so that it can be uninstalled. Like Exit in its menu, it first gives the default playback device back.
russian.CloseRunning=Программа FxSound Port запущена. Для удаления она будет закрыта — так же, как по команде «Закрыть» в её меню: сначала она вернёт прежнее устройство воспроизведения по умолчанию.
english.InUseElsewhere=FxSound Port's program file is in use where this uninstaller cannot close it, by FxSound Port in another user's session for example, and may stay until Windows restarts.
russian.InUseElsewhere=Файл программы FxSound Port занят там, где программа удаления не может его освободить, — например, копией FxSound Port в сеансе другого пользователя — и может остаться до перезагрузки Windows.
english.GoOn=Click OK to go on, or Cancel to keep FxSound Port installed.
russian.GoOn=Нажмите «OK», чтобы продолжить, или «Отмена», чтобы не удалять FxSound Port.

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
  // The preprocessor's list above (Shipped), apostrophes doubled for Pascal: "70's.fac".
  ShippedFiles = '{#StringChange(Shipped, "'", "''")}';

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

// What an earlier version installed and this one does not ship: a preset dropped or renamed since,
// a licence of something no longer built in. Setup installs over the old version instead of
// uninstalling it first, and the stores list every .fac and .toml in these folders, so a file left
// over would be a factory preset for good — or, renamed with the same name inside, the one listed
// in place of the new copy. Removed, the folder holds what this version ships, as after an upgrade
// of the .msi.
//
// Only once the installation has succeeded (ssPostInstall), and only the files this version does
// not ship. An installation that fails or is cancelled part-way is rolled back, which deletes every
// file it copied that was not there before it started: had the old presets been cleared first, as
// [InstallDelete] clears them, the new copies would count as new and go with the rollback, leaving
// no presets at all. Only the package's own kinds of file in the package's own folders, too: the
// folder is always Program Files\FxSound Port (PrepareToInstall), user presets are kept under
// %APPDATA%, never here, and the uninstaller and its log are left alone. A name is compared
// without regard to case, as NTFS compares it, so that no shipped file is taken for another.
//
// A file is of the kind when its own name matches Mask. FindFirst asks Windows, which matches the
// wildcard against the short 8.3 name it gives a long name as well, as it does on the system drive
// by default, and by the rules of DOS: *.fac finds a Flat.fac_orig kept by hand by its short name,
// FLAT~1.FAC, and COPYING.* finds a COPYING without an extension. Neither is a file of the
// package's, and the stores, which take a preset by its own name's extension, never list the
// first, so each file Windows finds is matched against Mask once more, by its own name.
// WildcardMatch does not ignore case, so both sides are lowercased.
// Dir is the installation's folder with a backslash, Folder one of its folders, Mask the kind.
procedure RemoveLeftovers(const Dir, Folder, Mask: String);
var
  Found: TFindRec;
  Shipped, Kind, Relative: String;
begin
  Shipped := AnsiLowercase(ShippedFiles);
  Kind := AnsiLowercase(Mask);
  if not FindFirst(Dir + Folder + Mask, Found) then
    Exit;
  try
    repeat
      Relative := Folder + Found.Name;
      if (Found.Attributes and FILE_ATTRIBUTE_DIRECTORY = 0) and
        WildcardMatch(AnsiLowercase(Found.Name), Kind) and
        (Pos('|' + AnsiLowercase(Relative) + '|', Shipped) = 0) then
      begin
        if DeleteFile(Dir + Relative) then
          Log('Removed ' + Relative + ', which this version does not ship')
        else
          Log('Could not remove ' + Relative + ', which this version does not ship');
      end;
    until not FindNext(Found);
  finally
    FindClose(Found);
  end;
end;

// The folders here are the ones the preprocessor lists the shipped files of (Shipped, at the top):
// one cleaned here and not listed there would lose every file in it.
procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir: String;
begin
  if CurStep = ssPostInstall then
  begin
    Dir := ExpandConstant('{app}\');
    RemoveLeftovers(Dir, 'presets\Factsoft\', '*.fac');
    RemoveLeftovers(Dir, 'presets\BonusPresets\', '*.fac');
    RemoveLeftovers(Dir, 'presets\Input\', '*.toml');
    RemoveLeftovers(Dir, '', 'COPYING.*');
    RemoveLeftovers(Dir, '', 'OFL.*');
  end;
end;

// Closing a running FxSound Port before the uninstaller removes anything: CloseRunningProgram and
// what it uses, down to CurUninstallStepChanged.
//
// A running FxSound Port has fxsound.exe loaded, which cannot be deleted, and it would go on
// playing with its presets deleted from under it, so it is closed first. With the Restart Manager,
// as Setup's CloseApplications closes it for an upgrade and Windows Installer for the .msi: the
// program is asked to end as at log-off, which FxSound Port answers as Exit in its menu does,
// giving the default playback device back first.
//
// Only this installation's fxsound.exe running in this session is closed. A first Restart Manager
// session lists every process that has the file open; the ones that run it in this session are
// registered again, by process, in a second one, and only that one is shut down. The first, shut
// down, would also close whatever else has the file open, Explorer reading its icon say, which
// nothing would start again; and with a copy in another user's session in its list, which cannot
// be closed from here (a reboot reason), it would close nothing at all, this session's copy
// included (ERROR_FAIL_NOACTION_REBOOT). FxSound LLC's FxSound.exe, which may be running too, is
// another file and never listed. Not forced: whatever keeps fxsound.exe open is left with it, and
// the file goes when Windows restarts (uninsrestartdelete), which the question says beforehand.
const
  // RM_REBOOT_REASON: nothing keeps the session's processes from being shut down.
  RmRebootReasonNone = 0;
  ProcessQueryLimitedInformation = $1000;
  // The length of the two lists below. A file in use by more processes than that has none closed.
  RmListLength = 16;

type
  // RM_UNIQUE_PROCESS: a process id and the process's start time, a FILETIME, which tells it from a
  // later process given the same id.
  TRmUniqueProcess = record
    ProcessId: Cardinal;
    StartTimeLow: Cardinal;
    StartTimeHigh: Cardinal;
  end;
  // RM_PROCESS_INFO, 668 bytes: the two names have room for CCH_RM_MAX_APP_NAME and
  // CCH_RM_MAX_SVC_NAME characters and a null. Every field is a whole number of four-byte words
  // long, so the record has no padding for Pascal Script to lay out otherwise than the API.
  TRmProcessInfo = record
    Process: TRmUniqueProcess;
    AppName: array[0..255] of Char;
    ServiceShortName: array[0..63] of Char;
    ApplicationType: Cardinal;
    AppStatus: Cardinal;
    TSSessionId: Cardinal;
    Restartable: Cardinal;
  end;
  TRmProcessList = array[0..15] of TRmProcessInfo;
  TRmUniqueProcessList = array[0..15] of TRmUniqueProcess;

// The Restart Manager, and what tells which program a process runs and in which session. FileNames
// is the API's array of string pointers, which is how Pascal Script passes a TArrayOfString; a
// list passed by reference is the pointer to its first element, and a pointer passed as 0 is null.
// RmRegisterResources is declared twice, for the file and for processes.
function RmStartSession(var SessionHandle: Cardinal; SessionFlags: Cardinal;
  SessionKey: String): Cardinal;
  external 'RmStartSession@rstrtmgr.dll stdcall delayload uninstallonly';
function RmRegisterFile(SessionHandle: Cardinal; FileCount: Cardinal;
  FileNames: TArrayOfString; ApplicationCount: Cardinal; Applications: Cardinal;
  ServiceCount: Cardinal; ServiceNames: Cardinal): Cardinal;
  external 'RmRegisterResources@rstrtmgr.dll stdcall delayload uninstallonly';
function RmRegisterProcesses(SessionHandle: Cardinal; FileCount: Cardinal; FileNames: Cardinal;
  ApplicationCount: Cardinal; var Applications: TRmUniqueProcessList; ServiceCount: Cardinal;
  ServiceNames: Cardinal): Cardinal;
  external 'RmRegisterResources@rstrtmgr.dll stdcall delayload uninstallonly';
function RmGetList(SessionHandle: Cardinal; var ProcInfoNeeded: Cardinal; var ProcInfo: Cardinal;
  var AffectedApps: TRmProcessList; var RebootReasons: Cardinal): Cardinal;
  external 'RmGetList@rstrtmgr.dll stdcall delayload uninstallonly';
function RmShutdown(SessionHandle: Cardinal; ActionFlags: Cardinal;
  StatusCallback: Cardinal): Cardinal;
  external 'RmShutdown@rstrtmgr.dll stdcall delayload uninstallonly';
function RmEndSession(SessionHandle: Cardinal): Cardinal;
  external 'RmEndSession@rstrtmgr.dll stdcall delayload uninstallonly';
function GetCurrentProcessId(): Cardinal;
  external 'GetCurrentProcessId@kernel32.dll stdcall uninstallonly';
function ProcessIdToSessionId(ProcessId: Cardinal; var SessionId: Cardinal): Cardinal;
  external 'ProcessIdToSessionId@kernel32.dll stdcall uninstallonly';
function OpenProcess(DesiredAccess: Cardinal; InheritHandle: Cardinal;
  ProcessId: Cardinal): Cardinal;
  external 'OpenProcess@kernel32.dll stdcall uninstallonly';
function QueryFullProcessImageName(Process: Cardinal; Flags: Cardinal; ExeName: String;
  var Size: Cardinal): Cardinal;
  external 'QueryFullProcessImageNameW@kernel32.dll stdcall uninstallonly';
function CloseHandle(Handle: Cardinal): Cardinal;
  external 'CloseHandle@kernel32.dll stdcall uninstallonly';

// Whether process ProcessId runs Exe. One this uninstaller may not ask is taken to run another
// program. Paths are compared without regard to case, as NTFS compares them.
function RunsProgram(ProcessId: Cardinal; const Exe: String): Boolean;
var
  Process, Size: Cardinal;
  Image: String;
begin
  Result := False;
  Process := OpenProcess(ProcessQueryLimitedInformation, 0, ProcessId);
  if Process = 0 then
    Exit;
  try
    // In characters, with the terminating null: the longest path Windows has.
    Size := 32768;
    Image := StringOfChar(' ', Size);
    if QueryFullProcessImageName(Process, 0, Image, Size) <> 0 then
      Result := AnsiLowercase(Copy(Image, 1, Size)) = AnsiLowercase(Exe);
  finally
    CloseHandle(Process);
  end;
end;

// The name the Restart Manager gives a process, for the log.
function RmAppName(const Info: TRmProcessInfo): String;
var
  I: Integer;
begin
  Result := '';
  I := 0;
  while (I <= 255) and (Info.AppName[I] <> #0) do
  begin
    Result := Result + Info.AppName[I];
    I := I + 1;
  end;
end;

// Of the first Count processes of InUse, the ones that run Exe in this session, into Copies; returns
// how many there are. Every other one has the file open where it is not closed.
function CopiesHere(const InUse: TRmProcessList; Count: Integer; const Exe: String;
  var Copies: TRmUniqueProcessList): Integer;
var
  ThisSession: Cardinal;
  I: Integer;
  Which: String;
begin
  Result := 0;
  // Should the uninstaller's own session not be known, one no process is in.
  if ProcessIdToSessionId(GetCurrentProcessId(), ThisSession) = 0 then
    ThisSession := $FFFFFFFF;
  for I := 0 to Count - 1 do
  begin
    if (InUse[I].TSSessionId = ThisSession) and
      RunsProgram(InUse[I].Process.ProcessId, Exe) then
    begin
      Copies[Result] := InUse[I].Process;
      Result := Result + 1;
      Which := 'FxSound Port in this session';
    end
    else
      Which := 'not closed from here';
    Log('fxsound.exe is in use by ' + RmAppName(InUse[I]) + ', process ' +
      IntToStr(InUse[I].Process.ProcessId) + ' in session ' + IntToStr(InUse[I].TSSessionId) +
      ': ' + Which);
  end;
end;

// The question, unless the uninstaller is silent; Cancel stops the uninstall. Closing: FxSound
// Port is about to be closed in this session. Elsewhere: fxsound.exe is in use where it is not.
procedure ConfirmClosing(Closing, Elsewhere: Boolean);
var
  Question: String;
begin
  if UninstallSilent then
    Exit;
  Question := '';
  if Closing then
    Question := CustomMessage('CloseRunning') + #13#10#13#10;
  if Elsewhere then
    Question := Question + CustomMessage('InUseElsewhere') + #13#10#13#10;
  if MsgBox(Question + CustomMessage('GoOn'), mbConfirmation, MB_OKCANCEL) <> IDOK then
    Abort;
end;

procedure CloseRunningProgram;
var
  Session, Needed, Count, Reasons, Status: Cardinal;
  Listed, Own: Integer;
  Exe: String;
  Files: TArrayOfString;
  InUse: TRmProcessList;
  Copies: TRmUniqueProcessList;
  Closing, Elsewhere: Boolean;
begin
  Exe := ExpandConstant('{app}\{#AppExeName}');
  // What has the file open. The session key and its terminating null: 33 characters.
  if RmStartSession(Session, 0, StringOfChar(' ', 33)) <> ErrorSuccess then
  begin
    Log('The Restart Manager did not start; a running FxSound Port is not closed');
    Exit;
  end;
  try
    SetArrayLength(Files, 1);
    Files[0] := Exe;
    Needed := 0;
    Count := RmListLength;
    Reasons := 0;
    Status := RmRegisterFile(Session, 1, Files, 0, 0, 0, 0);
    if Status = ErrorSuccess then
      Status := RmGetList(Session, Needed, Count, InUse, Reasons);
  finally
    RmEndSession(Session);
  end;
  Own := 0;
  if Status = ErrorSuccess then
  begin
    Listed := Count;
    if Listed = 0 then
      Exit;
    Log('fxsound.exe is in use; Restart Manager reboot reasons ' + IntToStr(Reasons));
    Own := CopiesHere(InUse, Listed, Exe, Copies);
    Elsewhere := Own < Listed;
  end
  else if Status = ErrorMoreData then
  begin
    Log('fxsound.exe is in use by ' + IntToStr(Needed) + ' processes; none is closed');
    Elsewhere := True;
  end
  else
  begin
    Log('The Restart Manager did not list what has fxsound.exe open (error ' +
      IntToStr(Status) + '); a running FxSound Port is not closed');
    Exit;
  end;

  // This session's copies, in a session of their own, closed only when nothing stands in the way.
  Closing := False;
  if (Own > 0) and (RmStartSession(Session, 0, StringOfChar(' ', 33)) = ErrorSuccess) then
  begin
    try
      Needed := 0;
      Count := RmListLength;
      Reasons := 0;
      Status := RmRegisterProcesses(Session, 0, 0, Own, Copies, 0, 0);
      if Status = ErrorSuccess then
        Status := RmGetList(Session, Needed, Count, InUse, Reasons);
      Closing := (Status = ErrorSuccess) and (Count > 0) and (Reasons = RmRebootReasonNone);
      if Closing then
      begin
        ConfirmClosing(True, Elsewhere);
        Status := RmShutdown(Session, 0, 0);
        if Status = ErrorSuccess then
          Log('FxSound Port was running in this session and is closed')
        else
          Log('FxSound Port did not close (Restart Manager error ' + IntToStr(Status) +
            '); fxsound.exe goes when Windows restarts');
      end
      // Unless it has ended meanwhile, it runs on where it is not closed after all.
      else if (Status <> ErrorSuccess) or (Count > 0) then
      begin
        Log('FxSound Port is not closed in this session (Restart Manager error ' +
          IntToStr(Status) + ', reboot reasons ' + IntToStr(Reasons) + ')');
        Elsewhere := True;
      end;
    finally
      RmEndSession(Session);
    end;
  end
  else if Own > 0 then
  begin
    Log('The Restart Manager did not start; FxSound Port is not closed in this session');
    Elsewhere := True;
  end;
  if not Closing and Elsewhere then
    ConfirmClosing(False, True);
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
