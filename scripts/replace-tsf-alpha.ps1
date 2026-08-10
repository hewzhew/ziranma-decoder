[CmdletBinding()]
# Keep this file ASCII unless it gains a UTF-8 BOM. update-ime.cmd invokes
# Windows PowerShell 5.1, which otherwise decodes BOM-less scripts by code page.
param(
    [switch]$AdminPhase,
    [switch]$EnableCurrentUserAfterReplace,
    [switch]$ForceReregister,
    [switch]$StatusOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$devctl = Join-Path $repositoryRoot 'target\release\tsf-devctl.exe'
$candidatectl = Join-Path $repositoryRoot 'target\release\candidatectl.exe'
$sourceDll = Join-Path $repositoryRoot 'target\release\ziranma_core.dll'
$sourceCandidateRoot = Join-Path $repositoryRoot 'target\release\candidate-data'
$stateRoot = Join-Path $repositoryRoot '.local\tsf-alpha'
$receipt = Join-Path $stateRoot 'install-v1.txt'
$adminReport = Join-Path $stateRoot 'admin-phase-last.txt'
$replacementLockPath = Join-Path $stateRoot 'replacement.lock'
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

function Open-ReplacementLock {
    if (Test-Path -LiteralPath $script:stateRoot) {
        $stateRootItem = Get-Item -LiteralPath $script:stateRoot -Force
        if (-not $stateRootItem.PSIsContainer -or
            ($stateRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'The TSF Alpha state root is invalid.'
        }
    } else {
        New-Item -ItemType Directory -Path $script:stateRoot | Out-Null
    }
    if (Test-Path -LiteralPath $script:replacementLockPath) {
        $lockItem = Get-Item -LiteralPath $script:replacementLockPath -Force
        if ($lockItem.PSIsContainer -or
            ($lockItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'The TSF Alpha replacement lock is invalid.'
        }
    }
    try {
        return [IO.File]::Open(
            $script:replacementLockPath,
            [IO.FileMode]::OpenOrCreate,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
    } catch {
        throw 'Another TSF Alpha replacement is running, or its lock is unavailable.'
    }
}

function Remove-StaleAdministratorReport {
    if (-not (Test-Path -LiteralPath $script:adminReport)) {
        return
    }
    $item = Get-Item -LiteralPath $script:adminReport -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'The administrator replacement report is invalid.'
    }
    Remove-Item -LiteralPath $script:adminReport -Force
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
        [string]$Result,
        [Parameter(Mandatory = $true)]
        [object]$HostCacheState,
        [Parameter(Mandatory = $true)]
        [Diagnostics.Stopwatch]$TotalStopwatch,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$TimingParts
    )

    Write-Host ''
    Write-Host $Result
    Write-Host "DLL SHA-256: $Digest"
    if ($script:EnableCurrentUserAfterReplace) {
        Write-Host 'Current user enable requested: True'
    } else {
        Write-Host 'Current user enable requested: False'
    }
    if (-not $HostCacheState.ScanAvailable) {
        Write-Host 'Host cache: inspection unavailable'
    } elseif ($HostCacheState.MatchingVersion -eq 0 -and
        $HostCacheState.OtherVersions -eq 0) {
        Write-Host 'Host cache: no visible process has loaded TSF Alpha'
    } elseif ($HostCacheState.OtherVersions -eq 0) {
        Write-Host (
            "Host cache: {0} visible process(es) loaded the current build; no old build found" -f
                $HostCacheState.MatchingVersion
        )
    } elseif ($HostCacheState.MatchingVersion -eq 0) {
        Write-Host (
            "Host cache: {0} visible process(es) still use an old build" -f
                $HostCacheState.OtherVersions
        )
    } else {
        Write-Host (
            "Host cache: {0} visible process(es) loaded the current build; {1} still use an old build" -f
                $HostCacheState.MatchingVersion,
                $HostCacheState.OtherVersions
        )
    }
    if ($HostCacheState.ScanAvailable -and
        $HostCacheState.OtherVersions -gt 0) {
        Write-Host 'No process must be closed now; old hosts will load the current build next time.'
    } else {
        Write-Host 'No further host action is needed.'
    }
    Write-Host (
        'Timing: {0} ms total ({1})' -f
            $TotalStopwatch.ElapsedMilliseconds,
            ($TimingParts -join '; ')
    )
    Write-Host 'Microsoft Pinyin and the default input method were unchanged'
}

function Get-HostCacheState {
    $stateOutput = [Collections.Generic.List[string]]::new()
    $stateExitCode = Invoke-DevCtlCapture `
        -Arguments @('host-cache-state', '--dll', $script:sourceDll) `
        -Lines $stateOutput
    if ($stateExitCode -ne 0) {
        return [PSCustomObject]@{
            ScanAvailable = $false
            MatchingVersion = [uint32]0
            OtherVersions = [uint32]0
        }
    }
    $stateLines = @(
        $stateOutput |
            Where-Object { $_ -like 'TSF_HOST_CACHE_STATE *' }
    )
    if ($stateLines.Count -ne 1) {
        return [PSCustomObject]@{
            ScanAvailable = $false
            MatchingVersion = [uint32]0
            OtherVersions = [uint32]0
        }
    }
    $stateMatch = [regex]::Match(
        $stateLines[0],
        '^TSF_HOST_CACHE_STATE schema=ziranma-tsf-host-cache-state-v1 scan_available=(true|false) matching_version=([0-9]+) other_versions=([0-9]+) writes=false$'
    )
    if (-not $stateMatch.Success) {
        return [PSCustomObject]@{
            ScanAvailable = $false
            MatchingVersion = [uint32]0
            OtherVersions = [uint32]0
        }
    }
    return [PSCustomObject]@{
        ScanAvailable = $stateMatch.Groups[1].Value -eq 'true'
        MatchingVersion = [uint32]::Parse($stateMatch.Groups[2].Value)
        OtherVersions = [uint32]::Parse($stateMatch.Groups[3].Value)
    }
}

function Get-CurrentUserState {
    $stateOutput = [Collections.Generic.List[string]]::new()
    $stateExitCode = Invoke-DevCtlCapture `
        -Arguments @('current-user-state') `
        -Lines $stateOutput
    if ($stateExitCode -ne 0) {
        $stateOutput | ForEach-Object { Write-Host $_ }
        throw "current-user-state stopped with exit code $stateExitCode"
    }
    $stateLines = @(
        $stateOutput |
            Where-Object { $_ -like 'TSF_CURRENT_USER_STATE *' }
    )
    if ($stateLines.Count -ne 1) {
        throw 'current-user-state returned an unexpected report shape'
    }
    $stateMatch = [regex]::Match(
        $stateLines[0],
        '^TSF_CURRENT_USER_STATE schema=ziranma-tsf-current-user-state-v1 enabled=(true|false) active=(true|false) writes=false$'
    )
    if (-not $stateMatch.Success) {
        throw 'current-user-state returned an invalid report'
    }
    return [PSCustomObject]@{
        Enabled = $stateMatch.Groups[1].Value -eq 'true'
        Active = $stateMatch.Groups[2].Value -eq 'true'
    }
}

function Write-UpdateStatus {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDigest,
        [AllowNull()]
        [string]$InstalledDigest,
        [AllowNull()]
        [object]$CurrentUserState,
        [Parameter(Mandatory = $true)]
        [object]$HostCacheState,
        [Parameter(Mandatory = $true)]
        [string]$UpdateState,
        [Parameter(Mandatory = $true)]
        [Diagnostics.Stopwatch]$TotalStopwatch,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$TimingParts
    )

    Write-Host 'TSF Alpha update status'
    Write-Host "Release DLL: $SourceDigest"
    if ($null -eq $InstalledDigest) {
        Write-Host 'Installed DLL: none'
    } else {
        Write-Host "Installed DLL: $InstalledDigest"
    }
    Write-Host "Update: $UpdateState"
    if ($null -eq $CurrentUserState) {
        Write-Host 'Current user: unavailable before first install'
    } else {
        Write-Host (
            'Current user: enabled={0} active={1}' -f
                $CurrentUserState.Enabled.ToString().ToLowerInvariant(),
                $CurrentUserState.Active.ToString().ToLowerInvariant()
        )
    }
    if (-not $HostCacheState.ScanAvailable) {
        Write-Host 'Host cache: inspection unavailable'
    } else {
        Write-Host (
            'Host cache: release build {0}; other builds {1}' -f
                $HostCacheState.MatchingVersion,
                $HostCacheState.OtherVersions
        )
    }
    if ($UpdateState -eq 'already current' -and $HostCacheState.OtherVersions -gt 0) {
        Write-Host 'Next: no installation needed; existing old hosts update when reopened.'
    } elseif ($UpdateState -eq 'already current') {
        Write-Host 'Next: no installation or host restart needed.'
    } elseif ($null -eq $InstalledDigest) {
        Write-Host 'Next: run update-ime.cmd when a machine-wide first install is wanted.'
    } else {
        Write-Host 'Next: run update-ime.cmd when convenient; existing hosts need not close first.'
    }
    Write-Host (
        'Timing: {0} ms total ({1})' -f
            $TotalStopwatch.ElapsedMilliseconds,
            ($TimingParts -join '; ')
    )
    Write-Host 'This action: read only'
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

if ($StatusOnly) {
    if ($AdminPhase -or $EnableCurrentUserAfterReplace -or $ForceReregister) {
        throw 'StatusOnly cannot be combined with replacement switches.'
    }
    $statusStopwatch = [Diagnostics.Stopwatch]::StartNew()
    $releaseValidationStopwatch = [Diagnostics.Stopwatch]::StartNew()
    $sourceDigest = Get-Sha256Hex -Path $sourceDll
    if (-not (Test-Path -LiteralPath $sourceCandidateRoot -PathType Container)) {
        throw 'Release candidate data is missing.'
    }
    Invoke-CandidateCtl -Arguments @('status', '--root', $sourceCandidateRoot) -Quiet
    $releaseValidationStopwatch.Stop()
    $installedStateStopwatch = [Diagnostics.Stopwatch]::StartNew()
    $receiptPresent = Test-Path -LiteralPath $receipt -PathType Leaf
    $receiptDigest = Get-InstalledReceiptDigest
    if ($receiptPresent -and $null -eq $receiptDigest) {
        throw 'The installed TSF receipt is invalid.'
    }
    $currentUserState = if ($null -ne $receiptDigest) {
        Get-CurrentUserState
    } else {
        $null
    }
    if ($receiptDigest -eq $sourceDigest) {
        $installedCandidateRoot = Join-Path `
            $repositoryRoot `
            ".local\tsf-alpha\builds\$sourceDigest\candidate-data"
        if (-not (Test-Path -LiteralPath $installedCandidateRoot -PathType Container)) {
            throw 'The current immutable build has no installed candidate data.'
        }
        Invoke-CandidateCtl -Arguments @('status', '--root', $installedCandidateRoot) -Quiet
        if (-not (Test-CandidateSlotStateMatch `
            -SourceRoot $sourceCandidateRoot `
            -InstalledRoot $installedCandidateRoot)) {
            throw 'Release candidate slots differ from the immutable installed build.'
        }
    }
    $installedStateStopwatch.Stop()
    $hostCacheStopwatch = [Diagnostics.Stopwatch]::StartNew()
    $hostCacheState = Get-HostCacheState
    $hostCacheStopwatch.Stop()
    $updateState = if ($null -eq $receiptDigest) {
        'not installed'
    } elseif ($receiptDigest -eq $sourceDigest) {
        'already current'
    } else {
        'ready to install'
    }
    Write-UpdateStatus `
        -SourceDigest $sourceDigest `
        -InstalledDigest $receiptDigest `
        -CurrentUserState $currentUserState `
        -HostCacheState $hostCacheState `
        -UpdateState $updateState `
        -TotalStopwatch $statusStopwatch `
        -TimingParts @(
            "release validation $($releaseValidationStopwatch.ElapsedMilliseconds) ms",
            "installed state $($installedStateStopwatch.ElapsedMilliseconds) ms",
            "host scan $($hostCacheStopwatch.ElapsedMilliseconds) ms"
        )
    exit 0
}

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

$replacementLock = Open-ReplacementLock
try {
$totalStopwatch = [Diagnostics.Stopwatch]::StartNew()
$preflightStopwatch = [Diagnostics.Stopwatch]::StartNew()
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
    $wasCurrentUserActive = (Get-CurrentUserState).Active
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
        $preflightStopwatch.Stop()
        $verificationStopwatch = [Diagnostics.Stopwatch]::StartNew()
        Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet
        $verificationStopwatch.Stop()
        $hostCacheStopwatch = [Diagnostics.Stopwatch]::StartNew()
        $hostCacheState = Get-HostCacheState
        $hostCacheStopwatch.Stop()
        Write-ReplacementSummary `
            -Digest $sourceDigest `
            -Result 'TSF Alpha is already current' `
            -HostCacheState $hostCacheState `
            -TotalStopwatch $totalStopwatch `
            -TimingParts @(
                "preflight $($preflightStopwatch.ElapsedMilliseconds) ms",
                "enable verification $($verificationStopwatch.ElapsedMilliseconds) ms",
                "host scan $($hostCacheStopwatch.ElapsedMilliseconds) ms"
            )
        exit 0
    } catch {
        Write-Host 'The existing TSF Alpha registration needs one compatibility refresh.'
    }
}

$preflightStopwatch.Stop()
Remove-StaleAdministratorReport
$disableStopwatch = [Diagnostics.Stopwatch]::StartNew()
if (Test-Path -LiteralPath $receipt -PathType Leaf) {
    Invoke-DevCtl -Arguments @(
        'disable-current-user',
        '--confirm-disable-current-user-development-alpha'
    )
}
$disableStopwatch.Stop()

$adminArguments = @(
    '-NoProfile',
    '-NonInteractive',
    '-ExecutionPolicy',
    'Bypass',
    '-File',
    "`"$PSCommandPath`"",
    '-AdminPhase'
)
$adminLaunchError = $null
$administratorStopwatch = [Diagnostics.Stopwatch]::StartNew()
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
$administratorStopwatch.Stop()
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

$postflightStopwatch = [Diagnostics.Stopwatch]::StartNew()
try {
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
    }
} catch {
    Write-Warning 'Machine replacement completed, but postflight verification failed; restoring the requested current-user enablement.'
    Restore-RequestedCurrentUserEnablement
    throw
}
$postflightStopwatch.Stop()

$hostCacheStopwatch = [Diagnostics.Stopwatch]::StartNew()
$hostCacheState = Get-HostCacheState
$hostCacheStopwatch.Stop()
Write-ReplacementSummary `
    -Digest $sourceDigest `
    -Result 'TSF Alpha replacement completed' `
    -HostCacheState $hostCacheState `
    -TotalStopwatch $totalStopwatch `
    -TimingParts @(
        "preflight $($preflightStopwatch.ElapsedMilliseconds) ms",
        "disable $($disableStopwatch.ElapsedMilliseconds) ms",
        "administrator/UAC $($administratorStopwatch.ElapsedMilliseconds) ms",
        "postflight $($postflightStopwatch.ElapsedMilliseconds) ms",
        "host scan $($hostCacheStopwatch.ElapsedMilliseconds) ms"
    )
} finally {
    if ($null -ne $replacementLock) {
        $replacementLock.Dispose()
    }
}
