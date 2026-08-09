@echo off
setlocal

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
set "research_root=%~dp0.local\tsf-alpha\user-data\research-inbox"
set "action=%~1"

if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)
set "researchctl="
for /f "usebackq delims=" %%P in (`call "%resolver%" researchctl`) do set "researchctl=%%P"
if not defined researchctl (
    echo Research settings are unavailable. Run refresh-ime.cmd first.
    exit /b 1
)
if not exist "%researchctl%" (
    echo Research settings are unavailable. Run refresh-ime.cmd status.
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

if /i "%action%"=="review" (
    "%researchctl%" review --confirm-show-private-text --root "%research_root%"
    exit /b %ERRORLEVEL%
)

echo Usage: research-ime.cmd [status^|on^|off^|review]
exit /b 2
