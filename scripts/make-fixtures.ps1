# Builds the VHDX scenario fixtures used by crates/core/tests/vhdx_scenarios.rs.
#
# Run from an elevated PowerShell (diskpart needs administrator rights):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\make-fixtures.ps1
#
# For each scenario it creates a small dynamic VHDX in .\fixtures, formats it NTFS with
# Windows itself, writes deterministic files, deletes some, detaches the disk and writes a
# manifest (path, size, SHA-256, what the scanner should report). Nothing outside .\fixtures is
# touched, except with -DisableTrimWhileBuilding (meant for throwaway CI machines): it sets the
# global NTFS DisableDeleteNotify to 1 while the images are built and restores the old value.
# Without it, Windows may TRIM freed clusters inside the virtual disk, which is real-world
# behaviour the tests accept ("intact or zeroed") for non-resident files.

param(
  [string]$Out = (Join-Path (Split-Path -Parent $PSScriptRoot) 'fixtures'),
  [switch]$DisableTrimWhileBuilding
)
$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Run this script from an elevated PowerShell: diskpart needs administrator rights.'
}

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path

function Invoke-DiskPart([string[]]$Lines) {
  $script = Join-Path $env:TEMP ("unlost-dp-{0}.txt" -f [guid]::NewGuid())
  Set-Content -LiteralPath $script -Value $Lines -Encoding ascii
  try {
    $log = & diskpart.exe /s $script 2>&1
    if ($LASTEXITCODE -ne 0) { throw "diskpart failed:`n$($log -join "`n")" }
  } finally {
    Remove-Item -LiteralPath $script -Force -ErrorAction SilentlyContinue
  }
}

function New-Bytes([int]$Size, [int]$Seed, [byte[]]$Header) {
  $rng = New-Object System.Random($Seed)
  $b = New-Object byte[] $Size
  $rng.NextBytes($b)
  # No zero bytes, so "reads as zeros" can only mean the content is gone.
  for ($i = 0; $i -lt $Size; $i++) { if ($b[$i] -eq 0) { $b[$i] = 1 } }
  if ($Header) { [Array]::Copy($Header, $b, $Header.Length) }
  return ,$b
}

function Get-Sha256([byte[]]$Bytes) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes)) -replace '-', '').ToLowerInvariant() }
  finally { $sha.Dispose() }
}

function New-Scenario([string]$Name, [scriptblock]$Populate, [int]$SizeMB = 64) {
  $vhdx = Join-Path $Out "$Name.vhdx"
  $mnt = Join-Path $Out "mnt-$Name"
  if (Test-Path -LiteralPath $vhdx) { Remove-Item -LiteralPath $vhdx -Force }
  if (Test-Path -LiteralPath $mnt) { Remove-Item -LiteralPath $mnt -Recurse -Force }
  New-Item -ItemType Directory -Path $mnt | Out-Null
  Invoke-DiskPart @(
    "create vdisk file=""$vhdx"" maximum=$SizeMB type=expandable",
    "select vdisk file=""$vhdx""",
    'attach vdisk',
    'convert gpt',
    'create partition primary',
    'format fs=ntfs quick label=UNLOST',
    "assign mount=""$mnt"""
  )
  $files = New-Object System.Collections.ArrayList
  try {
    & $Populate $mnt $files
  } finally {
    # Detaching flushes the volume; the mount point goes first so no folder points at nothing.
    & mountvol.exe "$mnt\" /D | Out-Null
    Invoke-DiskPart @("select vdisk file=""$vhdx""", 'detach vdisk')
    Remove-Item -LiteralPath $mnt -Recurse -Force -ErrorAction SilentlyContinue
  }
  $manifest = [ordered]@{
    image = "$Name.vhdx"
    trimDisabled = [bool]$DisableTrimWhileBuilding
    files = @($files)
  }
  $json = $manifest | ConvertTo-Json -Depth 5
  [IO.File]::WriteAllText((Join-Path $Out "$Name.json"), $json, (New-Object Text.UTF8Encoding $false))
  Write-Host "built $vhdx ($($files.Count) files)"
}

function Add-File($Root, $Files, [string]$Rel, [byte[]]$Bytes, [bool]$Deleted, [string]$Expect) {
  $full = Join-Path $Root $Rel.TrimStart('\')
  [void][IO.Directory]::CreateDirectory("\\?\" + (Split-Path -Parent $full))
  [IO.File]::WriteAllBytes("\\?\$full", $Bytes)
  [void]$Files.Add([ordered]@{ path = $Rel; size = $Bytes.Length; sha256 = (Get-Sha256 $Bytes); deleted = $Deleted; expect = $Expect })
}

# Turkish names built from code points so this file stays ASCII (PowerShell 5.1 reads scripts
# without a BOM as ANSI).
$Sirket = "$([char]0x015E)irket Belgeleri"
$Cagri = "$([char]0x00C7)a$([char]0x011F)r$([char]0x0131) $([char]0x00D6)zet.docx"

$jpeg = [byte[]](0xFF, 0xD8, 0xFF, 0xE0)
$pdf = [Text.Encoding]::ASCII.GetBytes('%PDF-1.7')
$zip = [byte[]](0x50, 0x4B, 0x03, 0x04)

$oldTrim = $null
if ($DisableTrimWhileBuilding) {
  $oldTrim = ((fsutil behavior query DisableDeleteNotify NTFS) -replace '\D', '') | Select-Object -First 1
  fsutil behavior set DisableDeleteNotify NTFS 1 | Out-Null
}
try {
  New-Scenario 'ntfs-basic' {
    param($root, $files)
    $nonres = if ($DisableTrimWhileBuilding) { 'intact' } else { 'intactOrZeroed' }
    Add-File $root $files '\Docs\note.txt' ([Text.Encoding]::UTF8.GetBytes('Meeting notes. Keep these.')) $true 'intact'
    Add-File $root $files '\Docs\report.pdf' (New-Bytes 150000 1 $pdf) $true $nonres
    Add-File $root $files '\Photos\IMG_0001.jpg' (New-Bytes 2000000 2 $jpeg) $true $nonres
    Add-File $root $files "\$Sirket\$Cagri" (New-Bytes 80000 3 $zip) $true $nonres
    Add-File $root $files '\Old\Nested\deep.bin' (New-Bytes 300000 4 $null) $true $nonres
    Add-File $root $files '\big.bin' (New-Bytes 10000000 5 $null) $true $nonres
    Add-File $root $files '\keep\live.txt' (New-Bytes 5000 6 $null) $false 'live'
    $long = '\Long\' + ('a' * 120) + '\' + ('b' * 120) + '\file.txt'
    Add-File $root $files $long (New-Bytes 3000 7 $null) $true $nonres

    # Fragmentation: fill with small files, delete every other one, then write a file into the
    # gaps so it ends up in several runs.
    for ($i = 0; $i -lt 40; $i++) { [IO.File]::WriteAllBytes((Join-Path $root "gap$i.tmp"), (New-Bytes 65536 (100 + $i) $null)) }
    for ($i = 0; $i -lt 40; $i += 2) { Remove-Item -LiteralPath (Join-Path $root "gap$i.tmp") -Force }
    Add-File $root $files '\frag.bin' (New-Bytes 900000 8 $null) $true $nonres

    foreach ($f in @('\Docs\note.txt', '\Docs\report.pdf', '\Photos\IMG_0001.jpg', "\$Sirket\$Cagri", '\big.bin', '\frag.bin', $long)) {
      [IO.File]::Delete("\\?\" + (Join-Path $root $f.TrimStart('\')))
    }
    Remove-Item -LiteralPath (Join-Path $root 'Old') -Recurse -Force
  }

  New-Scenario 'ntfs-overwritten' {
    param($root, $files)
    Add-File $root $files '\victim.bin' (New-Bytes 5000000 20 $null) $true 'overwritten'
    Add-File $root $files '\small.txt' ([Text.Encoding]::UTF8.GetBytes('small resident file')) $true 'intact'
    # Create the filler's record first: NTFS reuses the lowest free record, and the deleted
    # files' records must survive so the scanner can report them.
    $fill = Join-Path $root 'filler.bin'
    [IO.File]::WriteAllBytes($fill, [byte[]]@())
    Remove-Item -LiteralPath (Join-Path $root 'victim.bin') -Force
    Remove-Item -LiteralPath (Join-Path $root 'small.txt') -Force
    # Then take every free cluster, so the victim's clusters are reallocated.
    $fs = [IO.File]::Open($fill, 'Append', 'Write')
    try {
      foreach ($size in @(1048576, 65536, 4096)) {
        $chunk = New-Bytes $size 21 $null
        while ($true) {
          try { $fs.Write($chunk, 0, $chunk.Length); $fs.Flush() } catch { break }
        }
      }
    } finally { try { $fs.Dispose() } catch { } }
  } 32
} finally {
  if ($DisableTrimWhileBuilding -and $null -ne $oldTrim) {
    fsutil behavior set DisableDeleteNotify NTFS $oldTrim | Out-Null
  }
}
Get-ChildItem -LiteralPath $Out | Format-Table Name, Length -AutoSize
