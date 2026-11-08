# package_release.ps1 - build a self-contained Windows release folder (and zip).
#
# Layout produced (pawch-agent.exe resolves everything relative to its own directory):
#   Pawch Agent\
#     pawch-agent.exe      service binary (start/stop/status self-management built in)
#     config.toml          server/gateway/approval/sandbox/context/recovery/memory config
#     providers.json       provider presets
#     prompts\system.md    system conventions (edited from the UI)
#     web\dist\            built frontend (served by the exe)
#     plugins\             wasm/wat plugin examples (optional)
#     ui-ext\              UI slot extensions (optional, copied when present)
#     tools\git-runtime.zip bundled MinGit (optional; run scripts\bundle_git.ps1 first)
#     start.bat stop.bat   double-click entries
#     README.md LICENSE
#
# Two flavors:
#   full (default)  - ships tools\git-runtime.zip (bundled MinGit). On machines with no
#                     system git, checkpoints / undo / change list / review still work.
#   -Slim           - leaves that zip out (smaller download). On machines with no system
#                     git those features are unavailable and the UI says so with install
#                     instructions (no silent degradation).
# 2026-11-04 final: git is never bundled; -Slim is the only behavior (-Slim kept for compat, no difference).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\package_release.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\package_release.ps1 -SkipBuild -SkipZip
#   powershell -ExecutionPolicy Bypass -File scripts\package_release.ps1 -Slim
#
# NOTE: keep this file ASCII-only (PS 5.1 reads .ps1 in the system ANSI codepage).

param(
    [switch]$SkipBuild,
    [switch]$SkipFrontend,
    [switch]$SkipZip,
    [switch]$Slim,
    [string]$OutRoot = "dist"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Info($m) { Write-Host "[package] $m" -ForegroundColor Cyan }
function Warn($m) { Write-Host "[package] WARN: $m" -ForegroundColor Yellow }

# --- version from Cargo.toml (single source: the crate version) ---
$cargo = Get-Content (Join-Path $root "Cargo.toml") -TotalCount 6
$version = ($cargo | Select-String -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if (-not $version) { throw "cannot read version from Cargo.toml" }
Info "version = $version"

# --- 1. release binary ---
if (-not $SkipBuild) {
    Info "cargo build --release"
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
}
$exe = Join-Path $root "target\release\pawch-agent.exe"
if (-not (Test-Path $exe)) { throw "target\release\pawch-agent.exe not found (run without -SkipBuild)" }

# --- 2. frontend ---
$distSrc = Join-Path $root "web\dist"
if (-not $SkipFrontend) {
    Info "npm run build (web)"
    Push-Location (Join-Path $root "web")
    if (-not (Test-Path "node_modules")) { npm ci }
    npm run build
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) { throw "frontend build failed" }
}
if (-not (Test-Path (Join-Path $distSrc "index.html"))) { throw "web\dist\index.html missing" }

# --- 3. assemble ---
# 安装目录名：就叫 "Pawch Agent"，不带点、不带版本号。
# 压缩包以目录名命名，Windows「全部解压」会用它建同名文件夹。
$name = "Pawch Agent"
$out = Join-Path $root (Join-Path $OutRoot $name)
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Path $out -Force | Out-Null

Copy-Item $exe (Join-Path $out "pawch-agent.exe")
# 桌面壳（窗口/托盘/内置浏览器宿主）是主入口；构建过就随包带，没构建才退化为纯服务
$shell = Join-Path $root "desktop\target\release\Pawch Agent.exe"
if (Test-Path $shell) {
    Copy-Item $shell (Join-Path $out "Pawch Agent.exe")
    Info "desktop shell included"
} else {
    Warn "desktop shell not built - release runs service-only (run scripts\build-desktop.bat first)"
}
New-Item -ItemType Directory -Path (Join-Path $out "web") -Force | Out-Null
Copy-Item $distSrc (Join-Path $out "web\dist") -Recurse -Force

foreach ($item in @("config.toml", "providers.json", "README.md", "LICENSE")) {
    $p = Join-Path $root $item
    if (Test-Path $p) { Copy-Item $p $out } else { Warn "missing $item" }
}
# Installer reads this for the Control Panel "Programs and Features" version
Set-Content -Path (Join-Path $out "version.txt") -Value $version -Encoding ASCII
# skills/ 是全局技能根（app_root/skills，见 src/skills.rs::global_skills_root）——
# 漏掉它等于发行版比开发版少六个内置技能（docx/pdf/pptx/xlsx/git-commit/github）
foreach ($dir in @("prompts", "plugins", "ui-ext", "skills")) {
    $p = Join-Path $root $dir
    if (Test-Path $p) { Copy-Item $p (Join-Path $out $dir) -Recurse -Force }
    elseif ($dir -eq "skills") { Warn "missing skills/ - release build will ship without built-in skills" }
}

# git 一律不捆绑（2026-11-04 final）：安装包体积减半（38MB MinGit），
# 机器上有系统 git 就照常提供检查点/撤销/变更列表；没有时 UI 明说缺什么。
if (Test-Path (Join-Path $out "tools")) { Remove-Item (Join-Path $out "tools") -Recurse -Force }
Info "git not bundled: system git used when present"

# --- 4. launch entries (ASCII only) ---
$startBat = @"
@echo off
rem Prefer the desktop shell (window + tray + built-in browser host).
cd /d "%~dp0"
if exist "Pawch Agent.exe" (
  start "" "Pawch Agent.exe"
  exit /b 0
)
if not exist "pawch-agent.exe" (
  echo [start] pawch-agent.exe not found.
  pause
  exit /b 1
)
pawch-agent.exe start
"@
$stopBat = @"
@echo off
rem Stop the running agent service.
cd /d "%~dp0"
if not exist "pawch-agent.exe" (
  echo [stop] pawch-agent.exe not found. Nothing to stop.
  pause
  exit /b 1
)
pawch-agent.exe stop
pause
"@
Set-Content -Path (Join-Path $out "start.bat") -Value $startBat -Encoding ASCII
Set-Content -Path (Join-Path $out "stop.bat") -Value $stopBat -Encoding ASCII

# --- 5. zip ---
if (-not $SkipZip) {
    $zipOut = Join-Path $root (Join-Path $OutRoot "$name.zip")
    if (Test-Path $zipOut) { Remove-Item $zipOut -Force }
    Info "compressing $name.zip"
    Compress-Archive -Path (Join-Path $out "*") -DestinationPath $zipOut -CompressionLevel Optimal
    Info ("zip: {0:N1} MB" -f ((Get-Item $zipOut).Length / 1MB))
}

Info "done -> $out"
