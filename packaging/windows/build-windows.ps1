<#
.SYNOPSIS
    Builds the Windows packages of FxSound Port: a setup program (Inno Setup 6), a Windows
    Installer package (WiX Toolset 5.0.2) and a portable .zip, each with its SHA-256 sum.

.DESCRIPTION
    The Windows counterpart of packaging/build-tarball.sh, and what the release attaches for
    Windows (.github/workflows/packages.yml, the windows job). Into dist\ beside the repository
    root it writes

        fxsound-<version>-windows-x64-setup.exe    and .sha256
        fxsound-<version>-windows-x64.msi          and .sha256
        fxsound-<version>-windows-x64.zip          and .sha256

    where each .sha256 is one line in sha256sum's format, so `sha256sum -c` checks it.

    All three carry the same tree: what build-tarball.sh ships, laid out as Windows finds it,
    everything in the folder of the executable (install_data_dirs in
    crates/fxsound-preset/src/lib.rs looks for "presets" beside fxsound.exe):

        fxsound.exe
        presets\Factsoft\*.fac   presets\BonusPresets\*.fac   presets\Input\*.toml
        LICENSE   COPYING.rnnoise   OFL.noto   OFL.manrope   README.txt

    The fonts and the translations are compiled into fxsound.exe. The virtual playback device it
    plays through is not shipped and not downloaded; README.txt and both installers' last page
    say so.

    The version is Cargo.toml's [workspace.package] version and is written nowhere else: the
    installers get it on their compilers' command lines. As build-tarball.sh does, the script
    builds the binary from the tree it sits in, and refuses to pack one whose `--version` is not
    "fxsound <that version>". It also refuses one that needs the Visual C++ runtime DLLs, which
    a clean Windows does not have: the build here links the C runtime statically.

    Needs cargo (unless -Binary is given), Inno Setup 6.3 or later (ISCC.exe on PATH, in its
    default folder, or named by the ISCC environment variable) and WiX Toolset 5.0.2 (wix.exe,
    `dotnet tool install --global wix --version 5.0.2`) with its UI extension
    (`wix extension add -g WixToolset.UI.wixext/5.0.2`). Not WiX 6 or later, whose licence this
    project has not accepted. The zip and the checksums are .NET's own.

    Written for Windows PowerShell 5.1, which every Windows 10 and 11 has and which CI runs it
    with; PowerShell 7 is not what it is tested on.

.PARAMETER Binary
    An fxsound.exe built elsewhere, packed instead of building one. It is held to the same two
    checks.

.PARAMETER Version
    The version the three files are named after, as build-tarball.sh's argument is; the tree's
    when omitted. What the installers report is always the tree's version, which is the
    binary's.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File packaging\windows\build-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$Binary,
    [string]$Version
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$root = (Resolve-Path -LiteralPath (Join-Path $here '..\..')).Path

# Manrope's licence lives beside its faces; until the branch that adds them is merged, neither is
# in the tree.
$manropeOfl = 'assets\fonts\OFL-Manrope.txt'

function Write-Step([string]$text) {
    Write-Host ''
    Write-Host "==> $text"
}

# A warning that stands out in a long build log, and in a GitHub Actions run as an annotation.
function Write-LoudWarning([string]$title, [string[]]$lines) {
    $rule = '!' * 78
    Write-Warning $rule
    foreach ($line in @($title) + $lines) {
        Write-Warning $line
    }
    Write-Warning $rule
    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Host "::warning title=$title::$($lines -join ' ')"
    }
}

# [workspace.package]'s version: the one fxsound reports (env!("CARGO_PKG_VERSION")).
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

# One argument as CommandLineToArgvW and the C runtime read it back, for Start-Process, which in
# Windows PowerShell joins an argument list with spaces and quotes nothing.
function ConvertTo-Argument([string]$value) {
    if ($value.Length -gt 0 -and $value -notmatch '[\s"]') {
        return $value
    }
    $escaped = [regex]::Replace($value, '(\\*)"', { param($m) ($m.Groups[1].Value * 2) + '\"' })
    $escaped = [regex]::Replace($escaped, '(\\+)$', { param($m) $m.Groups[1].Value * 2 })
    return '"' + $escaped + '"'
}

# Runs a tool to completion and fails on a non-zero exit. Its output goes to the console, or, with
# -Log, to that file (and its error output to <log>.err), which is then shown. Start-Process
# rather than the call operator: Windows PowerShell turns a native program's error output into
# error records, which $ErrorActionPreference = 'Stop' makes fatal, and cargo writes its progress
# there.
function Invoke-Tool([string]$file, [string[]]$arguments, [string]$log, [string]$directory = $root) {
    $line = ($arguments | ForEach-Object { ConvertTo-Argument $_ }) -join ' '
    $start = @{
        FilePath = $file
        ArgumentList = $line
        WorkingDirectory = $directory
        NoNewWindow = $true
        Wait = $true
        PassThru = $true
    }
    if ($log) {
        $start.RedirectStandardOutput = $log
        $start.RedirectStandardError = "$log.err"
    }
    $process = Start-Process @start
    if ($log) {
        # ISCC and wix both write UTF-8 into a redirected stream.
        foreach ($output in @($log, "$log.err")) {
            Get-Content -LiteralPath $output -Encoding UTF8 | ForEach-Object { Write-Host $_ }
        }
    }
    if ($process.ExitCode -ne 0) {
        throw "$(Split-Path -Leaf $file) failed with exit code $($process.ExitCode)"
    }
}

# What `<exe> --version` prints. Through a pipe, which reaches a GUI-subsystem program's stdout
# as well, and under a timeout: a binary that does not know --version starts its window instead,
# and would hold the build for ever.
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
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            throw "$exe --version was still running after 30 s; refusing to pack it"
        }
        if ($process.ExitCode -ne 0) {
            throw "$exe --version exited with $($process.ExitCode): $($stderr.Result.Trim())"
        }
        return $stdout.Result.Trim()
    } finally {
        $process.Dispose()
    }
}

# The DLLs a PE image imports, read from its import directory.
function Get-ImportedDlls([string]$path) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x00004550) {
        throw "$path is not a Windows executable"
    }
    $coff = $pe + 4
    $sectionCount = [BitConverter]::ToUInt16($bytes, $coff + 2)
    $optionalSize = [BitConverter]::ToUInt16($bytes, $coff + 16)
    $optional = $coff + 20
    # The data directories follow the optional header's fixed part, 96 bytes for PE32 and 112 for
    # PE32+; the import table is the second of them.
    $directories = $optional + $(if ([BitConverter]::ToUInt16($bytes, $optional) -eq 0x20B) { 112 } else { 96 })
    $importRva = [BitConverter]::ToUInt32($bytes, $directories + 8)
    $sections = $optional + $optionalSize
    $toOffset = {
        param([uint32]$rva)
        for ($i = 0; $i -lt $sectionCount; $i++) {
            $header = $sections + 40 * $i
            $virtual = [BitConverter]::ToUInt32($bytes, $header + 12)
            $size = [Math]::Max([BitConverter]::ToUInt32($bytes, $header + 8), [BitConverter]::ToUInt32($bytes, $header + 16))
            if ($rva -ge $virtual -and $rva -lt $virtual + $size) {
                return [int]($rva - $virtual + [BitConverter]::ToUInt32($bytes, $header + 20))
            }
        }
        throw "$path has an import table outside its sections"
    }
    $names = New-Object System.Collections.Generic.List[string]
    if ($importRva -eq 0) {
        return , $names
    }
    # IMAGE_IMPORT_DESCRIPTORs, 20 bytes each, up to the one whose name is zero.
    for ($descriptor = & $toOffset $importRva; ; $descriptor += 20) {
        $nameRva = [BitConverter]::ToUInt32($bytes, $descriptor + 12)
        if ($nameRva -eq 0) {
            break
        }
        $start = & $toOffset $nameRva
        $end = $start
        while ($bytes[$end] -ne 0) {
            $end++
        }
        $names.Add([System.Text.Encoding]::ASCII.GetString($bytes, $start, $end - $start))
    }
    return , $names
}

# RTF for the .msi's licence page, which shows nothing else: the text verbatim, in a fixed-width
# face because the licence is laid out with spaces.
function ConvertTo-Rtf([string]$text) {
    $body = $text.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
    $body = [regex]::Replace($body, '[^\x00-\x7F]', {
        param($m)
        $code = [int][char]$m.Value
        if ($code -gt 32767) { $code -= 65536 }
        '\u' + $code + '?'
    })
    $body = $body.Replace("`r`n", "`n").Replace("`n", "\par`r`n")
    return "{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fmodern\fcharset0 Consolas;}}`r`n" +
        "\viewkind4\uc1\pard\f0\fs16`r`n$body}`r`n"
}

# Reads one column of an .msi's table through the Windows Installer automation interface, which
# opens the database read-only and registers nothing.
function Get-MsiColumn([string]$msi, [string]$query) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($msi, 0))
    $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($query))
    $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
    $values = New-Object System.Collections.Generic.List[string]
    while ($true) {
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -eq $record) {
            break
        }
        $values.Add($record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(1)))
    }
    $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null) | Out-Null
    foreach ($com in @($view, $database, $installer)) {
        [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($com)
    }
    return $values.ToArray()
}

function Find-Iscc {
    $candidates = @()
    if ($env:ISCC) {
        $candidates += $env:ISCC
    }
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($onPath) {
        $candidates += $onPath.Source
    }
    foreach ($base in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, (Join-Path $env:LOCALAPPDATA 'Programs'))) {
        if ($base) {
            $candidates += (Join-Path $base 'Inno Setup 6\ISCC.exe')
        }
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw 'ISCC.exe was not found: install Inno Setup 6.3 or later, or set ISCC to its path'
}

function Find-Wix {
    $onPath = Get-Command wix.exe -ErrorAction SilentlyContinue
    $wix = if ($onPath) { $onPath.Source } else { Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe' }
    if (-not (Test-Path -LiteralPath $wix -PathType Leaf)) {
        throw 'wix.exe was not found: dotnet tool install --global wix --version 5.0.2'
    }
    $version = (& $wix --version | Out-String).Trim()
    # WiX 6 and 7 build only after `wix eula accept`, the Open Source Maintenance Fee licence,
    # which this project has not accepted; 5.0.2 is the last WiX without it.
    if ($version -notmatch '^5\.') {
        throw "wix.exe is version $version; this package is built with WiX 5.0.2"
    }
    $extensions = (& $wix extension list -g | Out-String)
    if ($extensions -notmatch 'WixToolset\.UI\.wixext 5\.') {
        throw 'the WiX UI extension is missing: wix extension add -g WixToolset.UI.wixext/5.0.2'
    }
    return @{ Path = $wix; Version = $version }
}

# --------------------------------------------------------------------------------- the version

$treeVersion = Get-TreeVersion (Join-Path $root 'Cargo.toml')
if (-not $Version) {
    $Version = $treeVersion
}
if ($Version -notmatch '^[0-9A-Za-z][0-9A-Za-z.+_-]*$') {
    throw "'$Version' cannot be part of a file name"
}
# MSI and VERSIONINFO hold numbers only, and MSI at most 255.255.65535: a pre-release suffix is
# dropped, so 1.0.0-rc.1 and 1.0.0 have the same numeric version (the .msi replaces one with the
# other: AllowSameVersionUpgrades).
if ($treeVersion -notmatch '^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$') {
    throw "Cargo.toml's version '$treeVersion' is not major.minor.patch"
}
$major, $minor, $patch = [int]$Matches[1], [int]$Matches[2], [int]$Matches[3]
if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 65535) {
    throw "$treeVersion does not fit an MSI version (at most 255.255.65535)"
}
$numeric = "$major.$minor.$patch"
$base = "fxsound-$Version-windows-x64"

$dist = Join-Path $root 'dist'
$work = Join-Path $root 'target\windows-package'
$stage = Join-Path $work 'stage'
$icon = Join-Path $root 'assets\windows\fxsound.ico'

Write-Step "FxSound Port $treeVersion (files named $base)"

# Before the build, which is the long part: a missing tool should not wait for it.
$iscc = Find-Iscc
$wix = Find-Wix
Write-Host "Inno Setup: $iscc"
Write-Host "WiX:        $($wix.Path) $($wix.Version)"

# ----------------------------------------------------------------------------------- the binary

if ($Binary) {
    $exe = (Resolve-Path -LiteralPath $Binary).Path
    Write-Step "Packing $exe"
} else {
    Write-Step 'cargo build --release --locked -p fxsound-app'
    $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if (-not $cargo) {
        throw 'cargo was not found; install Rust, or pass -Binary'
    }
    # Always built, as build-tarball.sh does: an fxsound.exe an earlier checkout left in target\
    # would otherwise be packed under this tree's version. The target directory is pinned for this
    # one command, so a CARGO_TARGET_DIR set elsewhere cannot send the build where this script does
    # not look. The C runtime is linked statically, because the dynamic one, vcruntime140.dll,
    # comes with the Visual C++ Redistributable and not with Windows. RUSTFLAGS is appended to,
    # and both variables are put back afterwards.
    $saved = @{ CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR; RUSTFLAGS = $env:RUSTFLAGS; CARGO_ENCODED_RUSTFLAGS = $env:CARGO_ENCODED_RUSTFLAGS }
    try {
        $env:CARGO_TARGET_DIR = Join-Path $root 'target'
        if ($env:CARGO_ENCODED_RUSTFLAGS) {
            # Cargo reads only this one when it is set; its flags are separated by 0x1F.
            $env:CARGO_ENCODED_RUSTFLAGS = "$($env:CARGO_ENCODED_RUSTFLAGS)$([char]0x1F)-C$([char]0x1F)target-feature=+crt-static"
        } else {
            $env:RUSTFLAGS = (@($env:RUSTFLAGS, '-C target-feature=+crt-static') | Where-Object { $_ }) -join ' '
        }
        Invoke-Tool $cargo.Source @('build', '--release', '--locked', '-p', 'fxsound-app', '--bin', 'fxsound')
    } finally {
        foreach ($name in $saved.Keys) {
            # $null removes a variable that was not set before.
            [System.Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
    }
    $exe = Join-Path $root 'target\release\fxsound.exe'
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "$exe does not exist"
}

# The binary has to be this tree's: `fxsound --version` prints "fxsound <version>".
$said = Get-VersionOutput $exe
if ($said -cne "fxsound $treeVersion") {
    throw "build-windows.ps1: $exe says '$said', but Cargo.toml is at $treeVersion; refusing to pack it"
}
Write-Host "$exe --version: $said"
$imports = Get-ImportedDlls $exe
Write-Host "imports: $($imports -join ', ')"
$runtime = @($imports | Where-Object { $_ -match '^(vcruntime|msvcp|concrt)\d' })
if ($runtime.Count -gt 0) {
    throw ("$exe needs $($runtime -join ', '), which Windows does not include; build it with " +
        "RUSTFLAGS='-C target-feature=+crt-static', as this script does")
}

# ----------------------------------------------------------------------------------- the stage

Write-Step "Staging $stage"
if (Test-Path -LiteralPath $work) {
    Remove-Item -LiteralPath $work -Recurse -Force
}
New-Item -ItemType Directory -Path $stage | Out-Null
New-Item -ItemType Directory -Path $dist -Force | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'fxsound.exe')

# The three preset trees build-tarball.sh installs, and only the files it installs: not
# BonusPresets.zip, nor the MeaningfulPresets list beside the .fac files.
foreach ($set in @(@('Factsoft', '.fac'), @('BonusPresets', '.fac'), @('Input', '.toml'))) {
    $from = Join-Path $root "assets\presets\$($set[0])"
    $to = Join-Path $stage "presets\$($set[0])"
    New-Item -ItemType Directory -Path $to | Out-Null
    $files = @(Get-ChildItem -LiteralPath $from -File | Where-Object { $_.Extension -eq $set[1] })
    if ($files.Count -eq 0) {
        throw "no *$($set[1]) in $from"
    }
    foreach ($file in $files) {
        Copy-Item -LiteralPath $file.FullName -Destination $to
    }
}

# The AGPL, and beside it the notices of the parts of the binary that are not AGPL, whose
# licences make shipping the text a condition of shipping the binary: the RNNoise code and model
# (BSD-3-Clause) and the embedded fonts (SIL OFL 1.1). Byte for byte, as the Linux packages carry
# them; the CI job compares them with the tree's.
$notices = [ordered]@{
    'LICENSE' = 'LICENSE'
    'COPYING.rnnoise' = 'crates\fxsound-rnnoise\COPYING'
    'OFL.noto' = 'assets\fonts\OFL.txt'
}
if (Test-Path -LiteralPath (Join-Path $root $manropeOfl) -PathType Leaf) {
    $notices['OFL.manrope'] = $manropeOfl
} elseif (@(Get-ChildItem -LiteralPath (Join-Path $root 'assets\fonts') -Filter 'Manrope*' -File).Count -gt 0) {
    throw "assets\fonts has Manrope faces but no $manropeOfl; their licence has to ship with them"
} else {
    Write-LoudWarning 'Manrope licence missing' @(
        "$manropeOfl is not in this tree, so the packages go out WITHOUT OFL.manrope.",
        'It arrives with the Manrope faces; a release must not be built before they are merged.')
}
foreach ($name in $notices.Keys) {
    Copy-Item -LiteralPath (Join-Path $root $notices[$name]) -Destination (Join-Path $stage $name)
}

# README.txt with Windows line ends and a byte-order mark, so that every editor opens its
# Russian half as UTF-8.
$readme = [System.IO.File]::ReadAllText((Join-Path $here 'README.txt'), [System.Text.Encoding]::UTF8)
$readme = $readme.Replace("`r`n", "`n").Replace("`n", "`r`n")
[System.IO.File]::WriteAllText((Join-Path $stage 'README.txt'), $readme, (New-Object System.Text.UTF8Encoding $true))

$staged = @(Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName)
foreach ($file in $staged) {
    Write-Host ("  {0,10}  {1}" -f $file.Length, $file.FullName.Substring($stage.Length + 1))
}

# -------------------------------------------------------------------------- the setup program

$setup = Join-Path $dist "$base-setup.exe"
Write-Step "Inno Setup: $setup"
Invoke-Tool $iscc @(
    "/DAppVersion=$treeVersion",
    "/DAppVersionNumeric=$numeric.0",
    "/DStageDir=$stage",
    "/DIconFile=$icon",
    "/DOutputDir=$dist",
    "/DOutputBaseFilename=$base-setup",
    (Join-Path $here 'fxsound.iss')
) -log (Join-Path $work 'iscc.log')

# ------------------------------------------------------------------- the Windows Installer package

$msi = Join-Path $dist "$base.msi"
Write-Step "WiX: $msi"
$rtf = Join-Path $work 'license.rtf'
$license = [System.IO.File]::ReadAllText((Join-Path $root 'LICENSE'), [System.Text.Encoding]::UTF8)
[System.IO.File]::WriteAllText($rtf, (ConvertTo-Rtf $license), (New-Object System.Text.ASCIIEncoding))
Invoke-Tool $wix.Path @(
    'build',
    '-arch', 'x64',
    '-ext', 'WixToolset.UI.wixext',
    '-d', "Version=$numeric",
    '-d', "StageDir=$stage",
    '-d', "LicenseRtf=$rtf",
    '-d', "IconFile=$icon",
    '-intermediatefolder', (Join-Path $work 'wix'),
    '-pdb', (Join-Path $work 'fxsound.wixpdb'),
    '-o', $msi,
    (Join-Path $here 'fxsound.wxs')
) -log (Join-Path $work 'wix.log')

# The Windows Installer SDK's consistency checks (ICEs), as Orca runs them: on a copy of the
# package, installing nothing. ICE61 is the warning AllowSameVersionUpgrades always raises.
Invoke-Tool $wix.Path @(
    'msi', 'validate',
    '-sice', 'ICE61',
    '-pdb', (Join-Path $work 'fxsound.wixpdb'),
    '-intermediateFolder', (Join-Path $work 'ice'),
    $msi
) -log (Join-Path $work 'ice.log')

# Every staged file has to be in the package's File table, and nothing else.
$packed = @(Get-MsiColumn $msi 'SELECT `FileName` FROM `File`' | ForEach-Object { ($_ -split '\|')[-1] } | Sort-Object)
$expected = @($staged | ForEach-Object { $_.Name } | Sort-Object)
$difference = @(Compare-Object $expected $packed)
if ($difference.Count -gt 0) {
    throw "the .msi's files are not the staged ones: $($difference | Out-String)"
}
$productVersion = @(Get-MsiColumn $msi 'SELECT `Value` FROM `Property` WHERE `Property` = ''ProductVersion''')[0]
if ($productVersion -ne $numeric) {
    throw "the .msi's ProductVersion is $productVersion, not $numeric"
}
Write-Host "$($packed.Count) files in the File table; ProductVersion $productVersion"

# ------------------------------------------------------------------------------------- the zip

$zip = Join-Path $dist "$base.zip"
Write-Step "Zip: $zip"
# System.IO.Compression rather than Compress-Archive, which in Windows PowerShell writes entry
# names with backslashes that other unzip tools take for part of the file name. One top folder,
# named after the archive, as in the tarball.
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
if (Test-Path -LiteralPath $zip) {
    Remove-Item -LiteralPath $zip -Force
}
$archive = [System.IO.Compression.ZipFile]::Open($zip, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in $staged) {
        $entry = "$base/" + $file.FullName.Substring($stage.Length + 1).Replace('\', '/')
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName,
            $entry, [System.IO.Compression.CompressionLevel]::Optimal)
    }
} finally {
    $archive.Dispose()
}

# ---------------------------------------------------------------------------------- the sums

Write-Step 'SHA-256'
foreach ($artifact in @($setup, $msi, $zip)) {
    $name = Split-Path -Leaf $artifact
    $hash = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText("$artifact.sha256", "$hash  $name`n", (New-Object System.Text.UTF8Encoding $false))
    Write-Host ("{0}  {1}  ({2:N0} bytes)" -f $hash, $name, (Get-Item -LiteralPath $artifact).Length)
}
foreach ($artifact in @($setup, $msi, $zip)) {
    Write-Output $artifact
}
