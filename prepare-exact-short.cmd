@echo off
setlocal

if not "%~2"=="" goto usage

set "action=%~1"
if "%action%"=="" set "action=status"
if /i "%action%"=="status" goto configure
if /i "%action%"=="prepare" goto configure
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
    echo User tool resolver is missing. Nothing was prepared or enabled.
    exit /b 1
)

set "candidatectl="
for /f "usebackq delims=" %%P in (`call "%resolver%" candidatectl`) do set "candidatectl=%%P"
if not defined candidatectl goto tool_unavailable
if not exist "%candidatectl%" goto tool_unavailable

set "required_command=exact-short-status"
if /i "%action%"=="prepare" set "required_command=exact-short-prepare"
"%candidatectl%" 2>&1 | "%SystemRoot%\System32\findstr.exe" /l /c:"%required_command%" >nul
if errorlevel 1 goto tool_outdated

if /i "%action%"=="prepare" goto prepare

:status
"%candidatectl%" exact-short-status --root "%exact_root%"
exit /b %ERRORLEVEL%

:prepare
if not exist "%core_root%\slots.zcs" goto core_unavailable
if not exist "%supplemental_root%\slots.zcs" goto supplement_unavailable
if not exist "%supplemental_root%\supplemental.zcl" goto supplement_unavailable
if not exist "%package%\manifest.zcm" goto package_unavailable
if not exist "%package%\provenance.zcp" goto package_unavailable
if not exist "%package%\lexicon.tsv" goto package_unavailable

"%candidatectl%" exact-short-prepare ^
    --root "%exact_root%" ^
    --core-root "%core_root%" ^
    --supplemental-root "%supplemental_root%" ^
    --package "%package%" ^
    --expected-sha256 "%expected_sha256%" ^
    --exact-promotions 2 ^
    --sample-limit 16 ^
    --repetitions 5
set "prepare_exit_code=%ERRORLEVEL%"
if not "%prepare_exit_code%"=="0" (
    echo Preparation did not complete. This entry did not enable the exact-short layer.
    exit /b %prepare_exit_code%
)

echo.
"%candidatectl%" exact-short-status --root "%exact_root%"
set "status_exit_code=%ERRORLEVEL%"
if not "%status_exit_code%"=="0" (
    echo Preparation completed, but the final read-only status check failed.
    exit /b %status_exit_code%
)
echo Preparation completed. The exact-short layer remains disabled.
exit /b 0

:tool_unavailable
echo Candidate data manager is unavailable. Run refresh-ime.cmd status first.
echo Nothing was prepared or enabled.
exit /b 1

:tool_outdated
echo The current user tool bundle does not support %required_command%.
echo Run refresh-ime.cmd refresh explicitly, then try again.
echo Nothing was prepared or enabled.
exit /b 1

:core_unavailable
echo The authenticated core candidate slot is unavailable.
echo Nothing was prepared or enabled.
exit /b 1

:supplement_unavailable
echo The authenticated supplemental candidate slot is unavailable.
echo Nothing was prepared or enabled.
exit /b 1

:package_unavailable
echo The fixed public exact-short package is unavailable.
echo Nothing was prepared or enabled.
exit /b 1

:usage
echo Usage: prepare-exact-short.cmd [status^|prepare]
echo Default: status ^(read only^). The explicit prepare action prepares but never enables the layer.
exit /b 2
