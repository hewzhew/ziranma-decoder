[CmdletBinding()]
param(
    [switch]$StatusOnly,
    [switch]$Rollback,
    [string]$UserToolsRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($StatusOnly -and $Rollback) {
    throw 'StatusOnly cannot be combined with Rollback.'
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($UserToolsRoot)) {
    $UserToolsRoot = Join-Path $repositoryRoot '.local\tsf-alpha\user-tools'
}
$UserToolsRoot = [IO.Path]::GetFullPath($UserToolsRoot)
$buildsRoot = Join-Path $UserToolsRoot 'builds'
$cargoTarget = Join-Path $UserToolsRoot 'cargo-target'
$statePath = Join-Path $UserToolsRoot 'slots.zut'
$lockPath = Join-Path $UserToolsRoot 'refresh.lock'
$schema = 'ziranma-user-tools-slots-v1'
$bundleSchema = 'ziranma-user-tools-bundle-v1'
$toolNames = @(
    'aliasctl',
    'aliaspad',
    'candidatectl',
    'personalctl',
    'researchctl',
    'wishctl',
    'wishpad'
)
$utf8 = New-Object Text.UTF8Encoding($false, $true)

function Test-BundleId {
    param([AllowNull()][string]$Value)

    return $null -ne $Value -and $Value -match '^[0-9a-f]{64}$'
}

function Assert-NormalFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing."
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label must not be a reparse point."
    }
    if ($item.Length -le 0) {
        throw "$Label is empty."
    }
}

function Assert-NormalDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "$Label is missing."
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label must not be a reparse point."
    }
}

function Open-RefreshLock {
    if (Test-Path -LiteralPath $lockPath) {
        $item = Get-Item -LiteralPath $lockPath -Force
        if ($item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'The user tool refresh lock is invalid.'
        }
    }
    return [IO.File]::Open(
        $lockPath,
        [IO.FileMode]::OpenOrCreate,
        [IO.FileAccess]::ReadWrite,
        [IO.FileShare]::None
    )
}

function Get-Sha256Hex {
    param([Parameter(Mandatory = $true)][string]$Path)

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

function Get-BytesSha256Hex {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)

    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha256.ComputeHash($Bytes)
        return ([BitConverter]::ToString($digest)).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function Get-ManifestBody {
    param([Parameter(Mandatory = $true)][string]$ReleaseRoot)

    $lines = @("schema=$bundleSchema")
    foreach ($tool in $toolNames) {
        $name = "$tool.exe"
        $path = Join-Path $ReleaseRoot $name
        Assert-NormalFile -Path $path -Label $name
        $lines += "tool.$name=$(Get-Sha256Hex -Path $path)"
    }
    return ($lines -join "`n") + "`n"
}

function Assert-Bundle {
    param([Parameter(Mandatory = $true)][string]$BundleId)

    if (-not (Test-BundleId -Value $BundleId)) {
        throw 'The user tool bundle id is invalid.'
    }
    $bundleRoot = Join-Path $buildsRoot $BundleId
    Assert-NormalDirectory -Path $bundleRoot -Label 'User tool bundle'
    $manifestPath = Join-Path $bundleRoot 'manifest.zut'
    Assert-NormalFile -Path $manifestPath -Label 'User tool manifest'
    if ((Get-Item -LiteralPath $manifestPath).Length -gt 4096) {
        throw 'The user tool manifest is too large.'
    }
    $manifestBytes = [IO.File]::ReadAllBytes($manifestPath)
    if ((Get-BytesSha256Hex -Bytes $manifestBytes) -ne $BundleId) {
        throw 'The user tool manifest does not match its bundle id.'
    }
    $manifest = $utf8.GetString($manifestBytes)
    $lines = $manifest.TrimEnd("`r", "`n") -split "`n"
    if ($lines.Count -ne $toolNames.Count + 1 -or $lines[0].TrimEnd("`r") -ne "schema=$bundleSchema") {
        throw 'The user tool manifest shape is invalid.'
    }
    for ($index = 0; $index -lt $toolNames.Count; $index++) {
        $name = "$($toolNames[$index]).exe"
        $line = $lines[$index + 1].TrimEnd("`r")
        $prefix = "tool.$name="
        if (-not $line.StartsWith($prefix, [StringComparison]::Ordinal)) {
            throw "The user tool manifest entry for $name is invalid."
        }
        $expected = $line.Substring($prefix.Length)
        if (-not (Test-BundleId -Value $expected)) {
            throw "The user tool digest for $name is invalid."
        }
        $path = Join-Path $bundleRoot $name
        Assert-NormalFile -Path $path -Label $name
        if ((Get-Sha256Hex -Path $path) -ne $expected) {
            throw "$name does not match the user tool manifest."
        }
    }
    $expectedNames = @('manifest.zut') + @($toolNames | ForEach-Object { "$_.exe" })
    $actualItems = @(Get-ChildItem -LiteralPath $bundleRoot -Force)
    if ($actualItems.Count -ne $expectedNames.Count) {
        throw 'The user tool bundle contains unexpected entries.'
    }
    foreach ($item in $actualItems) {
        if ($item.PSIsContainer -or $expectedNames -notcontains $item.Name) {
            throw 'The user tool bundle contains an unexpected entry.'
        }
    }
}

function Read-SlotState {
    if (-not (Test-Path -LiteralPath $statePath)) {
        return $null
    }
    Assert-NormalFile -Path $statePath -Label 'User tool slot state'
    if ((Get-Item -LiteralPath $statePath).Length -gt 512) {
        throw 'The user tool slot state is too large.'
    }
    $text = $utf8.GetString([IO.File]::ReadAllBytes($statePath))
    $lines = $text.TrimEnd("`r", "`n") -split "`n"
    if ($lines.Count -ne 3 -or $lines[0].TrimEnd("`r") -ne "schema=$schema") {
        throw 'The user tool slot state shape is invalid.'
    }
    $currentLine = $lines[1].TrimEnd("`r")
    $previousLine = $lines[2].TrimEnd("`r")
    if (-not $currentLine.StartsWith('current=', [StringComparison]::Ordinal) -or
        -not $previousLine.StartsWith('previous=', [StringComparison]::Ordinal)) {
        throw 'The user tool slot fields are invalid.'
    }
    $current = $currentLine.Substring('current='.Length)
    $previous = $previousLine.Substring('previous='.Length)
    if (-not (Test-BundleId -Value $current) -or
        ($previous -ne '-' -and -not (Test-BundleId -Value $previous))) {
        throw 'The user tool slot ids are invalid.'
    }
    $previousValue = if ($previous -eq '-') { $null } else { $previous }
    $canonicalPrevious = if ($null -eq $previousValue) { '-' } else { $previousValue }
    $canonical = "schema=$schema`r`ncurrent=$current`r`nprevious=$canonicalPrevious`r`n"
    return [PSCustomObject]@{
        Current = $current
        Previous = $previousValue
        Canonical = $text -ceq $canonical
    }
}

function Write-SlotState {
    param(
        [Parameter(Mandatory = $true)][string]$Current,
        [AllowNull()][AllowEmptyString()][string]$Previous
    )

    if (-not (Test-BundleId -Value $Current) -or
        (-not [string]::IsNullOrEmpty($Previous) -and -not (Test-BundleId -Value $Previous))) {
        throw 'Refusing to write invalid user tool slot ids.'
    }
    $previousValue = if ([string]::IsNullOrEmpty($Previous)) { '-' } else { $Previous }
    $body = "schema=$schema`r`ncurrent=$Current`r`nprevious=$previousValue`r`n"
    $temporary = Join-Path $UserToolsRoot ('.slots.tmp-' + [Guid]::NewGuid().ToString('N'))
    $backup = Join-Path $UserToolsRoot ('.slots.backup-' + [Guid]::NewGuid().ToString('N'))
    try {
        [IO.File]::WriteAllText($temporary, $body, $utf8)
        if (Test-Path -LiteralPath $statePath -PathType Leaf) {
            [IO.File]::Replace($temporary, $statePath, $backup)
        } else {
            [IO.File]::Move($temporary, $statePath)
        }
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) {
            Remove-Item -LiteralPath $temporary -Force
        }
        if (Test-Path -LiteralPath $backup -PathType Leaf) {
            Remove-Item -LiteralPath $backup -Force
        }
    }
}

function Short-Id {
    param([AllowNull()][AllowEmptyString()][string]$Value)

    if ([string]::IsNullOrEmpty($Value)) {
        return 'none'
    }
    return $Value.Substring(0, 12)
}

function Show-Status {
    $state = Read-SlotState
    Write-Host 'IME user tool status'
    if ($null -eq $state) {
        Write-Host 'Current: not published; launchers use target/release'
        Write-Host 'Previous: none'
        Write-Host 'Tools: 7 managed tools'
    } else {
        Assert-Bundle -BundleId $state.Current
        if ($null -ne $state.Previous) {
            Assert-Bundle -BundleId $state.Previous
        }
        Write-Host "Current: $(Short-Id -Value $state.Current) (verified)"
        Write-Host "Previous: $(Short-Id -Value $state.Previous)"
        Write-Host 'Tools: 7 verified executables'
    }
    Write-Host 'TSF DLL: unchanged'
    Write-Host 'Administrator: not required'
    Write-Host 'This action: read only'
}

function Publish-Bundle {
    $releaseRoot = Join-Path $cargoTarget 'release'
    $manifest = Get-ManifestBody -ReleaseRoot $releaseRoot
    $manifestBytes = $utf8.GetBytes($manifest)
    $bundleId = Get-BytesSha256Hex -Bytes $manifestBytes
    $bundleRoot = Join-Path $buildsRoot $bundleId
    if (Test-Path -LiteralPath $bundleRoot) {
        Assert-Bundle -BundleId $bundleId
        return $bundleId
    }
    $temporary = Join-Path $buildsRoot ('.bundle.tmp-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporary | Out-Null
    try {
        foreach ($tool in $toolNames) {
            $name = "$tool.exe"
            [IO.File]::Copy((Join-Path $releaseRoot $name), (Join-Path $temporary $name), $false)
        }
        [IO.File]::WriteAllBytes((Join-Path $temporary 'manifest.zut'), $manifestBytes)
        Move-Item -LiteralPath $temporary -Destination $bundleRoot
        Assert-Bundle -BundleId $bundleId
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Container) {
            Remove-Item -LiteralPath $temporary -Recurse -Force
        }
    }
    return $bundleId
}

if ($StatusOnly) {
    Show-Status
    exit 0
}

if ($Rollback) {
    Assert-NormalDirectory -Path $UserToolsRoot -Label 'User tool root'
    $lock = $null
    try {
        $lock = Open-RefreshLock
        $state = Read-SlotState
        if ($null -eq $state -or $null -eq $state.Previous) {
            throw 'There is no previous user tool bundle to restore.'
        }
        Assert-Bundle -BundleId $state.Current
        Assert-Bundle -BundleId $state.Previous
        Write-SlotState -Current $state.Previous -Previous $state.Current
        Write-Host 'IME user tool rollback completed'
        Write-Host "Current: $(Short-Id -Value $state.Previous)"
        Write-Host "Previous: $(Short-Id -Value $state.Current)"
        Write-Host 'TSF DLL: unchanged'
    } finally {
        if ($null -ne $lock) {
            $lock.Dispose()
        }
    }
    exit 0
}

$cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
if ($null -eq $cargo) {
    throw 'Cargo is missing from PATH. Nothing was published or installed.'
}
New-Item -ItemType Directory -Path $UserToolsRoot -Force | Out-Null
New-Item -ItemType Directory -Path $buildsRoot -Force | Out-Null
New-Item -ItemType Directory -Path $cargoTarget -Force | Out-Null
Assert-NormalDirectory -Path $UserToolsRoot -Label 'User tool root'
Assert-NormalDirectory -Path $buildsRoot -Label 'User tool builds root'
Assert-NormalDirectory -Path $cargoTarget -Label 'User tool Cargo target'
$lock = $null
try {
    $lock = Open-RefreshLock
    $arguments = @('build', '--release', '--locked', '--offline', '--target-dir', $cargoTarget)
    foreach ($tool in $toolNames) {
        $arguments += @('--bin', $tool)
    }
    & $cargo.Source @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "The isolated user tool build failed with exit code $LASTEXITCODE. Nothing was published or installed."
    }
    $publishedBundleIds = @(Publish-Bundle)
    if ($publishedBundleIds.Count -ne 1) {
        throw 'User tool publication returned an ambiguous bundle id.'
    }
    $bundleId = [string]$publishedBundleIds[0]
    if (-not (Test-BundleId -Value $bundleId)) {
        throw 'User tool publication returned an invalid bundle id.'
    }
    $state = Read-SlotState
    if ($null -eq $state) {
        Write-SlotState -Current $bundleId -Previous $null
        $result = 'published'
        $previous = $null
    } elseif ($state.Current -eq $bundleId) {
        Assert-Bundle -BundleId $bundleId
        if (-not $state.Canonical) {
            Write-SlotState -Current $state.Current -Previous $state.Previous
        }
        $result = 'already current'
        $previous = $state.Previous
    } else {
        Assert-Bundle -BundleId $state.Current
        Write-SlotState -Current $bundleId -Previous $state.Current
        $result = 'updated'
        $previous = $state.Current
    }
    Write-Host 'IME user tool refresh completed'
    Write-Host "Current: $(Short-Id -Value $bundleId) ($result)"
    Write-Host "Previous: $(Short-Id -Value $previous)"
    Write-Host 'Tools: alias, candidate, personal, research, and wish management'
    Write-Host 'TSF DLL: unchanged'
    Write-Host 'Administrator: not required'
    Write-Host 'Existing tool processes: unchanged; reopen them when convenient'
} finally {
    if ($null -ne $lock) {
        $lock.Dispose()
    }
}
