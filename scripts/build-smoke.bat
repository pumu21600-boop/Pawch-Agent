@echo off
rem Build the release smoke binary into %PAWCH_TARGET_DIR%-smoke (keeps the normal
rem target dir and the running service's exe untouched). Default is
rem D:\pawch-build\target-smoke; override with PAWCH_TARGET_DIR.
rem Needs the VS 2022 dev environment for link.exe/cl.exe - see run-tests.bat for why.
rem Usage: scripts\build-smoke.bat
setlocal
set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
  echo [build-smoke] vcvars64.bat not found - update the path in this script
  exit /b 1
)
call "%VCVARS%" >nul
cd /d %~dp0..
if not defined PAWCH_TARGET_DIR set "PAWCH_TARGET_DIR=D:\pawch-build\target"
set "CARGO_TARGET_DIR=%PAWCH_TARGET_DIR%-smoke"
cargo build --release --bin pawch-agent
