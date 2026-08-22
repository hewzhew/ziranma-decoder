@echo off
setlocal

if not "%~1"=="" goto usage

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)

set "typing_practice="
for /f "usebackq delims=" %%P in (`call "%resolver%" typing-practice`) do set "typing_practice=%%P"
if not defined typing_practice (
    echo Typing practice lab is unavailable. Run refresh-ime.cmd refresh first.
    exit /b 1
)
if not exist "%typing_practice%" (
    echo Typing practice lab is unavailable. Run refresh-ime.cmd status.
    exit /b 1
)

start "" "%typing_practice%"
exit /b 0

:usage
echo Usage: typing-practice.cmd 1>&2
exit /b 2
