@echo off
setlocal

if not "%~2"=="" (
    echo Usage: update-ime.cmd [status]
    exit /b 2
)

set "update_mode=%~1"
if not "%update_mode%"=="" if /i not "%update_mode%"=="status" (
    echo Usage: update-ime.cmd [status]
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

if /i "%update_mode%"=="status" goto status

"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%update_script%" -EnableCurrentUserAfterReplace
set "update_exit_code=%ERRORLEVEL%"
if "%update_exit_code%"=="0" (
    if exist "%~dp0wish-ime.cmd" (
        call "%~dp0wish-ime.cmd"
    )
)
exit /b %update_exit_code%

:status
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%update_script%" -StatusOnly
exit /b %ERRORLEVEL%
