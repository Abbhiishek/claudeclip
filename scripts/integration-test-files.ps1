<#
.SYNOPSIS
  E2E test for ClaudeClip's file-drop handling. Run in STA:
  powershell.exe -STA -File integration-test-files.ps1

  Copies a real file (as a file-drop list, like Explorer's Ctrl+C) and verifies
  ClaudeClip surfaces its path as clipboard text while keeping the drop list.
#>
param([int]$TimeoutSeconds = 8)

Add-Type -AssemblyName System.Windows.Forms

# Create a throwaway file to "copy".
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("claudeclip_test_{0}.mp4" -f ([guid]::NewGuid().ToString('N')))
[System.IO.File]::WriteAllBytes($tmp, (New-Object byte[] 1024))
Write-Output "test file = $tmp"

$col = New-Object System.Collections.Specialized.StringCollection
[void]$col.Add($tmp)
[System.Windows.Forms.Clipboard]::SetFileDropList($col)
Write-Output "placed file-drop list on clipboard"

$expected = ($tmp -replace '\\', '/')

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$got = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $t = [System.Windows.Forms.Clipboard]::GetText()
    if ($t) { $got = $t.Trim(); break }
}

if (-not $got) {
    Write-Output "FAIL: no text appeared on clipboard"
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    exit 1
}
Write-Output "clipboard text = $got"

if ($got -ne $expected) {
    Write-Output "FAIL: text '$got' != expected '$expected'"
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    exit 1
}

$stillDrop = [System.Windows.Forms.Clipboard]::ContainsFileDropList()
Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
if (-not $stillDrop) {
    Write-Output "FAIL: file-drop list was lost (Explorer paste would break)"
    exit 1
}
Write-Output "file-drop list retained: yes"
Write-Output "PASS"
exit 0
