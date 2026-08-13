# Regenerates assets/tray.ico and assets/tray-preview.png: a stylized side
# scroll wheel (rounded pill + ridge notches) with up/down chevrons, on a
# dark-slate circle. Run from the repo root with Windows PowerShell/pwsh;
# requires .NET's System.Drawing (present on Windows by default).

Add-Type -AssemblyName System.Drawing

$size = 64
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

$bg = [System.Drawing.Color]::FromArgb(255, 38, 47, 61)
$bgBrush = New-Object System.Drawing.SolidBrush($bg)
$g.FillEllipse($bgBrush, 2, 2, $size - 4, $size - 4)

$fg = [System.Drawing.Color]::FromArgb(255, 240, 244, 248)
$fgBrush = New-Object System.Drawing.SolidBrush($fg)
$fgPen = New-Object System.Drawing.Pen($fg, 3)
$fgPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$fgPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$fgPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round

# Side scroll wheel: a vertical rounded "pill" (the wheel, seen edge-on,
# like the MX Master's thumb wheel).
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$path.AddArc(26, 14, 12, 12, 180, 180)
$path.AddArc(26, 38, 12, 12, 0, 180)
$path.CloseFigure()
$g.FillPath($fgBrush, $path)

# Ridge notches (cut into the pill as thin background-colored bars) so the
# wheel reads as textured/scrollable, not just a plain capsule.
$notchPen = New-Object System.Drawing.Pen($bg, 2)
$g.DrawLine($notchPen, 28, 22, 36, 22)
$g.DrawLine($notchPen, 28, 28, 36, 28)
$g.DrawLine($notchPen, 28, 34, 36, 34)
$g.DrawLine($notchPen, 28, 40, 36, 40)

# Small up/down chevrons beside the wheel, hinting at the scroll motion
# this glyph maps to.
[System.Drawing.Point[]]$up = @(
    New-Object System.Drawing.Point(44, 24)
    New-Object System.Drawing.Point(48, 18)
    New-Object System.Drawing.Point(52, 24)
)
[System.Drawing.Point[]]$down = @(
    New-Object System.Drawing.Point(44, 40)
    New-Object System.Drawing.Point(48, 46)
    New-Object System.Drawing.Point(52, 40)
)
$g.DrawLines($fgPen, $up)
$g.DrawLines($fgPen, $down)

$g.Dispose()

$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$icon = [System.Drawing.Icon]::FromHandle($bmp.GetHicon())
$fs = New-Object System.IO.FileStream("$dir\tray.ico", [System.IO.FileMode]::Create)
$icon.Save($fs)
$fs.Close()
$bmp.Save("$dir\tray-preview.png")

Write-Output "Wrote $dir\tray.ico and $dir\tray-preview.png"
