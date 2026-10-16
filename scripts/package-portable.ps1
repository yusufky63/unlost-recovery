# Collects release files into .\release: the NSIS setup, the MSI packages, a portable zip
# (exe + "portable" marker) and SHA256SUMS.txt. Run after `pnpm app:build`.
# Spaces become dashes so GitHub keeps the file names as they are.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -LiteralPath (Join-Path $root 'package.json') -Raw | ConvertFrom-Json).version
$exe = Join-Path $root 'target\release\unlost.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "Release exe not found. Run: pnpm app:build" }

$out = Join-Path $root 'release'
$stage = Join-Path $out 'portable'
if (Test-Path -LiteralPath $out) { Remove-Item -LiteralPath $out -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'Unlost.exe')
Set-Content -LiteralPath (Join-Path $stage 'portable') -Encoding ascii -Value 'Unlost keeps its settings and WebView2 profile in the data folder next to the exe while this file exists, so running it from a USB stick keeps its writes off the disk you recover from.'
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')

$zip = Join-Path $out "Unlost_${version}_x64_portable.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force
Remove-Item -LiteralPath $stage -Recurse -Force

$bundle = Join-Path $root 'target\release\bundle'
Get-ChildItem -Path (Join-Path $bundle 'nsis\*.exe'), (Join-Path $bundle 'msi\*.msi') -ErrorAction SilentlyContinue |
  ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $out ($_.Name -replace ' ', '-')) }

$sums = Get-ChildItem -LiteralPath $out -File | Sort-Object Name | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
}
Set-Content -LiteralPath (Join-Path $out 'SHA256SUMS.txt') -Value $sums -Encoding ascii
Get-ChildItem -LiteralPath $out | Format-Table Name, Length -AutoSize
