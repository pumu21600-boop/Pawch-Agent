@echo off
rem Run cargo test inside the VS 2022 developer environment.
rem Why: Git Bash's /usr/bin/link.exe is coreutils "link" and hijacks MSVC link.exe
rem ("missing operand after BOM" link failure); build scripts also need cl.exe.
rem This machine has no vswhere; the VS path is hardcoded below.
rem Usage: scripts\run-tests.bat [cargo test args, e.g. gateway::]
setlocal
set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
  echo [run-tests] vcvars64.bat not found - update the path in this script
  exit /b 1
)
call "%VCVARS%" >nul
cd /d %~dp0..
if not defined PAWCH_TARGET_DIR set "PAWCH_TARGET_DIR=D:\pawch-build\target"
set "CARGO_TARGET_DIR=%PAWCH_TARGET_DIR%"
cargo test --bin pawch-agent %*
