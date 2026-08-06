@echo off
setlocal

set "researchctl=%~dp0target\release\researchctl.exe"
set "research_root=%~dp0.local\tsf-alpha\user-data\research-inbox"
set "action=%~1"

if not exist "%researchctl%" (
    echo Research settings are missing. Run cargo build --release --bin researchctl first.
    exit /b 1
)

if "%action%"=="" set "action=status"

if /i "%action%"=="status" (
    "%researchctl%" status --root "%research_root%"
    exit /b %ERRORLEVEL%
)

if /i "%action%"=="on" (
    "%researchctl%" enable --confirm-continuous-private-feedback --root "%research_root%"
    exit /b %ERRORLEVEL%
)

if /i "%action%"=="off" (
    "%researchctl%" disable --root "%research_root%"
    exit /b %ERRORLEVEL%
)

echo Usage: research-ime.cmd [status^|on^|off]
exit /b 2
