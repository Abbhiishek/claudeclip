<#
.SYNOPSIS
  End-to-end test for ClaudeClip. MUST be run in an STA PowerShell host
  (clipboard image APIs require STA): powershell.exe -STA -File integration-test.ps1

  Puts a synthetic bitmap on the clipboard, then waits for ClaudeClip (assumed
  already running) to (a) save a PNG and (b) place its path on the clipboard as
  text while keeping the image. Exits 0 on PASS, non-zero on FAIL.
#>
param(
    [int]$TimeoutSeconds = 8,
    [int]$Width = 123,
    [int]$Height = 77
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$saveDir = Join-Path ([Environment]::GetFolderPath('MyPictures')) 'ClaudeClips'
Write-Output "save_dir = $saveDir"

# --- put a synthetic bitmap on the clipboard -------------------------------
$bmp = New-Object System.Drawing.Bitmap($Width, $Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::FromArgb(255, 40, 120, 220))
$brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::Orange)
$g.FillRectangle($brush, 10, 10, 40, 30)
$g.Dispose()

[System.Windows.Forms.Clipboard]::SetImage($bmp)
Write-Output "placed ${Width}x${Height} bitmap on clipboard"

# --- poll for the path text ------------------------------------------------
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$pathText = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $t = [System.Windows.Forms.Clipboard]::GetText()
    if ($t -and ($t -match '\.png$')) {
        $pathText = $t.Trim()
        break
    }
}

if (-not $pathText) {
    Write-Output "FAIL: no path text appeared on the clipboard within $TimeoutSeconds s"
    exit 1
}
Write-Output "clipboard text = $pathText"

# Normalize (we emit forward slashes in 'plain' mode).
$winPath = $pathText.Trim('"') -replace '/', '\'

if (-not (Test-Path -LiteralPath $winPath)) {
    Write-Output "FAIL: file does not exist: $winPath"
    exit 1
}

# --- validate PNG signature ------------------------------------------------
$bytes = [System.IO.File]::ReadAllBytes($winPath)
$sig = @(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)
for ($i = 0; $i -lt 8; $i++) {
    if ($bytes[$i] -ne $sig[$i]) {
        Write-Output "FAIL: not a PNG (bad signature at byte $i)"
        exit 1
    }
}

# --- validate dimensions ---------------------------------------------------
$ms = New-Object System.IO.MemoryStream(,$bytes)
$img = [System.Drawing.Image]::FromStream($ms)
$w = $img.Width; $h = $img.Height
$img.Dispose(); $ms.Dispose()
if ($w -ne $Width -or $h -ne $Height) {
    Write-Output "FAIL: dimensions ${w}x${h}, expected ${Width}x${Height}"
    exit 1
}
Write-Output "png ok: ${w}x${h}, $($bytes.Length) bytes"

# --- confirm the image is STILL on the clipboard (keep_image) --------------
$stillImage = [System.Windows.Forms.Clipboard]::ContainsImage()
if (-not $stillImage) {
    Write-Output "FAIL: image no longer on clipboard (keep_image broken)"
    exit 1
}
Write-Output "image retained on clipboard: yes"

Write-Output "PASS"
exit 0
