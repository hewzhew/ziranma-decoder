@echo off
setlocal

set "resolver=%~dp0scripts\resolve-user-tool.cmd"
if not exist "%resolver%" (
    echo User tool resolver is missing.
    exit /b 1
)

set "candidatectl="
for /f "usebackq delims=" %%P in (`call "%resolver%" candidatectl`) do set "candidatectl=%%P"
if not defined candidatectl (
    echo Candidate data manager is unavailable. Run refresh-ime.cmd refresh first.
    exit /b 1
)
if not exist "%candidatectl%" (
    echo Candidate data manager is unavailable. Run refresh-ime.cmd status.
    exit /b 1
)

"%candidatectl%" %*
exit /b %ERRORLEVEL%
