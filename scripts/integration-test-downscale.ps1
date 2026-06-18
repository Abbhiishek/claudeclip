<#
.SYNOPSIS
  E2E test for the max_dimension downscale. Requires a running instance whose
  config has `max_dimension = <MaxDim>`. Run in STA:
  powershell.exe -STA -File integration-test-downscale.ps1 -MaxDim 200

  Places a large bitmap on the clipboard and verifies the saved PNG's longer
  edge is capped while the aspect ratio is preserved.
#>
param([int]$SrcW = 600, [int]$SrcH = 400, [int]$MaxDim = 200, [int]$TimeoutSeconds = 8)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$bmp = New-Object System.Drawing.Bitmap($SrcW, $SrcH)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::FromArgb(255, 30, 160, 90))
$g.Dispose()
[System.Windows.Forms.Clipboard]::SetImage($bmp)
Write-Output "placed ${SrcW}x${SrcH} bitmap; expecting longer edge <= $MaxDim"

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$pathText = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $t = [System.Windows.Forms.Clipboard]::GetText()
    if ($t -and ($t -match '\.png$')) { $pathText = $t.Trim(); break }
}
if (-not $pathText) { Write-Output "FAIL: no path on clipboard"; exit 1 }

$winPath = $pathText.Trim('"') -replace '/', '\'
# The file is encoded on a background thread; give it a moment if needed.
$tries = 0
while (-not (Test-Path -LiteralPath $winPath) -and $tries -lt 20) { Start-Sleep -Milliseconds 100; $tries++ }
if (-not (Test-Path -LiteralPath $winPath)) { Write-Output "FAIL: file missing: $winPath"; exit 1 }

$bytes = [System.IO.File]::ReadAllBytes($winPath)
$ms = New-Object System.IO.MemoryStream(,$bytes)
$img = [System.Drawing.Image]::FromStream($ms)
$w = $img.Width; $h = $img.Height
$img.Dispose(); $ms.Dispose()
Write-Output "saved dimensions: ${w}x${h}"

$longest = [Math]::Max($w, $h)
if ($longest -gt $MaxDim) { Write-Output "FAIL: longer edge $longest > $MaxDim"; exit 1 }

# Aspect ratio preserved (within rounding).
$srcAspect = $SrcW / $SrcH
$dstAspect = $w / $h
if ([Math]::Abs($srcAspect - $dstAspect) -gt 0.05) {
    Write-Output "FAIL: aspect changed ($srcAspect -> $dstAspect)"
    exit 1
}
Write-Output "PASS"
exit 0
