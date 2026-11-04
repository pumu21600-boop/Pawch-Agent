@echo off
rem Build the desktop shell and give it the user-facing name "Pawch Agent.exe".
rem Why a rename: cargo's crate/target name cannot contain a space (rustc rejects
rem "invalid character ' ' in crate name"), so the build output stays
rem pawch-agent-desktop.exe and we move it afterwards. Keep it ASCII-only.
rem Usage: scripts\build-desktop.bat   (stop the shell first, the exe is locked while running)
setlocal
cd /d %~dp0..
set "OUT=desktop\target\release\Pawch Agent.exe"
if exist "%OUT%" del "%OUT%"
pushd desktop
cargo build --release
set "RC=%ERRORLEVEL%"
popd
if not "%RC%"=="0" (
  echo [build-desktop] cargo build failed ^(%RC%^)
  exit /b %RC%
)
move /Y "desktop\target\release\pawch-agent-desktop.exe" "%OUT%" >nul
if errorlevel 1 (
  echo [build-desktop] rename failed - is the shell still running?
  exit /b 1
)
echo [build-desktop] ok -^> %OUT%
