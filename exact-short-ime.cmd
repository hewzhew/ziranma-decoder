@echo off
setlocal

if not "%~2"=="" goto usage

set "action=%~1"
if "%action%"=="" set "action=status"
if /i "%action%"=="status" goto configure
if /i "%action%"=="enable" goto configure
if /i "%action%"=="disable" goto configure
goto usage

:configure
set "repository_root=%~dp0"
set "resolver=%repository_root%scripts\resolve-user-tool.cmd"
set "exact_root=%repository_root%.local\tsf-alpha\user-data\public-exact-short"
set "core_root=%repository_root%target\release\candidate-data"
set "supplemental_root=%repository_root%.local\tsf-alpha\user-data\public-supplement"
set "package=%repository_root%.local\public-audit\wanxiang-fdda7afb\package-exact-short-consensus-depth2-v1"
set "expected_sha256=2cd80edd03f2c420e8b54b37db32576dc73c7f63e787df1c82ba99980c0ddec3"

if not exist "%resolver%" (
    echo User tool resolver is missing. The exact-short state was not changed.
    exit /b 1
)

set "candidatectl="
for /f "usebackq delims=" %%P in (`call "%resolver%" candidatectl`) do set "candidatectl=%%P"
if not defined candidatectl goto tool_unavailable
if not exist "%candidatectl%" goto tool_unavailable

set "required_command=exact-short-readiness"
if /i "%action%"=="enable" set "required_command=exact-short-enable"
if /i "%action%"=="disable" set "required_command=exact-short-disable"
"%candidatectl%" 2>&1 | "%SystemRoot%\System32\findstr.exe" /l /c:"%required_command%" >nul
if errorlevel 1 goto tool_outdated

if /i "%action%"=="enable" goto enable
if /i "%action%"=="disable" goto disable

:status
call :readiness
exit /b %ERRORLEVEL%

:enable
"%candidatectl%" exact-short-enable ^
    --root "%exact_root%" ^
    --core-root "%core_root%" ^
    --supplemental-root "%supplemental_root%" ^
    --package "%package%" ^
    --expected-sha256 "%expected_sha256%" ^
    --exact-promotions 2
set "enable_exit_code=%ERRORLEVEL%"
if not "%enable_exit_code%"=="0" (
    echo Enable did not complete. Review the message above before retrying.
    exit /b %enable_exit_code%
)
echo.
call :readiness
set "status_exit_code=%ERRORLEVEL%"
if not "%status_exit_code%"=="0" (
    echo Enable completed, but the final read-only readiness check failed.
    echo Run exact-short-ime.cmd disable to return to the safe state.
    exit /b %status_exit_code%
)
exit /b 0

:disable
"%candidatectl%" exact-short-disable --root "%exact_root%"
set "disable_exit_code=%ERRORLEVEL%"
if not "%disable_exit_code%"=="0" exit /b %disable_exit_code%
echo.
call :readiness
exit /b %ERRORLEVEL%

:readiness
"%candidatectl%" exact-short-readiness ^
    --root "%exact_root%" ^
    --core-root "%core_root%" ^
    --supplemental-root "%supplemental_root%" ^
    --package "%package%" ^
    --expected-sha256 "%expected_sha256%" ^
    --exact-promotions 2
exit /b %ERRORLEVEL%

:tool_unavailable
echo Candidate data manager is unavailable. Run refresh-ime.cmd status first.
echo The exact-short state was not changed.
exit /b 1

:tool_outdated
echo The current user tool bundle does not support %required_command%.
echo Run refresh-ime.cmd refresh explicitly, then try again.
echo The exact-short state was not changed.
exit /b 1

:usage
echo Usage: exact-short-ime.cmd [status^|enable^|disable]
echo Default: status ^(read only^). Enable and disable always require explicit actions.
exit /b 2
