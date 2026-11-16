$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$destination = Join-Path $PSScriptRoot '..\plugins\clipboard-history\Images'
[void][IO.Directory]::CreateDirectory($destination)

# Vector geometry is rendered at 4x resolution for clean edges at every ICO size.
function New-ClipboardIcon([string]$Kind, [string]$Accent, [string]$Background) {
    $frames = @()
    foreach ($size in @(16, 24, 32, 48, 64)) {
        $canvas = [Drawing.Bitmap]::new($size * 4, $size * 4)
        $graphics = [Drawing.Graphics]::FromImage($canvas)
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.ScaleTransform($size / 8.0, $size / 8.0)
        $ink = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($Accent))
        $paper = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($Background))
        $white = [Drawing.SolidBrush]::new([Drawing.Color]::White)
        $pen = [Drawing.Pen]::new($ink, 1.8)
        $pen.StartCap = $pen.EndCap = [Drawing.Drawing2D.LineCap]::Round
        $tile = [Drawing.Drawing2D.GraphicsPath]::new()
        $tile.AddArc(1, 1, 10, 10, 180, 90)
        $tile.AddArc(21, 1, 10, 10, 270, 90)
        $tile.AddArc(21, 21, 10, 10, 0, 90)
        $tile.AddArc(1, 21, 10, 10, 90, 90)
        $tile.CloseFigure()
        $graphics.FillPath($paper, $tile)
        switch ($Kind) {
            'text' {
                $outline = [Drawing.PointF[]]@([Drawing.PointF]::new(9,6), [Drawing.PointF]::new(19,6), [Drawing.PointF]::new(24,11), [Drawing.PointF]::new(24,26), [Drawing.PointF]::new(9,26))
                $graphics.FillPolygon($white, $outline)
                $graphics.DrawPolygon($pen, $outline)
                $graphics.DrawLines($pen, [Drawing.PointF[]]@([Drawing.PointF]::new(19,6), [Drawing.PointF]::new(19,11), [Drawing.PointF]::new(24,11)))
                $graphics.DrawLine($pen, 13, 15, 20, 15)
                $graphics.DrawLine($pen, 13, 19, 20, 19)
                $graphics.DrawLine($pen, 13, 23, 17, 23)
            }
            'image' {
                $graphics.FillRectangle($white, 6, 8, 20, 17)
                $graphics.DrawRectangle($pen, 6, 8, 20, 17)
                $graphics.FillEllipse($ink, 19, 11, 4, 4)
                $graphics.FillPolygon($ink, [Drawing.PointF[]]@([Drawing.PointF]::new(8,23), [Drawing.PointF]::new(14,15), [Drawing.PointF]::new(19,21), [Drawing.PointF]::new(22,18), [Drawing.PointF]::new(25,23)))
            }
            'files' {
                $graphics.FillPolygon($ink, [Drawing.PointF[]]@([Drawing.PointF]::new(5,9), [Drawing.PointF]::new(14,9), [Drawing.PointF]::new(17,12), [Drawing.PointF]::new(27,12), [Drawing.PointF]::new(27,25), [Drawing.PointF]::new(5,25)))
                $graphics.FillRectangle($white, 7, 14, 18, 9)
                $graphics.FillRectangle($ink, 7, 17, 18, 6)
            }
        }
        $bitmap = [Drawing.Bitmap]::new($size, $size)
        $output = [Drawing.Graphics]::FromImage($bitmap)
        $output.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $output.DrawImage($canvas, 0, 0, $size, $size)
        $stream = [IO.MemoryStream]::new()
        $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
        $frames += @{ Size = $size; Bytes = $stream.ToArray() }
        $stream.Dispose(); $output.Dispose(); $bitmap.Dispose()
        $tile.Dispose(); $pen.Dispose(); $white.Dispose(); $paper.Dispose(); $ink.Dispose()
        $graphics.Dispose(); $canvas.Dispose()
    }
    $file = [IO.File]::Create((Join-Path $destination "$Kind.ico"))
    $writer = [IO.BinaryWriter]::new($file)
    try {
        $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$frames.Count)
        $offset = 6 + 16 * $frames.Count
        foreach ($frame in $frames) {
            $writer.Write([byte]$frame.Size); $writer.Write([byte]$frame.Size)
            $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]32)
            $writer.Write([uint32]$frame.Bytes.Length); $writer.Write([uint32]$offset)
            $offset += $frame.Bytes.Length
        }
        foreach ($frame in $frames) { $writer.Write([byte[]]$frame.Bytes) }
    } finally { $writer.Dispose(); $file.Dispose() }
}

New-ClipboardIcon text '#2563EB' '#EAF1FF'
New-ClipboardIcon image '#087F8C' '#E5F7F5'
New-ClipboardIcon files '#B77910' '#FFF3D6'
