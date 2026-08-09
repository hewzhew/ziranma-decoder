@echo off
setlocal

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
set "personal_root=%~dp0.local\tsf-alpha\user-data\personal-ranking"
if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)

set "personalctl="
for /f "usebackq delims=" %%P in (`call "%resolver%" personalctl`) do set "personalctl=%%P"
if not defined personalctl (
    echo Personal ranking manager is unavailable. Run refresh-ime.cmd first.
    exit /b 1
)
if not exist "%personalctl%" (
    echo Personal ranking manager is unavailable. Run refresh-ime.cmd status.
    exit /b 1
)

if "%~1"=="" goto status

"%personalctl%" %* --root "%personal_root%"
exit /b %ERRORLEVEL%

:status
"%personalctl%" status --root "%personal_root%"
exit /b %ERRORLEVEL%
