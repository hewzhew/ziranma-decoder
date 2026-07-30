@echo off
setlocal

if not "%~1"=="" (
    echo Usage: update-ime.cmd
    exit /b 2
)

set "update_script=%~dp0scripts\replace-tsf-alpha.ps1"
if not exist "%update_script%" (
    echo Ziranma update script is missing.
    exit /b 1
)

set "windows_powershell=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%windows_powershell%" (
    echo Windows PowerShell is missing.
    exit /b 1
)

"%windows_powershell%" -NoProfile -ExecutionPolicy Bypass -File "%update_script%" -EnableCurrentUserAfterReplace
set "update_exit_code=%ERRORLEVEL%"
exit /b %update_exit_code%
