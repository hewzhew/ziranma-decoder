@echo off
setlocal

set "aliasctl=%~dp0target\release\aliasctl.exe"
set "alias_root=%~dp0.local\tsf-alpha\user-data\aliases"

if not exist "%aliasctl%" (
    echo Alias manager is missing. Run cargo build --release --bin aliasctl first.
    exit /b 1
)

"%aliasctl%" %* --root "%alias_root%"
exit /b %ERRORLEVEL%
