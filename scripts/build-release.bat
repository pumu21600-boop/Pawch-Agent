@echo off
rem Build the release binary into %PAWCH_TARGET_DIR%\release (the path start.bat runs).
rem Default target dir is D:\pawch-build\target so C: does not fill up; override with
rem the PAWCH_TARGET_DIR environment variable.
rem Needs the VS 2022 dev environment for link.exe/cl.exe - see run-tests.bat.
rem Usage: scripts\build-release.bat
setlocal
set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
  echo [build-release] vcvars64.bat not found - update the path in this script
  exit /b 1
)
call "%VCVARS%" >nul
cd /d %~dp0..
if not defined PAWCH_TARGET_DIR set "PAWCH_TARGET_DIR=D:\pawch-build\target"
set "CARGO_TARGET_DIR=%PAWCH_TARGET_DIR%"
cargo build --release --bin pawch-agent
