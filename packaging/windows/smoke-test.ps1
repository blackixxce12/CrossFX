<#
.SYNOPSIS
    Checks the packages build-windows.ps1 made. With -InstallOnThisMachine it also installs and
    uninstalls them, which is for a machine that is thrown away afterwards, such as a CI runner.

.DESCRIPTION
    The Windows half of what every Linux leg of .github/workflows/packages.yml does after its
    build: install the package, smoke-test it, and here uninstall it again, because a Windows
    uninstaller has as much to get right as the installer.

    Always, the .zip: unpacked into a temporary folder, it has to hold exactly the tree the
    installers install, and its fxsound.exe has to answer `--version` with the tree's version.

    With -InstallOnThisMachine, first both installers asked for a folder other than
    Program Files\FxSound Port, which each has to refuse; then the .msi and then the setup
    program, each in turn:

      * installed silently, per machine;
      * the installed tree checked as the .zip's is (the setup program adds its uninstaller), and
        the Start menu shortcut and the entry in the list of installed apps with it;
      * the other installer run over it, which has to refuse;
      * the setup program only: run again over itself, as an upgrade is, with files an earlier
        version might have shipped and this one does not planted in its folder, which the
        upgrade has to leave out of the tree, and files of other kinds beside them, which it
        has to keep;
      * the setup program only: run again while a preset is held open, so that the upgrade fails
        part-way and is rolled back, which has to leave the installed tree as it was;
      * the setup program only: FxSound Port started (--hide) to be running when it is
        uninstalled, as it usually is, which the uninstaller has to close;
      * what FxSound writes for itself set as it would: the HKCU Run value "Launch on system
        startup" writes, "FxSound Port", and the registration and icon of its notifications,
        each with a neighbour of another program's beside it, so that the uninstall is seen to
        remove its own and only those;
      * uninstalled silently, after which the folder, the shortcut, the entry, the "FxSound Port"
        value, the notifications' registration and their icon have to be gone, the neighbours
        still there, and FxSound Port not running.

    Nothing here shows FxSound's window: the setup program runs /VERYSILENT, which skips its
    "Launch" box, the .msi /qn, which shows no last page at all, and the copy started to be
    uninstalled under starts in the notification area.

.PARAMETER InstallOnThisMachine
    Install and uninstall both installers, for real, under Program Files. Not on a machine that
    is kept, and not on one where FxSound Port is installed.

.PARAMETER Version
    The version in the packages' file names, as build-windows.ps1's -Version; the tree's when
    omitted.
#>
[CmdletBinding()]
param(
    [switch]$InstallOnThisMachine,
    [string]$Version
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$product = 'FxSound Port'
$publisher = 'FxSound Linux port contributors'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
# Where FxSound registers the AppUserModelID its notifications are shown under, and the folder of
# their icon (crates/fxsound-app/src/win/toast.rs).
$appIdKey = 'HKCU:\Software\Classes\AppUserModelId'
$appUserModelId = 'FxSoundPort.FxSound'
$notificationDir = Join-Path $env:LOCALAPPDATA $product

function Write-Step([string]$text) {
    Write-Host ''
    Write-Host "==> $text"
}

function Get-TreeVersion([string]$cargoToml) {
    $section = ''
    foreach ($line in [System.IO.File]::ReadAllLines($cargoToml)) {
        if ($line -match '^\s*\[([^\]]+)\]\s*(#.*)?$') {
            $section = $Matches[1].Trim()
        } elseif ($section -eq 'workspace.package' -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    throw "no version in the [workspace.package] section of $cargoToml"
}

$treeVersion = Get-TreeVersion (Join-Path $root 'Cargo.toml')
if (-not $Version) {
    $Version = $treeVersion
}
$base = "fxsound-$Version-windows-x64"
$dist = Join-Path $root 'dist'
$zip = Join-Path $dist "$base.zip"
$msi = Join-Path $dist "$base.msi"
$setup = Join-Path $dist "$base-setup.exe"

# What every installed or unpacked tree holds, as relative path => the file in the checkout it
# has to equal, or $null where only its presence is checked (the executable, README.txt).
$expected = [ordered]@{ 'fxsound.exe' = $null; 'README.txt' = $null }
$expected['LICENSE'] = Join-Path $root 'LICENSE'
$expected['COPYING.rnnoise'] = Join-Path $root 'crates\fxsound-rnnoise\COPYING'
$expected['OFL.noto'] = Join-Path $root 'assets\fonts\OFL.txt'
$manrope = Join-Path $root 'assets\fonts\OFL-Manrope.txt'
if (Test-Path -LiteralPath $manrope -PathType Leaf) {
    $expected['OFL.manrope'] = $manrope
}
foreach ($set in @(@('Factsoft', '.fac'), @('BonusPresets', '.fac'), @('Input', '.toml'))) {
    $from = Join-Path $root "assets\presets\$($set[0])"
    foreach ($file in @(Get-ChildItem -LiteralPath $from -File | Where-Object { $_.Extension -eq $set[1] })) {
        $expected["presets\$($set[0])\$($file.Name)"] = $file.FullName
    }
}

function Assert([bool]$condition, [string]$message) {
    if (-not $condition) {
        if ($env:GITHUB_ACTIONS -eq 'true') {
            Write-Host "::error::$message"
        }
        throw $message
    }
}

# `<exe> --version`, through a pipe and under a timeout, as build-windows.ps1 asks it.
function Get-VersionOutput([string]$exe) {
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $exe
    $info.Arguments = '--version'
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        # Drained as well, so that a full pipe cannot stall the program.
        $null = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            throw "$exe --version was still running after 30 s"
        }
        return $stdout.Result.Trim()
    } finally {
        $process.Dispose()
    }
}

# The tree in $dir is the expected one; $extras are the files an installer may add (the setup
# program's uninstaller).
function Assert-Tree([string]$dir, [string[]]$extras = @()) {
    Assert (Test-Path -LiteralPath $dir -PathType Container) "$dir does not exist"
    $found = @(Get-ChildItem -LiteralPath $dir -Recurse -File | ForEach-Object { $_.FullName.Substring($dir.Length + 1) })
    foreach ($relative in $expected.Keys) {
        $path = Join-Path $dir $relative
        Assert (Test-Path -LiteralPath $path -PathType Leaf) "$relative is missing from $dir"
        if ($expected[$relative]) {
            $have = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
            $want = (Get-FileHash -LiteralPath $expected[$relative] -Algorithm SHA256).Hash
            Assert ($have -eq $want) "$path is not $($expected[$relative])"
        }
    }
    $unexpected = @($found | Where-Object { -not $expected.Contains($_) -and $extras -notcontains $_ })
    Assert ($unexpected.Count -eq 0) "$dir has files no package should: $($unexpected -join ', ')"
    $said = Get-VersionOutput (Join-Path $dir 'fxsound.exe')
    Assert ($said -ceq "fxsound $treeVersion") "$dir\fxsound.exe --version says '$said', not 'fxsound $treeVersion'"
    Write-Host "  $($expected.Count) files as expected; fxsound.exe --version: $said"
}

Write-Step "The .zip: $zip"
Assert (Test-Path -LiteralPath $zip -PathType Leaf) "$zip does not exist; run build-windows.ps1 first"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$unpacked = Join-Path ([System.IO.Path]::GetTempPath()) ("fxsound-smoke-" + [guid]::NewGuid().ToString('N'))
try {
    [System.IO.Compression.ZipFile]::ExtractToDirectory($zip, $unpacked)
    $top = @(Get-ChildItem -LiteralPath $unpacked)
    Assert ($top.Count -eq 1 -and $top[0].Name -eq $base) "the .zip does not hold exactly one folder named $base"
    Assert-Tree $top[0].FullName
} finally {
    Remove-Item -LiteralPath $unpacked -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $InstallOnThisMachine) {
    Write-Host ''
    Write-Host 'The installers were not tried: that takes -InstallOnThisMachine, on a throw-away machine.'
    return
}

# ------------------------------------------------------------------------------ the installers

$programFiles = [System.Environment]::GetFolderPath('ProgramFiles')
$installDir = Join-Path $programFiles $product
$shortcut = Join-Path ([System.Environment]::GetFolderPath('CommonPrograms')) "$product.lnk"
$logs = Join-Path $root 'target\windows-package\smoke-test'
New-Item -ItemType Directory -Path $logs -Force | Out-Null

# Apps and Features entries named FxSound Port, under the 64-bit and the 32-bit view.
function Get-UninstallEntries {
    $keys = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    return @(Get-ItemProperty -Path $keys -ErrorAction SilentlyContinue |
        Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -eq $product })
}

function Get-RunValue([string]$name) {
    $values = Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue
    if ($values -and $values.PSObject.Properties[$name]) {
        return $values.$name
    }
    return $null
}

function Invoke-Installer([string]$file, [string]$arguments, [switch]$Refused) {
    $process = Start-Process -FilePath $file -ArgumentList $arguments -Wait -PassThru
    if ($Refused) {
        Assert ($process.ExitCode -ne 0) "$(Split-Path -Leaf $file) $arguments went ahead"
    } else {
        Assert ($process.ExitCode -eq 0) "$(Split-Path -Leaf $file) $arguments exited with $($process.ExitCode)"
    }
}

# What every installed package has to look like, and what every uninstall has to leave behind.
function Assert-Installed([string]$what, [string[]]$extras, [string]$displayVersion) {
    Assert-Tree $installDir $extras
    Assert (Test-Path -LiteralPath $shortcut -PathType Leaf) "$what made no Start menu shortcut at $shortcut"
    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut).TargetPath
    Assert ($target -eq (Join-Path $installDir 'fxsound.exe')) "the shortcut points at $target"
    $entries = @(Get-UninstallEntries)
    Assert ($entries.Count -eq 1) "$what left $($entries.Count) entries named $product in Apps and Features"
    Assert ($entries[0].Publisher -eq $publisher) "$what's entry is published by $($entries[0].Publisher)"
    Assert ($entries[0].DisplayVersion -eq $displayVersion) "$what's entry is version $($entries[0].DisplayVersion), not $displayVersion"
    Write-Host "  shortcut, and an entry for $product $displayVersion by $publisher"
}

# What FxSound writes for itself, which the installers do not install and an uninstall has to
# remove all the same: the Run value of "Launch on system startup", and the registration of the
# AppUserModelID its notifications are shown under with their icon. Each with a neighbour named
# as another program's might be, which has to be left alone.
function Set-UserData {
    New-ItemProperty -LiteralPath $runKey -Name $product -Value "`"$installDir\fxsound.exe`" --hide" -PropertyType String -Force | Out-Null
    New-ItemProperty -LiteralPath $runKey -Name 'FxSound' -Value 'left alone by FxSound Port' -PropertyType String -Force | Out-Null
    foreach ($id in @($appUserModelId, 'FxSound')) {
        $key = Join-Path $appIdKey $id
        New-Item -Path $key -Force | Out-Null
        New-ItemProperty -LiteralPath $key -Name 'DisplayName' -Value $id -PropertyType String -Force | Out-Null
    }
    New-ItemProperty -LiteralPath (Join-Path $appIdKey $appUserModelId) -Name 'IconUri' -Value (Join-Path $notificationDir 'notification.png') -PropertyType String -Force | Out-Null
    New-Item -ItemType Directory -Path $notificationDir -Force | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $notificationDir 'notification.png'), [byte[]](0x89, 0x50, 0x4E, 0x47))
}

function Assert-Uninstalled([string]$what) {
    Assert (-not (Test-Path -LiteralPath $installDir)) "$what left $installDir behind"
    Assert (-not (Test-Path -LiteralPath $shortcut)) "$what left $shortcut behind"
    Assert (@(Get-UninstallEntries).Count -eq 0) "$what left its Apps and Features entry behind"
    Assert ($null -eq (Get-RunValue $product)) "$what left the '$product' Run value behind"
    Assert ($null -ne (Get-RunValue 'FxSound')) "$what removed the 'FxSound' Run value, which is not its own"
    Remove-ItemProperty -LiteralPath $runKey -Name 'FxSound'
    Assert (-not (Test-Path -LiteralPath (Join-Path $appIdKey $appUserModelId))) "$what left the registration of $appUserModelId behind"
    Assert (Test-Path -LiteralPath (Join-Path $appIdKey 'FxSound')) "$what removed the AppUserModelId 'FxSound', which is not its own"
    Remove-Item -LiteralPath (Join-Path $appIdKey 'FxSound') -Recurse
    Assert (-not (Test-Path -LiteralPath $notificationDir)) "$what left $notificationDir behind"
    Write-Host '  folder, shortcut, entry, Run value and notifications'' registration gone; the others kept'
}

# The uninstaller hands over to a copy of itself in %TEMP% and exits at once, so its exit says
# nothing about the uninstall; its entry in Apps and Features going away does.
function Wait-ForSetupUninstall {
    $deadline = (Get-Date).AddMinutes(3)
    while ((@(Get-UninstallEntries).Count -gt 0 -or (Test-Path -LiteralPath $installDir)) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 1
    }
}

Assert (-not (Test-Path -LiteralPath $installDir)) "$installDir exists already; this test needs a machine without $product"
$numeric = ($treeVersion -replace '[-+].*$', '')

# Each installer puts FxSound Port into Program Files\FxSound Port and nowhere else: every account
# runs it from there, and a folder elsewhere, as one straight on the system drive is, may be one
# other accounts can change. A folder asked for on the command line is refused, by the .msi's
# launch condition (exit 1603) and by the setup program's PrepareToInstall (exit 7).
Write-Step 'A folder other than Program Files'
$elsewhere = Join-Path $env:SystemDrive "\$product Elsewhere"
Assert (-not (Test-Path -LiteralPath $elsewhere)) "$elsewhere exists already"
Invoke-Installer 'msiexec.exe' "/i `"$msi`" INSTALLFOLDER=`"$elsewhere`" /qn /norestart /l*v `"$logs\msi-elsewhere.log`"" -Refused
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR=`"$elsewhere`" /LOG=`"$logs\setup-elsewhere.log`"" -Refused
Assert (-not (Test-Path -LiteralPath $elsewhere)) "an installer made $elsewhere"
Assert (-not (Test-Path -LiteralPath $installDir)) "an installer refused $elsewhere and installed into $installDir all the same"
Assert (@(Get-UninstallEntries).Count -eq 0) "an installer refused $elsewhere and left an entry in Apps and Features"
Write-Host "  both refuse $elsewhere"

Write-Step "The .msi: $msi"
Invoke-Installer 'msiexec.exe' "/i `"$msi`" /qn /norestart /l*v `"$logs\msi-install.log`""
Assert-Installed 'the .msi' @() $numeric
# Each installer refuses to go over the other's installation (two entries, one folder).
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-over-msi.log`"" -Refused
Assert (@(Get-UninstallEntries).Count -eq 1) 'the setup program installed over the .msi'
Write-Host '  the setup program refuses to install over it'
Set-UserData
Invoke-Installer 'msiexec.exe' "/x `"$msi`" /qn /norestart /l*v `"$logs\msi-uninstall.log`""
Assert-Uninstalled 'the .msi'

Write-Step "The setup program: $setup"
$setupFiles = @('unins000.exe', 'unins000.dat', 'unins000.msg')
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-install.log`""
Assert-Installed 'the setup program' $setupFiles $treeVersion
Invoke-Installer 'msiexec.exe' "/i `"$msi`" /qn /norestart /l*v `"$logs\msi-over-setup.log`"" -Refused
Assert (@(Get-UninstallEntries).Count -eq 1) 'the .msi installed over the setup program'
Write-Host '  the .msi refuses to install over it'

# An upgrade: the setup program installs over the version before it rather than uninstalling it,
# and the stores list every preset in the folders, so the presets and notices that version shipped
# and this one does not have to go, as the .msi's upgrade removes them with the old product.
#
# Those and nothing else: a file of another kind in the same folders stays, even one Windows finds
# when asked for a kind. The setup program asks it for *.fac, which Windows matches against the
# short 8.3 name it gives a long name as well, as it does on the system drive by default, and so
# finds a copy of a preset kept by hand as Flat.fac_orig by its short name, FLAT~1.FAC; and for
# COPYING.* and OFL.*, which by the rules of DOS find a COPYING and an OFL without an extension.
# They are taken away again once checked, for the legs below, which want the package's tree and
# nothing more.
$dropped = @('presets\Factsoft\Dropped.fac', 'presets\BonusPresets\Dropped.fac', 'presets\Input\Dropped.toml', 'OFL.dropped', 'COPYING.dropped')
foreach ($relative in $dropped) {
    [System.IO.File]::WriteAllText((Join-Path $installDir $relative), "shipped by an earlier version`n")
}
$byShortName = 'presets\Factsoft\Flat.fac_orig'
$kept = @($byShortName, 'COPYING', 'OFL')
foreach ($relative in $kept) {
    [System.IO.File]::WriteAllText((Join-Path $installDir $relative), "not a file of $product's`n")
}
# The short name is the one thing here that depends on the machine: where Windows makes none,
# *.fac does not find Flat.fac_orig, and the leg shows less.
$short = (New-Object -ComObject Scripting.FileSystemObject).GetFile((Join-Path $installDir $byShortName)).ShortName
if ($short -notlike '*.fac') {
    $note = "$byShortName has no short name *.fac finds on this machine (it is '$short'), so the upgrade is not seen to keep such a file"
    Write-Warning $note
    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Host "::warning title=Smoke test::$note"
    }
}
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-upgrade.log`""
Assert-Installed 'the upgrade' ($setupFiles + $kept) $treeVersion
foreach ($relative in $kept) {
    $path = Join-Path $installDir $relative
    Assert (Test-Path -LiteralPath $path -PathType Leaf) "the upgrade removed $relative, which is not a file of $product's"
    Remove-Item -LiteralPath $path
}
Write-Host "  an upgrade leaves out the $($dropped.Count) files this version does not ship and keeps the $($kept.Count) of other kinds beside them"

# An upgrade that fails part-way, here on a preset another program holds open, which Setup cannot
# replace: silent, it gives up (exit 5, Abort at the Retry box) and rolls back, deleting every file
# it copied that was not there before it started. The files the version before shipped and this one
# does not are removed only after a successful installation, so nothing the installed version has
# may go with the rollback: cleared before the copy, the presets copied again would have counted
# as new. Not a .fac or .toml the Restart Manager is asked about (only *.exe, *.dll, *.chm are), so
# Setup does not try to close this script to free it.
$held = @($expected.Keys | Where-Object { $_ -like 'presets\Input\*' })[0]
$holder = [System.IO.File]::Open((Join-Path $installDir $held), [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
try {
    Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-failed-upgrade.log`"" -Refused
} finally {
    $holder.Dispose()
}
Assert-Installed 'an upgrade that failed' $setupFiles $treeVersion
Write-Host "  an upgrade that fails on $held is rolled back with nothing of the installation lost"

# Uninstalled while FxSound Port runs, as it usually is: it sits in the notification area once
# started. The uninstaller has to close it, or fxsound.exe and its folder stay behind.
$running = Start-Process -FilePath (Join-Path $installDir 'fxsound.exe') -ArgumentList '--hide' -PassThru
# Held from the start: Windows PowerShell's Process otherwise has no exit code to give once the
# process has ended.
$null = $running.Handle
Start-Sleep -Seconds 5
if ($running.HasExited) {
    $note = "fxsound.exe --hide ended by itself (exit $($running.ExitCode)) on this machine, so closing a running FxSound Port is not tried"
    Write-Warning $note
    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Host "::warning title=Smoke test::$note"
    }
} else {
    Write-Host "  FxSound Port is running (process $($running.Id))"
}
Set-UserData
Invoke-Installer (Join-Path $installDir 'unins000.exe') "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$logs\setup-uninstall.log`""
Wait-ForSetupUninstall
Assert ($running.WaitForExit(30000)) "the uninstaller left FxSound Port (process $($running.Id)) running"
Assert-Uninstalled 'the setup program'

Write-Host ''
Write-Host 'The .zip, the .msi and the setup program all check out.'
