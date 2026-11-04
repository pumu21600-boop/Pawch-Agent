@echo off
rem P20.4: prefer the desktop shell - the built-in browser runs as a child webview inside
rem the shell (no separate Edge process). The shell keeps the service alive itself
rem (spawns pawch-agent.exe if not running). Falls back to service-only mode if the shell
rem exe is not built; the browser panel then tells the user the shell is required.
rem NOTE: keep this file ASCII-only - cmd reads .bat in the OEM codepage and UTF-8
rem Chinese turns into garbage that gets executed as a command.
cd /d "%~dp0"
rem 壳的对外名字是 "Pawch Agent.exe"。cargo 的 crate 名不能带空格（rustc 限制），
rem 构建产物叫 pawch-agent-desktop.exe；scripts\build-desktop.bat 构建完会改名。
rem 这里顺手把还没改名的产物提上来，避免手动 cargo build 之后启动到旧二进制。
if exist "desktop\target\release\pawch-agent-desktop.exe" if not exist "desktop\target\release\Pawch Agent.exe" (
  move "desktop\target\release\pawch-agent-desktop.exe" "desktop\target\release\Pawch Agent.exe" >nul
)
if exist "desktop\target\release\Pawch Agent.exe" (
  start "" "desktop\target\release\Pawch Agent.exe"
  exit /b 0
)
if not exist "target\release\pawch-agent.exe" (
  echo [start] neither desktop shell nor pawch-agent.exe found. Run: cargo build --release
  pause
  exit /b 1
)
echo [start] desktop shell not built - falling back to service-only mode.
echo [start] build it with: scripts\build-desktop.bat
target\release\pawch-agent.exe start
