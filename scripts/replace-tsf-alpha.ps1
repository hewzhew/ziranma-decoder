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

    $output = @(& $script:devctl @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
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

    $output = @(& $script:candidatectl @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "candidatectl failed with exit code $exitCode"
    }
    if (-not $Quiet) {
        $output | ForEach-Object { Write-Host $_ }
    }
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

if (-not (Test-Path -LiteralPath $devctl -PathType Leaf) -or
    -not (Test-Path -LiteralPath $candidatectl -PathType Leaf) -or
    -not (Test-Path -LiteralPath $sourceDll -PathType Leaf)) {
    throw 'Release TSF artifacts are missing; run cargo build --release first.'
}

Set-Location -LiteralPath $repositoryRoot

if ($AdminPhase) {
    if (-not (Test-Administrator)) {
        throw 'The machine replacement phase requires elevation.'
    }
    $adminOutput = [Collections.Generic.List[string]]::new()
    try {
        if (Test-Path -LiteralPath $receipt -PathType Leaf) {
            $stepOutput = @(& $devctl `
                'unregister-machine' `
                '--confirm-machine-wide-development-alpha' 2>&1)
            $stepExitCode = $LASTEXITCODE
            $stepOutput | ForEach-Object { $adminOutput.Add([string]$_) }
            if ($stepExitCode -ne 0) {
                throw "unregister-machine stopped with exit code $stepExitCode"
            }
        }
        $stepOutput = @(& $devctl `
            'register-machine' `
            '--dll' `
            $sourceDll `
            '--confirm-machine-wide-development-alpha' 2>&1)
        $stepExitCode = $LASTEXITCODE
        $stepOutput | ForEach-Object { $adminOutput.Add([string]$_) }
        if ($stepExitCode -ne 0) {
            throw "register-machine stopped with exit code $stepExitCode"
        }
        [IO.File]::WriteAllLines(
            $adminReport,
            [string[]]$adminOutput,
            [Text.UTF8Encoding]::new($false)
        )
        $adminOutput | ForEach-Object { Write-Host $_ }
        exit 0
    } catch {
        $adminOutput.Add($_.Exception.Message)
        [IO.File]::WriteAllLines(
            $adminReport,
            [string[]]$adminOutput,
            [Text.UTF8Encoding]::new($false)
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
if (-not $ForceReregister -and
    $EnableCurrentUserAfterReplace -and
    $receiptDigest -eq $sourceDigest) {
    try {
        Invoke-DevCtl -Arguments @(
            'enable-current-user',
            '--confirm-enable-current-user-development-alpha'
        ) -Quiet
        Invoke-DevCtl -Arguments @('verify-current-user-enabled') -Quiet
        Invoke-DevCtl -Arguments @('verify-current-user-enabled') -Quiet
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
if (Test-Administrator) {
    & powershell.exe @adminArguments
    $adminExitCode = $LASTEXITCODE
} else {
    $adminProcess = Start-Process `
        -FilePath 'powershell.exe' `
        -Verb RunAs `
        -WindowStyle Normal `
        -ArgumentList $adminArguments `
        -Wait `
        -PassThru
    $adminExitCode = $adminProcess.ExitCode
}
if ($adminExitCode -ne 0) {
    if (Test-Path -LiteralPath $adminReport -PathType Leaf) {
        Get-Content -LiteralPath $adminReport | ForEach-Object { Write-Host $_ }
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
    Invoke-DevCtl -Arguments @('verify-current-user-enabled') -Quiet
    Invoke-DevCtl -Arguments @('verify-current-user-enabled') -Quiet
}
Invoke-DevCtl -Arguments @('inspect', '--dll', $sourceDll) -Quiet

Write-ReplacementSummary `
    -Digest $sourceDigest `
    -Result 'TSF Alpha replacement completed'
