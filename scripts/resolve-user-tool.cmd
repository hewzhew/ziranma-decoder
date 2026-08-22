@echo off
setlocal

if "%~1"=="" goto usage
if not "%~2"=="" goto usage

set "tool=%~1"
if /i "%tool%"=="aliasctl" goto tool_ok
if /i "%tool%"=="aliaspad" goto tool_ok
if /i "%tool%"=="candidatectl" goto tool_ok
if /i "%tool%"=="personalctl" goto tool_ok
if /i "%tool%"=="researchctl" goto tool_ok
if /i "%tool%"=="typing-practice" goto tool_ok
if /i "%tool%"=="wishctl" goto tool_ok
if /i "%tool%"=="wishpad" goto tool_ok
goto usage

:tool_ok
set "repository_root=%~dp0..\"
set "slots=%repository_root%.local\tsf-alpha\user-tools\slots.zut"
set "fallback=%repository_root%target\release\%tool%.exe"

if not exist "%slots%" goto fallback

set "line_count="
for /f %%C in ('find /v /c "" ^< "%slots%"') do set "line_count=%%C"
if not "%line_count%"=="3" goto invalid_slots
findstr /l /x /c:"schema=ziranma-user-tools-slots-v1" "%slots%" >nul
if errorlevel 1 goto invalid_slots
findstr /r /x /c:"current=[0-9a-f][0-9a-f]*" "%slots%" >nul
if errorlevel 1 goto invalid_slots
findstr /r /x /c:"previous=-" /c:"previous=[0-9a-f][0-9a-f]*" "%slots%" >nul
if errorlevel 1 goto invalid_slots

set "slot_schema="
set "bundle_id="
for /f "usebackq tokens=1,* delims==" %%A in ("%slots%") do (
    if "%%A"=="schema" set "slot_schema=%%B"
    if "%%A"=="current" set "bundle_id=%%B"
)
if not "%slot_schema%"=="ziranma-user-tools-slots-v1" goto invalid_slots
if "%bundle_id:~63,1%"=="" goto invalid_slots
if not "%bundle_id:~64,1%"=="" goto invalid_slots
for /f "delims=0123456789abcdef" %%H in ("%bundle_id%") do goto invalid_slots

set "resolved=%repository_root%.local\tsf-alpha\user-tools\builds\%bundle_id%\%tool%.exe"
if not exist "%resolved%" goto invalid_slots
echo %resolved%
exit /b 0

:fallback
if not exist "%fallback%" exit /b 1
echo %fallback%
exit /b 0

:invalid_slots
echo User tool slots are invalid. Run refresh-ime.cmd status. 1>&2
exit /b 1

:usage
echo Usage: resolve-user-tool.cmd TOOL 1>&2
exit /b 2
