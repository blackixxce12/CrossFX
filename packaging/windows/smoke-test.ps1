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

    With -InstallOnThisMachine, then the .msi and then the setup program, each in turn:

      * installed silently, per machine;
      * the installed tree checked as the .zip's is (the setup program adds its uninstaller), and
        the Start menu shortcut and the entry in the list of installed apps with it;
      * the other installer run over it, which has to refuse;
      * the HKCU Run value FxSound's "Launch on system startup" writes, "FxSound Port", set, and
        beside it a neighbour named "FxSound", as another program's value might be, so that the
        uninstall is seen to remove its own value and only that;
      * uninstalled silently, after which the folder, the shortcut, the entry and the "FxSound
        Port" value have to be gone, and the "FxSound" value still there.

    Nothing here starts FxSound's window: the setup program runs /VERYSILENT, which skips its
    "Launch" box, and the .msi /qn, which shows no last page at all.

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

function Set-AutostartValues {
    New-ItemProperty -LiteralPath $runKey -Name $product -Value "`"$installDir\fxsound.exe`"" -PropertyType String -Force | Out-Null
    New-ItemProperty -LiteralPath $runKey -Name 'FxSound' -Value 'left alone by FxSound Port' -PropertyType String -Force | Out-Null
}

function Assert-Uninstalled([string]$what) {
    Assert (-not (Test-Path -LiteralPath $installDir)) "$what left $installDir behind"
    Assert (-not (Test-Path -LiteralPath $shortcut)) "$what left $shortcut behind"
    Assert (@(Get-UninstallEntries).Count -eq 0) "$what left its Apps and Features entry behind"
    Assert ($null -eq (Get-RunValue $product)) "$what left the '$product' Run value behind"
    Assert ($null -ne (Get-RunValue 'FxSound')) "$what removed the 'FxSound' Run value, which is not its own"
    Remove-ItemProperty -LiteralPath $runKey -Name 'FxSound'
    Write-Host '  folder, shortcut, entry and Run value gone; the other Run value kept'
}

Assert (-not (Test-Path -LiteralPath $installDir)) "$installDir exists already; this test needs a machine without $product"
$numeric = ($treeVersion -replace '[-+].*$', '')

Write-Step "The .msi: $msi"
Invoke-Installer 'msiexec.exe' "/i `"$msi`" /qn /norestart /l*v `"$logs\msi-install.log`""
Assert-Installed 'the .msi' @() $numeric
# Each installer refuses to go over the other's installation (two entries, one folder).
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-over-msi.log`"" -Refused
Assert (@(Get-UninstallEntries).Count -eq 1) 'the setup program installed over the .msi'
Write-Host '  the setup program refuses to install over it'
Set-AutostartValues
Invoke-Installer 'msiexec.exe' "/x `"$msi`" /qn /norestart /l*v `"$logs\msi-uninstall.log`""
Assert-Uninstalled 'the .msi'

Write-Step "The setup program: $setup"
Invoke-Installer $setup "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$logs\setup-install.log`""
Assert-Installed 'the setup program' @('unins000.exe', 'unins000.dat', 'unins000.msg') $treeVersion
Invoke-Installer 'msiexec.exe' "/i `"$msi`" /qn /norestart /l*v `"$logs\msi-over-setup.log`"" -Refused
Assert (@(Get-UninstallEntries).Count -eq 1) 'the .msi installed over the setup program'
Write-Host '  the .msi refuses to install over it'
Set-AutostartValues
# The uninstaller hands over to a copy of itself in %TEMP% and exits at once, so its exit says
# nothing about the uninstall; its entry in Apps and Features going away does.
Invoke-Installer (Join-Path $installDir 'unins000.exe') "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$logs\setup-uninstall.log`""
$deadline = (Get-Date).AddMinutes(3)
while ((@(Get-UninstallEntries).Count -gt 0 -or (Test-Path -LiteralPath $installDir)) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 1
}
Assert-Uninstalled 'the setup program'

Write-Host ''
Write-Host 'The .zip, the .msi and the setup program all check out.'
