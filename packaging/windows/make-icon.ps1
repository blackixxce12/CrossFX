<#
.SYNOPSIS
    Regenerates assets\windows\crossfx.ico from assets\images\fxsound_large.png, the original's
    artwork.

.DESCRIPTION
    The icon crossfx.exe carries (crates\crossfx-app\build.rs) and that both installers show.
    The .ico is committed, so a build needs neither this script nor System.Drawing; run it again
    only when the artwork changes, and commit what it writes.

    Seven sizes, the ones Windows asks for: 16, 24 and 32 for the title bar, the taskbar and the
    notification area at 100-200 % scaling, 48 and 64 for Explorer's medium icons, 128 and 256
    for its large and extra-large views. Every size up to 128 is a 32-bit DIB with its AND mask,
    which every icon reader understands; 256 is PNG-compressed, as Windows has wanted since
    Vista, because a 256-pixel DIB would weigh a quarter of a megabyte for one frame.

    Each size is resampled from the 256-pixel PNG with GDI+'s high-quality bicubic filter, edges
    clamped (TileFlipXY) so the transparent corners do not bleed grey into the rounded square;
    the 256 frame is the source pixels unchanged. The output is the same bytes on every run.

    PowerShell 5.1 or later and System.Drawing; no other tool.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File packaging\windows\make-icon.ps1
#>
[CmdletBinding()]
param(
    # The artwork; square, 256 pixels or larger.
    [string]$Source,
    # Where the icon goes.
    [string]$Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Source) { $Source = Join-Path $root 'assets\images\fxsound_large.png' }
if (-not $Output) { $Output = Join-Path $root 'assets\windows\crossfx.ico' }

Add-Type -AssemblyName System.Drawing

$sizes = @(16, 24, 32, 48, 64, 128, 256)
# Frames this size and larger are stored as PNG.
$pngFrom = 256

# The artwork, checked to be square and large enough. Kept open until every frame is made: a
# clone of a bitmap loaded from a file may still read from it.
function Read-Artwork([string]$path) {
    $loaded = New-Object System.Drawing.Bitmap $path
    $largest = ($sizes | Measure-Object -Maximum).Maximum
    if ($loaded.Width -ne $loaded.Height -or $loaded.Width -lt $largest) {
        $loaded.Dispose()
        throw "$path is $($loaded.Width)x$($loaded.Height); the icon needs a square of at least $largest"
    }
    return $loaded
}

# One frame: the artwork at $size x $size, 32-bit ARGB whatever the file stores.
function New-Frame([System.Drawing.Bitmap]$art, [int]$size) {
    $format = [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    if ($size -eq $art.Width) {
        return $art.Clone((New-Object System.Drawing.Rectangle 0, 0, $size, $size), $format)
    }
    $frame = New-Object System.Drawing.Bitmap $size, $size, $format
    $graphics = [System.Drawing.Graphics]::FromImage($frame)
    $attributes = New-Object System.Drawing.Imaging.ImageAttributes
    try {
        $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
        $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $attributes.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)
        $graphics.DrawImage($art, (New-Object System.Drawing.Rectangle 0, 0, $size, $size),
            0, 0, $art.Width, $art.Height, [System.Drawing.GraphicsUnit]::Pixel, $attributes)
    } finally {
        $attributes.Dispose()
        $graphics.Dispose()
    }
    return $frame
}

# The frame's pixels, top row first, four bytes each in B, G, R, A order.
function Get-Pixels([System.Drawing.Bitmap]$frame) {
    $size = $frame.Width
    $rect = New-Object System.Drawing.Rectangle 0, 0, $size, $size
    $data = $frame.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $row = $size * 4
        $pixels = New-Object byte[] ($row * $size)
        # Row by row: the stride of a locked bitmap may be padded or, for a bottom-up bitmap,
        # negative.
        for ($y = 0; $y -lt $size; $y++) {
            $line = [System.IntPtr]::Add($data.Scan0, $y * $data.Stride)
            [System.Runtime.InteropServices.Marshal]::Copy($line, $pixels, $y * $row, $row)
        }
        return , $pixels
    } finally {
        $frame.UnlockBits($data)
    }
}

# A frame as an icon DIB: BITMAPINFOHEADER (with the height doubled for the mask that follows),
# the colour bitmap bottom row first, then the 1-bit AND mask, set where a pixel is fully
# transparent, rows padded to 32 bits.
function ConvertTo-Dib([System.Drawing.Bitmap]$frame) {
    $size = $frame.Width
    $pixels = Get-Pixels $frame
    $row = $size * 4
    $maskRow = [int][Math]::Floor(($size + 31) / 32) * 4
    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $stream
    $writer.Write([uint32]40)                       # biSize
    $writer.Write([int32]$size)                     # biWidth
    $writer.Write([int32]($size * 2))               # biHeight: colour bitmap and mask
    $writer.Write([uint16]1)                        # biPlanes
    $writer.Write([uint16]32)                       # biBitCount
    $writer.Write([uint32]0)                        # biCompression: BI_RGB
    $writer.Write([uint32]($row * $size + $maskRow * $size)) # biSizeImage
    $writer.Write([int32]0)                         # biXPelsPerMeter
    $writer.Write([int32]0)                         # biYPelsPerMeter
    $writer.Write([uint32]0)                        # biClrUsed
    $writer.Write([uint32]0)                        # biClrImportant
    for ($y = $size - 1; $y -ge 0; $y--) {
        $writer.Write($pixels, $y * $row, $row)
    }
    for ($y = $size - 1; $y -ge 0; $y--) {
        $mask = New-Object byte[] $maskRow
        for ($x = 0; $x -lt $size; $x++) {
            if ($pixels[$y * $row + $x * 4 + 3] -eq 0) {
                $mask[[int][Math]::Floor($x / 8)] = $mask[[int][Math]::Floor($x / 8)] -bor (0x80 -shr ($x % 8))
            }
        }
        $writer.Write($mask)
    }
    $writer.Flush()
    return , $stream.ToArray()
}

function ConvertTo-Png([System.Drawing.Bitmap]$frame) {
    $stream = New-Object System.IO.MemoryStream
    $frame.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $stream.ToArray()
}

$art = Read-Artwork $Source
$images = New-Object System.Collections.Generic.List[byte[]]
try {
    foreach ($size in $sizes) {
        $frame = New-Frame $art $size
        try {
            if ($size -ge $pngFrom) {
                $images.Add((ConvertTo-Png $frame))
            } else {
                $images.Add((ConvertTo-Dib $frame))
            }
        } finally {
            $frame.Dispose()
        }
    }
} finally {
    $art.Dispose()
}

# ICONDIR, one ICONDIRENTRY per frame, then the frames in the same order.
$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter $stream
$writer.Write([uint16]0)                # reserved
$writer.Write([uint16]1)                # type: icon
$writer.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    # 0 stands for 256 in the one-byte width and height.
    $edge = [byte]($sizes[$i] % 256)
    $writer.Write($edge)                # bWidth
    $writer.Write($edge)                # bHeight
    $writer.Write([byte]0)              # bColorCount: not a palette image
    $writer.Write([byte]0)              # bReserved
    $writer.Write([uint16]1)            # wPlanes
    $writer.Write([uint16]32)           # wBitCount
    $writer.Write([uint32]$images[$i].Length)
    $writer.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) {
    $writer.Write($image)
}
$writer.Flush()

$directory = Split-Path -Parent $Output
if (-not (Test-Path -LiteralPath $directory)) {
    New-Item -ItemType Directory -Path $directory | Out-Null
}
[System.IO.File]::WriteAllBytes($Output, $stream.ToArray())
Write-Host ("{0}: {1} bytes, {2} px" -f $Output, $stream.Length, ($sizes -join ', '))
