@echo off
setlocal

set "aliasctl=%~dp0target\release\aliasctl.exe"
set "aliaspad=%~dp0target\release\aliaspad.exe"
set "alias_root=%~dp0.local\tsf-alpha\user-data\aliases"

if "%~1"=="" (
    if not exist "%aliaspad%" (
        echo Candidate pin panel is missing. Run cargo build --release --bin aliasctl --bin aliaspad first.
        exit /b 1
    )
    start "" "%aliaspad%"
    exit /b 0
)

if not exist "%aliasctl%" (
    echo Alias manager is missing. Run cargo build --release --bin aliasctl first.
    exit /b 1
)

"%aliasctl%" %* --root "%alias_root%"
exit /b %ERRORLEVEL%
