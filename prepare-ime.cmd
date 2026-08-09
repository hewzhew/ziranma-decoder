@echo off
setlocal

if not "%~1"=="" (
    echo Usage: prepare-ime.cmd
    exit /b 2
)

where cargo.exe >nul 2>&1
if not "%ERRORLEVEL%"=="0" (
    echo Cargo is missing from PATH. Nothing was built or installed.
    exit /b 1
)

set "repository_root=%~dp0"
pushd "%repository_root%"
if not "%ERRORLEVEL%"=="0" (
    echo The repository root is unavailable. Nothing was built or installed.
    exit /b 1
)

cargo.exe build --release --locked --offline ^
    --lib ^
    --bin tsf-devctl ^
    --bin candidatectl ^
    --bin aliasctl ^
    --bin aliaspad ^
    --bin personalctl ^
    --bin researchctl ^
    --bin wishctl ^
    --bin wishpad
set "build_exit_code=%ERRORLEVEL%"
if not "%build_exit_code%"=="0" (
    popd
    echo Release preparation failed. Nothing was installed.
    exit /b %build_exit_code%
)

call "%repository_root%update-ime.cmd" status
set "status_exit_code=%ERRORLEVEL%"
popd
if not "%status_exit_code%"=="0" (
    echo Release validation failed. Nothing was installed.
    exit /b %status_exit_code%
)

echo.
echo IME release preparation completed. Nothing was installed.
echo Run update-ime.cmd later only when a machine replacement is convenient.
exit /b 0
