@echo off
setlocal

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
set "wish_root=%~dp0.local\tsf-alpha\user-data\wishes"

if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)

if "%~1"=="" (
    set "wishpad="
    for /f "usebackq delims=" %%P in (`call "%resolver%" wishpad`) do set "wishpad=%%P"
    if not defined wishpad (
        echo Wish manager is unavailable. Run refresh-ime.cmd first.
        exit /b 1
    )
    if not exist "%wishpad%" (
        echo Wish manager is unavailable. Run refresh-ime.cmd status.
        exit /b 1
    )
    start "" "%wishpad%"
    exit /b 0
)

set "wishctl="
for /f "usebackq delims=" %%P in (`call "%resolver%" wishctl`) do set "wishctl=%%P"
if not defined wishctl (
    echo Wish manager is unavailable. Run refresh-ime.cmd first.
    exit /b 1
)
if not exist "%wishctl%" (
    echo Wish manager is unavailable. Run refresh-ime.cmd status.
    exit /b 1
)

"%wishctl%" %* --root "%wish_root%"
exit /b %ERRORLEVEL%
