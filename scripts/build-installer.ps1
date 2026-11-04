# build-installer.ps1 - build the Pawch Agent installer (Setup.exe + payload.zip).
#
# Prerequisites (already built):
#   target\release\pawch-agent.exe
#   desktop\target\release\Pawch Agent.exe      (optional; service-only without it)
#   web\dist\index.html
#
# Output:
#   dist-installer\Pawch Agent Setup\Pawch Agent Setup.exe
#   dist-installer\Pawch Agent Setup\payload.zip
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -SkipPackage
#   # signed build (cert already in CurrentUser\My):
#   powershell -File scripts\build-installer.ps1 -SignThumbprint <sha1>
#   # signed build with PFX (password via PAWCH_SIGN_PASSWORD):
#   powershell -File scripts\build-installer.ps1 -SignPfx C:\path\codesign.pfx

param(
    [switch]$SkipPackage,
    [string]$SignThumbprint = "",
    [string]$SignPfx = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Info($m) { Write-Host "[installer] $m" -ForegroundColor Cyan }
function Die($m) { Write-Host "[installer] ERROR: $m" -ForegroundColor Red; exit 1 }

$svc = Join-Path $root "target\release\pawch-agent.exe"
if (-not (Test-Path $svc)) { Die "target\release\pawch-agent.exe missing (cargo build --release)" }
if (-not (Test-Path (Join-Path $root "web\dist\index.html"))) { Die "web\dist\index.html missing (cd web; npm run build)" }
$shell = Join-Path $root "desktop\target\release\Pawch Agent.exe"
if (-not (Test-Path $shell)) { Write-Host "[installer] WARN: desktop shell missing - installer will be service-only" -ForegroundColor Yellow }

if (-not $SkipPackage) {
    Info "package release folder"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "package_release.ps1") -SkipBuild -SkipFrontend -SkipZip
    if ($LASTEXITCODE -ne 0) { Die "package_release.ps1 failed" }
}

$out = Join-Path $root "dist\Pawch Agent"
if (-not (Test-Path (Join-Path $out "pawch-agent.exe"))) { Die "release folder missing: $out" }

# Sign the release exes BEFORE zipping payload, so the installed binaries are signed too.
$signArgs = @()
if ($SignThumbprint) { $signArgs = @("-Thumbprint", $SignThumbprint) }
elseif ($SignPfx) { $signArgs = @("-Pfx", $SignPfx) }
if ($signArgs.Count -gt 0) {
    Info "sign release binaries"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "sign-release.ps1") @signArgs
    if ($LASTEXITCODE -ne 0) { Die "sign-release.ps1 failed (release)" }
}

$payload = Join-Path $root "installer\payload.zip"
if (Test-Path $payload) { Remove-Item $payload -Force }
Info "zip payload"
Get-ChildItem -LiteralPath $out -Force | Compress-Archive -DestinationPath $payload -CompressionLevel Optimal
if (-not (Test-Path $payload)) { Die "payload.zip not created" }

Info "build setup.exe"
Push-Location (Join-Path $root "installer")
cargo build --release
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { Die "cargo build (installer) failed" }

$setupSrc = Join-Path $root "installer\target\release\pawch-setup.exe"
if (-not (Test-Path $setupSrc)) { Die "pawch-setup.exe missing" }

$dest = Join-Path $root "dist-installer\Pawch Agent Setup"
if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
New-Item -ItemType Directory -Path $dest -Force | Out-Null

# Embed payload into setup.exe (single-file distribution, 2026-11-04 用户拍板):
# append zip bytes + u64 LE length + "PAWCH1" trailer; the installer reads its
# own tail. --payload <zip> and legacy side-by-side payload.zip still honored.
$setupOut = Join-Path $dest "Pawch Agent Setup.exe"
Copy-Item $setupSrc $setupOut
$bytes = [System.IO.File]::ReadAllBytes($setupSrc)
$zipBytes = [System.IO.File]::ReadAllBytes($payload)
$ms = New-Object System.IO.MemoryStream
$ms.Write($bytes, 0, $bytes.Length)
$ms.Write($zipBytes, 0, $zipBytes.Length)
$ms.Write([BitConverter]::GetBytes([UInt64]$zipBytes.Length), 0, 8)
$magic = [System.Text.Encoding]::ASCII.GetBytes("PAWCH1")
$ms.Write($magic, 0, $magic.Length)
[System.IO.File]::WriteAllBytes($setupOut, $ms.ToArray())
$ms.Dispose()
Info ("payload embedded: {0:N1} MB" -f ($zipBytes.Length / 1MB))

# Sign the setup exe last (embedding first keeps the trailer intact through signing; payload already contains signed app binaries).
if ($signArgs.Count -gt 0) {
    Info "sign setup.exe"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "sign-release.ps1") @signArgs -Path $setupOut
    if ($LASTEXITCODE -ne 0) { Die "sign-release.ps1 failed (setup)" }
}

Info "done -> $dest"
Info ("single-file setup: {0:N1} MB (payload {1:N1} MB embedded)" -f ((Get-Item $setupOut).Length / 1MB), ($zipBytes.Length / 1MB))

# Distribution zip wraps the single Setup.exe.
$zipOut = Join-Path $root "dist-installer\Pawch Agent Setup.zip"
if (Test-Path $zipOut) { Remove-Item $zipOut -Force }
Compress-Archive -Path $setupOut -DestinationPath $zipOut -CompressionLevel Optimal
Info "zip -> $zipOut"
