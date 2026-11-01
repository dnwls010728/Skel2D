# Builds the Windows distribution: dist\Skel2D-<version>-win64 and a .zip of it.
# Run from anywhere: powershell -ExecutionPolicy Bypass -File scripts\make_dist.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$name = "Skel2D-$version-win64"
$out = Join-Path $root "dist\$name"

cargo build --profile dist -p skel2d-cli -p skel2d-editor
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out, "$out\docs", "$out\examples" | Out-Null

Copy-Item target\dist\skel2d-editor.exe, target\dist\skel2d.exe $out
Copy-Item docs\dist\README.txt $out
Copy-Item docs\AI_GUIDE.md "$out\docs"
$manual = Get-ChildItem docs\manual -Filter 'Skel2D_*.pdf' | Select-Object -First 1
if (-not $manual) { throw 'manual PDF is missing: build docs\manual\pdf first' }
# An ASCII name: zip tools disagree on the encoding of Korean file names.
Copy-Item $manual.FullName "$out\Skel2D_Manual_ko.pdf"

# miya is left out: its PSD is a working file, not sample material meant for release.
foreach ($example in 'spineboy', 'celestial-circus', 'skins', 'face', 'walker', 'arm', 'rig', 'clipping') {
    if (-not (Test-Path "examples\$example")) { throw "examples\$example is missing" }
    Copy-Item -Recurse "examples\$example" "$out\examples\$example"
}

# Every example must open and find all its images from the packaged folder.
Get-ChildItem "$out\examples" -Recurse -Filter '*.skel.json' | ForEach-Object {
    $check = & "$out\skel2d.exe" validate $_.FullName | ConvertFrom-Json
    if (-not $check.valid) { throw "$($_.Name) does not validate" }
    $render = & "$out\skel2d.exe" render $_.FullName -o "$env:TEMP\skel2d_dist_check.png" | ConvertFrom-Json
    if ($render.missing_images.Count -gt 0) { throw "$($_.Name) misses images: $($render.missing_images -join ', ')" }
}
Remove-Item "$env:TEMP\skel2d_dist_check.png" -ErrorAction SilentlyContinue

$zip = "$out.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
# tar, not Compress-Archive: that writes backslash paths other systems cannot unpack.
& "$env:SystemRoot\System32\tar.exe" -a -c -f $zip -C (Split-Path $out -Parent) $name
if ($LASTEXITCODE -ne 0) { throw 'zip failed' }
Write-Output $zip
