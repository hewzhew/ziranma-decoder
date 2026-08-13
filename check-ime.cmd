@echo off
setlocal

if not "%~1"=="" (
    echo Usage: check-ime.cmd
    echo This command is always read only and accepts no actions.
    exit /b 2
)

set "repository_root=%~dp0"
set "tsf_status_script=%repository_root%scripts\replace-tsf-alpha.ps1"
set "tool_status_script=%repository_root%scripts\refresh-user-tools.ps1"
set "windows_powershell=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"

if not exist "%tsf_status_script%" (
    echo TSF status script is missing.
    exit /b 1
)
if not exist "%tool_status_script%" (
    echo User tool status script is missing.
    exit /b 1
)
if not exist "%windows_powershell%" (
    echo Windows PowerShell is missing.
    exit /b 1
)

echo === TSF Alpha status ===
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%tsf_status_script%" -StatusOnly
set "tsf_exit_code=%ERRORLEVEL%"

echo.
echo === IME user tool status ===
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%tool_status_script%" -StatusOnly
set "tool_exit_code=%ERRORLEVEL%"

echo.
echo This combined check was read only. No tool was published and no TSF build was installed.
if not "%tsf_exit_code%"=="0" exit /b %tsf_exit_code%
exit /b %tool_exit_code%
