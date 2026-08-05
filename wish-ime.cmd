@echo off
setlocal

set "wishctl=%~dp0target\release\wishctl.exe"
set "wishpad=%~dp0target\release\wishpad.exe"
set "wish_root=%~dp0.local\tsf-alpha\user-data\wishes"

if "%~1"=="" (
    if not exist "%wishpad%" (
        echo Wish manager is missing. Run cargo build --release --bin wishpad first.
        exit /b 1
    )
    start "" "%wishpad%"
    exit /b 0
)

if not exist "%wishctl%" (
    echo Wish manager is missing. Run cargo build --release --bin wishctl first.
    exit /b 1
)

"%wishctl%" %* --root "%wish_root%"
exit /b %ERRORLEVEL%
