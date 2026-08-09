@echo off
setlocal

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
set "alias_root=%~dp0.local\tsf-alpha\user-data\aliases"

if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)

if "%~1"=="" (
    set "aliaspad="
    for /f "usebackq delims=" %%P in (`call "%resolver%" aliaspad`) do set "aliaspad=%%P"
    if not defined aliaspad (
        echo Candidate pin panel is unavailable. Run refresh-ime.cmd first.
        exit /b 1
    )
    if not exist "%aliaspad%" (
        echo Candidate pin panel is unavailable. Run refresh-ime.cmd status.
        exit /b 1
    )
    start "" "%aliaspad%"
    exit /b 0
)

set "aliasctl="
for /f "usebackq delims=" %%P in (`call "%resolver%" aliasctl`) do set "aliasctl=%%P"
if not defined aliasctl (
    echo Alias manager is unavailable. Run refresh-ime.cmd first.
    exit /b 1
)
if not exist "%aliasctl%" (
    echo Alias manager is unavailable. Run refresh-ime.cmd status.
    exit /b 1
)

"%aliasctl%" %* --root "%alias_root%"
exit /b %ERRORLEVEL%
