# sign-release.ps1 - Authenticode-sign release binaries.
#
# Requires a code-signing certificate (not included):
#   - OV/EV certificate bought from a CA (hardware token / PFX), or
#   - Azure Trusted Signing, or
#   - a self-signed cert for LOCAL TESTING only (not trusted by other machines).
#
# Usage:
#   # cert already imported into CurrentUser\My (recommended: hardware token / CA cert)
#   powershell -File scripts\sign-release.ps1 -Thumbprint <sha1>
#
#   # sign a PFX directly (password via env PAWCH_SIGN_PASSWORD)
#   $env:PAWCH_SIGN_PASSWORD = "..."
#   powershell -File scripts\sign-release.ps1 -Pfx C:\path\codesign.pfx
#
#   # sign arbitrary files (testing)
#   powershell -File scripts\sign-release.ps1 -Thumbprint <sha1> -Path C:\tmp\a.exe
#
# Order matters: sign the release exes BEFORE zipping payload.zip, then sign Setup.exe.
# build-installer.ps1 accepts -SignThumbprint / -SignPfx to do this automatically.

param(
    [string]$Thumbprint = "",
    [string]$Pfx = "",
    [string]$Password = "",
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [string[]]$Path = @()
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Info($m) { Write-Host "[sign] $m" -ForegroundColor Cyan }
function Die($m) { Write-Host "[sign] ERROR: $m" -ForegroundColor Red; exit 1 }

if (-not $Thumbprint -and -not $Pfx) { Die "provide -Thumbprint or -Pfx (a code-signing certificate is required)" }
if ($Pfx -and -not $Password) { $Password = $env:PAWCH_SIGN_PASSWORD }
if ($Pfx -and -not (Test-Path $Pfx)) { Die "PFX not found: $Pfx" }
if ($Pfx -and -not $Password) { Die "PFX password missing (set PAWCH_SIGN_PASSWORD or -Password)" }

# newest Windows SDK signtool
$signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if (-not $signtool) {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { $signtool = $cmd.Source }
}
if (-not $signtool) { Die "signtool.exe not found (install Windows SDK)" }
Info "signtool: $signtool"

$targets = @()
if ($Path.Count -gt 0) {
    $targets = $Path | Where-Object { Test-Path $_ }
} else {
    foreach ($p in @(
        (Join-Path $root "dist\Pawch Agent\pawch-agent.exe"),
        (Join-Path $root "dist\Pawch Agent\Pawch Agent.exe"),
        (Join-Path $root "dist-installer\Pawch Agent Setup\Pawch Agent Setup.exe")
    )) {
        if (Test-Path $p) { $targets += $p }
    }
}
if ($targets.Count -eq 0) { Die "no files to sign (build release first, or pass -Path)" }

$failed = 0
foreach ($f in $targets) {
    Info "sign $f"
    if ($Pfx) {
        & $signtool sign /fd sha256 /td sha256 /tr $TimestampUrl /f $Pfx /p $Password $f
    } else {
        & $signtool sign /fd sha256 /td sha256 /tr $TimestampUrl /sha1 $Thumbprint $f
    }
    if ($LASTEXITCODE -ne 0) { $failed++; Write-Host "[sign] FAILED: $f" -ForegroundColor Red; continue }

    # /pa = default Authenticode policy; a self-signed cert will fail here on other
    # machines until its root is trusted - that is expected and reported honestly.
    & $signtool verify /pa $f | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Info "verified: $f"
    } else {
        Write-Host "[sign] WARN: signature present but not trusted by this machine (self-signed?): $f" -ForegroundColor Yellow
    }
}

if ($failed -gt 0) { exit 1 }
Info "done"
