@echo off
setlocal

if not "%~2"=="" goto usage

set "action=%~1"
if "%action%"=="" set "action=refresh"
if /i not "%action%"=="refresh" if /i not "%action%"=="status" if /i not "%action%"=="space" if /i not "%action%"=="rollback" goto usage

set "refresh_script=%~dp0scripts\refresh-user-tools.ps1"
if not exist "%refresh_script%" (
    echo User tool refresh script is missing.
    exit /b 1
)

set "windows_powershell=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%windows_powershell%" (
    echo Windows PowerShell is missing.
    exit /b 1
)

if /i "%action%"=="status" goto status
if /i "%action%"=="space" goto space
if /i "%action%"=="rollback" goto rollback

"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%refresh_script%"
exit /b %ERRORLEVEL%

:status
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%refresh_script%" -StatusOnly
exit /b %ERRORLEVEL%

:space
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%refresh_script%" -SpaceOnly
exit /b %ERRORLEVEL%

:rollback
"%windows_powershell%" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%refresh_script%" -Rollback
exit /b %ERRORLEVEL%

:usage
echo Usage: refresh-ime.cmd [refresh^|status^|space^|rollback]
exit /b 2
