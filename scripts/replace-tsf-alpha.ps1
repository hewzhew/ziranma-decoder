[CmdletBinding()]
param(
    [switch]$AdminPhase,
    [switch]$EnableCurrentUserAfterReplace,
    [switch]$ForceReregister
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$devctl = Join-Path $repositoryRoot 'target\release\tsf-devctl.exe'
$candidatectl = Join-Path $repositoryRoot 'target\release\candidatectl.exe'
$sourceDll = Join-Path $repositoryRoot 'target\release\ziranma_core.dll'
$sourceCandidateRoot = Join-Path $repositoryRoot 'target\release\candidate-data'
$receipt = Join-Path $repositoryRoot '.local\tsf-alpha\install-v1.txt'
$adminReport = Join-Path $repositoryRoot '.local\tsf-alpha\admin-phase-last.txt'
$windowsPowerShell = Join-Path `
    $env:SystemRoot `
    'System32\WindowsPowerShell\v1.0\powershell.exe'

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

function Invoke-DevCtl {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [switch]$Quiet
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $previousConsoleOutputEncoding = [Console]::OutputEncoding
    try {
        # Windows PowerShell 5.1 wraps native stderr as NativeCommandError.
        # Capture it with the process exit code before restoring strict script
        # error handling, so diagnostics do not terminate this function early.
        #
        # The Rust tools write UTF-8. Windows PowerShell otherwise decodes
        # redirected native output with the console code page (often CP936),
        # which turns Chinese diagnostics into mojibake before replaying them.
        $ErrorActionPreference = 'Continue'
        [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
        $output = @(& $script:devctl @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        [Console]::OutputEncoding = $previousConsoleOutputEncoding
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "tsf-devctl failed with exit code $exitCode"
    }
    if (-not $Quiet) {
        $output | ForEach-Object { Write-Host $_ }
    }
}

function Invoke-CandidateCtl {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [switch]$Quiet
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $previousConsoleOutputEncoding = [Console]::OutputEncoding
    try {
        $ErrorActionPreference = 'Continue'
        [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
        $output = @(& $script:candidatectl @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        [Console]::OutputEncoding = $previousConsoleOutputEncoding
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "candidatectl failed with exit code $exitCode"
    }
    if (-not $Quiet) {
        $output | ForEach-Object { Write-Host $_ }
    }
}

function Invoke-DevCtlCapture {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [Collections.Generic.List[string]]$Lines
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $previousConsoleOutputEncoding = [Console]::OutputEncoding
    $captured = @()
    $exitCode = 1
    try {
        $ErrorActionPreference = 'Continue'
        [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
        $captured = @(& $script:devctl @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        [Console]::OutputEncoding = $previousConsoleOutputEncoding
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $captured | ForEach-Object { [void]$Lines.Add([string]$_) }
    return $exitCode
}

function Get-Sha256Hex {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $stream = [IO.File]::OpenRead($Path)
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha256.ComputeHash($stream)
        return ([BitConverter]::ToString($digest)).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

function Get-InstalledReceiptDigest {
    if (-not (Test-Path -LiteralPath $script:receipt -PathType Leaf)) {
        return $null
    }
    $digestLines = @(
        Get-Content -LiteralPath $script:receipt |
            Where-Object { $_ -like 'dll_sha256=*' }
    )
    if ($digestLines.Count -ne 1) {
        return $null
    }
    $digest = $digestLines[0] -replace '^dll_sha256=', ''
    if ($digest -notmatch '^[0-9a-f]{64}$') {
        return $null
    }
    return $digest
}

function Test-CandidateSlotStateMatch {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceRoot,
        [Parameter(Mandatory = $true)]
        [string]$InstalledRoot
    )

    $sourceState = Join-Path $SourceRoot 'slots.zcs'
    $installedState = Join-Path $InstalledRoot 'slots.zcs'
    if (-not (Test-Path -LiteralPath $sourceState -PathType Leaf) -or
        -not (Test-Path -LiteralPath $installedState -PathType Leaf)) {
        return $false
    }
    $sourceStateDigest = Get-Sha256Hex -Path $sourceState
    $installedStateDigest = Get-Sha256Hex -Path $installedState
    return $sourceStateDigest -eq $installedStateDigest
}

function Write-ReplacementSummary {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Digest,
        [Parameter(Mandatory = $true)]
        [string]$Result
    )

    Write-Host ''
    Write-Host $Result
    Write-Host "DLL SHA-256: $Digest"
    Write-Host "Current user enable requested: $([bool]$script:EnableCurrentUserAfterReplace)"
    Write-Host 'Microsoft Pinyin and the default input method were unchanged'
}

function Restore-RequestedCurrentUserEnablement {
    if (-not $script:EnableCurrentUserAfterReplace -or
        -not (Test-Path -LiteralPath $script:receipt -PathType Leaf)) {
        return
    }

    try {
        Invoke-DevCtl -Arguments @(
            'enable-current-user',
            '--confirm-enable-current-user-development-alpha'
        ) -Quiet
        $verificationArguments = @('verify-current-user-enabled')
        if ($script:wasCurrentUserActive) {
            $verificationArguments += '--allow-active'
        }
        Invoke-DevCtl -Arguments $verificationArguments -Quiet
        Write-Host 'The previously installed TSF Alpha was restored for the current user.'
    } catch {
        Write-Warning 'The replacement failed and the available TSF Alpha could not be re-enabled automatically.'
    }
}

if (-not (Test-Path -LiteralPath $devctl -PathType Leaf) -or
    -not (Test-Path -LiteralPath $candidatectl -PathType Leaf) -or
    -not (Test-Path -LiteralPath $sourceDll -PathType Leaf)) {
    throw 'Release TSF artifacts are missing; run cargo build --release first.'
}
if (-not (Test-Path -LiteralPath $windowsPowerShell -PathType Leaf)) {
    throw 'Windows PowerShell 5.1 is unavailable.'
}

Set-Location -LiteralPath $repositoryRoot

if ($AdminPhase) {
    if (-not (Test-Administrator)) {
        throw 'The machine replacement phase requires elevation.'
    }
    $adminOutput = [Collections.Generic.List[string]]::new()
    $previousDigest = Get-InstalledReceiptDigest
    $previousDll = if ($null -ne $previousDigest) {
        Join-Path $repositoryRoot ".local\tsf-alpha\builds\$previousDigest\ziranma_core.dll"
    } else {
        $null
    }
    $oldUnregistered = $false
    $newRegistered = $false
    try {
        if (Test-Path -LiteralPath $receipt -PathType Leaf) {
            if ($null -eq $previousDll -or
                -not (Test-Path -LiteralPath $previousDll -PathType Leaf)) {
                throw 'the previous TSF Alpha receipt does not identify a recoverable DLL'
            }
            $stepExitCode = Invoke-DevCtlCapture -Arguments @(
                'unregister-machine',
                '--confirm-machine-wide-development-alpha'
            ) -Lines $adminOutput
            if ($stepExitCode -ne 0) {
                throw "unregister-machine stopped with exit code $stepExitCode"
            }
            $oldUnregistered = $true
        }
        $stepExitCode = Invoke-DevCtlCapture -Arguments @(
            'register-machine',
            '--dll',
            $sourceDll,
            '--confirm-machine-wide-development-alpha'
        ) -Lines $adminOutput
        if ($stepExitCode -ne 0) {
            throw "register-machine stopped with exit code $stepExitCode"
        }
        $newRegistered = $true
        [IO.File]::WriteAllLines(
            $adminReport,
            [string[]]$adminOutput,
            [Text.UTF8Encoding]::new($true)
        )
        $adminOutput | ForEach-Object { Write-Host $_ }
        exit 0
    } catch {
        [void]$adminOutput.Add($_.Exception.Message)
        if ($oldUnregistered -and -not $newRegistered -and $null -ne $previousDll) {
            [void]$adminOutput.Add('The new registration failed; restoring the previous TSF Alpha.')
            $restoreExitCode = Invoke-DevCtlCapture -Arguments @(
                'register-machine',
                '--dll',
                $previousDll,
                '--confirm-machine-wide-development-alpha'
            ) -Lines $adminOutput
            if ($restoreExitCode -eq 0) {
                [void]$adminOutput.Add('The previous machine registration was restored.')
            } else {
                [void]$adminOutput.Add(
                    "The previous machine registration could not be restored; exit code $restoreExitCode"
                )
            }
        }
        [IO.File]::WriteAllLines(
            $adminReport,
            [string[]]$adminOutput,
            [Text.UTF8Encoding]::new($true)
        )
        $adminOutput | ForEach-Object { Write-Host $_ }
        exit 1
    }
}

$sourceDigest = Get-Sha256Hex -Path $sourceDll
$installedBuildRoot = Join-Path $repositoryRoot ".local\tsf-alpha\builds\$sourceDigest"
$installedCandidateRoot = Join-Path $installedBuildRoot 'candidate-data'

if (-not (Test-Path -LiteralPath $sourceCandidateRoot -PathType Container)) {
    throw 'Release candidate data is missing; refusing to install the small development lexicon.'
}
Invoke-CandidateCtl -Arguments @('status', '--root', $sourceCandidateRoot) -Quiet

if (Test-Path -LiteralPath $installedCandidateRoot) {
    Invoke-CandidateCtl -Arguments @('status', '--root', $installedCandidateRoot) -Quiet
} else {
    New-Item -ItemType Directory -Path $installedBuildRoot -Force | Out-Null
    $temporaryCandidateRoot = Join-Path `
        $installedBuildRoot `
        ("candidate-data.tmp-" + [Guid]::NewGuid().ToString('N'))
    try {
        Copy-Item `
            -LiteralPath $sourceCandidateRoot `
            -Destination $temporaryCandidateRoot `
            -Recurse
        Invoke-CandidateCtl -Arguments @('status', '--root', $temporaryCandidateRoot) -Quiet
        Move-Item `
            -LiteralPath $temporaryCandidateRoot `
            -Destination $installedCandidateRoot
    } finally {
        if (Test-Path -LiteralPath $temporaryCandidateRoot) {
            Remove-Item -LiteralPath $temporaryCandidateRoot -Recurse -Force
        }
    }
}

$candidateSlotStateMatches = Test-CandidateSlotStateMatch `
    -SourceRoot $sourceCandidateRoot `
    -InstalledRoot $installedCandidateRoot
if (-not $candidateSlotStateMatches) {
    throw 'Release candidate slots differ from the immutable installed build; candidate-only replacement is not supported by this alpha.'
}

$receiptDigest = Get-InstalledReceiptDigest
$wasCurrentUserActive = $false
if ($null -ne $receiptDigest) {
    $currentUserStateOutput = [Collections.Generic.List[string]]::new()
    $currentUserStateExitCode = Invoke-DevCtlCapture `
        -Arguments @('current-user-state') `
        -Lines $currentUserStateOutput
    if ($currentUserStateExitCode -ne 0) {
        $currentUserStateOutput | ForEach-Object { Write-Host $_ }
        throw "current-user-state stopped with exit code $currentUserStateExitCode"
    }
    $currentUserStateLines = @(
        $currentUserStateOutput |
            Where-Object { $_ -like 'TSF_CURRENT_USER_STATE *' }
    )
    if ($currentUserStateLines.Count -ne 1) {
        throw 'current-user-state returned an unexpected report shape'
    }
    $currentUserStateMatch = [regex]::Match(
        $currentUserStateLines[0],
        '^TSF_CURRENT_USER_STATE schema=ziranma-tsf-current-user-state-v1 enabled=(true|false) active=(true|false) writes=false$'
    )
    if (-not $currentUserStateMatch.Success) {
        throw 'current-user-state returned an invalid report'
    }
    $wasCurrentUserActive = $currentUserStateMatch.Groups[2].Value -eq 'true'
}
$currentUserVerificationArguments = @('verify-current-user-enabled')
if ($wasCurrentUserActive) {
    $currentUserVerificationArguments += '--allow-active'
}
if (-not $ForceReregister -and
    $EnableCurrentUserAfterReplace -and
    $receiptDigest -eq $sourceDigest) {
    try {
        if (-not $wasCurrentUserActive) {
            Invoke-DevCtl -Arguments @(
                'enable-current-user',
                '--confirm-enable-current-user-development-alpha'
            ) -Quiet
        }
        Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet
        Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet
        Invoke-DevCtl -Arguments @('inspect', '--dll', $sourceDll) -Quiet
        Write-ReplacementSummary `
            -Digest $sourceDigest `
            -Result 'TSF Alpha is already current'
        exit 0
    } catch {
        Write-Host 'The existing TSF Alpha registration needs one compatibility refresh.'
    }
}

if (Test-Path -LiteralPath $receipt -PathType Leaf) {
    Invoke-DevCtl -Arguments @(
        'disable-current-user',
        '--confirm-disable-current-user-development-alpha'
    )
}

$adminArguments = @(
    '-NoProfile',
    '-ExecutionPolicy',
    'Bypass',
    '-File',
    "`"$PSCommandPath`"",
    '-AdminPhase'
)
$adminLaunchError = $null
if (Test-Administrator) {
    try {
        & $windowsPowerShell @adminArguments
        $adminExitCode = $LASTEXITCODE
    } catch {
        $adminExitCode = 1
        $adminLaunchError = $_.Exception.Message
    }
} else {
    try {
        $adminProcess = Start-Process `
            -FilePath $windowsPowerShell `
            -Verb RunAs `
            -WindowStyle Normal `
            -ArgumentList $adminArguments `
            -Wait `
            -PassThru
        $adminExitCode = $adminProcess.ExitCode
    } catch {
        $adminExitCode = 1
        $adminLaunchError = $_.Exception.Message
    }
}
if ($adminExitCode -ne 0) {
    if (Test-Path -LiteralPath $adminReport -PathType Leaf) {
        Get-Content -LiteralPath $adminReport -Encoding UTF8 |
            ForEach-Object { Write-Host $_ }
    }
    Restore-RequestedCurrentUserEnablement
    if ($null -ne $adminLaunchError) {
        Write-Warning "The administrator replacement process could not be started: $adminLaunchError"
    }
    throw "TSF Alpha replacement stopped with exit code $adminExitCode."
}
if (Test-Path -LiteralPath $adminReport -PathType Leaf) {
    Remove-Item -LiteralPath $adminReport -Force
}

Invoke-CandidateCtl -Arguments @('status', '--root', $installedCandidateRoot) -Quiet

$receiptDigest = Get-InstalledReceiptDigest
if ($receiptDigest -ne $sourceDigest) {
    throw 'The installed TSF receipt does not match the release DLL.'
}

if ($EnableCurrentUserAfterReplace) {
    Invoke-DevCtl -Arguments @(
        'enable-current-user',
        '--confirm-enable-current-user-development-alpha'
    ) -Quiet
    Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet
    Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet
}
Invoke-DevCtl -Arguments @('inspect', '--dll', $sourceDll) -Quiet

Write-ReplacementSummary `
    -Digest $sourceDigest `
    -Result 'TSF Alpha replacement completed'
