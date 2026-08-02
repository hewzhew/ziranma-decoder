@echo off
setlocal

set "wishctl=%~dp0target\release\wishctl.exe"
set "wish_root=%~dp0.local\tsf-alpha\user-data\wishes"

if not exist "%wishctl%" (
    echo Wish manager is missing. Run cargo build --release --bin wishctl first.
    exit /b 1
)

"%wishctl%" %* --root "%wish_root%"
exit /b %ERRORLEVEL%
