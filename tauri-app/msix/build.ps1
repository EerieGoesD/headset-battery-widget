# Packages the Tauri release build(s) into a Microsoft Store .msixbundle that
# reuses the existing Store identity (see AppxManifest.xml). Produces one .msix
# per architecture found, then bundles them. Output is UNSIGNED (Partner Center
# re-signs on submission), and a .msixbundle is required because the listing
# already shipped as a bundle.
#
# Prereqs (build the app first):
#   npm run tauri build -- --target x86_64-pc-windows-msvc  --no-bundle
#   npm run tauri build -- --target aarch64-pc-windows-msvc --no-bundle
$ErrorActionPreference = "Stop"

$root     = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo     = Resolve-Path (Join-Path $root "..\..")
$exeName  = "Headset Battery Widget.exe"
$makeappx = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\makeappx.exe"
$version  = "1.7.2.0"

# The x64 host build lands in target\release; arch-specific builds in target\<triple>\release.
$targets = @(
  @{ Arch = "x64";   Exe = Join-Path $repo "tauri-app\src-tauri\target\release\tauri-app.exe" }
  @{ Arch = "arm64"; Exe = Join-Path $repo "tauri-app\src-tauri\target\aarch64-pc-windows-msvc\release\tauri-app.exe" }
)

if (-not (Test-Path $makeappx)) { throw "makeappx not found: $makeappx" }

$manifestTemplate = Get-Content (Join-Path $root "AppxManifest.xml") -Raw

$dist = Join-Path $root "dist"
if (Test-Path $dist) { Remove-Item (Join-Path $dist "*") -Recurse -Force }
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$bundleInput = Join-Path $root "bundle_input"
if (Test-Path $bundleInput) { Remove-Item $bundleInput -Recurse -Force }
New-Item -ItemType Directory -Force -Path $bundleInput | Out-Null

$built = @()
foreach ($t in $targets) {
  if (-not (Test-Path $t.Exe)) { "SKIP $($t.Arch): exe not found ($($t.Exe))"; continue }

  $layout = Join-Path $root "layout_$($t.Arch)"
  if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $layout | Out-Null

  Copy-Item $t.Exe (Join-Path $layout $exeName) -Force
  Copy-Item (Join-Path $root "Images") (Join-Path $layout "Images") -Recurse -Force
  ($manifestTemplate -replace 'ProcessorArchitecture="[^"]*"', "ProcessorArchitecture=`"$($t.Arch)`"") |
    Set-Content -Path (Join-Path $layout "AppxManifest.xml") -Encoding UTF8

  $msix = Join-Path $bundleInput "HeadsetBatteryWidget_${version}_$($t.Arch).msix"
  & $makeappx pack /d $layout /p $msix /o
  if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed for $($t.Arch)" }
  $built += $t.Arch
}

if ($built.Count -eq 0) { throw "No architectures were packaged (no release exe found)." }

$bundle = Join-Path $dist "HeadsetBatteryWidget_$version.msixbundle"
& $makeappx bundle /d $bundleInput /p $bundle /bv $version /o
if ($LASTEXITCODE -ne 0) { throw "makeappx bundle failed with exit code $LASTEXITCODE" }

"BUILT BUNDLE ($($built -join ', ')): $bundle"
