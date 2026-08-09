[CmdletBinding()]
param(
    [switch]$StatusOnly,
    [switch]$SpaceOnly,
    [switch]$Cleanup,
    [switch]$ConfirmCleanupUnreferencedBundles,
    [switch]$Rollback,
    [string]$UserToolsRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$operationCount = [int][bool]$StatusOnly + [int][bool]$SpaceOnly + [int][bool]$Cleanup +
    [int][bool]$Rollback
if ($operationCount -gt 1) {
    throw 'StatusOnly, SpaceOnly, Cleanup, and Rollback cannot be combined.'
}
if ($ConfirmCleanupUnreferencedBundles -and -not $Cleanup) {
    throw 'ConfirmCleanupUnreferencedBundles requires Cleanup.'
}
if ($Cleanup -and -not $ConfirmCleanupUnreferencedBundles) {
    throw 'Cleanup requires ConfirmCleanupUnreferencedBundles.'
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$manifestPath = Join-Path $repositoryRoot 'Cargo.toml'
if ([string]::IsNullOrWhiteSpace($UserToolsRoot)) {
    $UserToolsRoot = Join-Path $repositoryRoot '.local\tsf-alpha\user-tools'
}
$UserToolsRoot = [IO.Path]::GetFullPath($UserToolsRoot)
$buildsRoot = Join-Path $UserToolsRoot 'builds'
$cargoTarget = Join-Path $UserToolsRoot 'cargo-target'
$statePath = Join-Path $UserToolsRoot 'slots.zut'
$lockPath = Join-Path $UserToolsRoot 'refresh.lock'
$desktopLauncherRoot = Join-Path $repositoryRoot '.local\tsf-alpha\desktop-launcher'
$desktopLauncherPath = Join-Path $desktopLauncherRoot 'ziranma-launcher.exe'
$schema = 'ziranma-user-tools-slots-v1'
$legacyBundleSchema = 'ziranma-user-tools-bundle-v1'
$bundleSchema = 'ziranma-user-tools-bundle-v2'
$legacyToolNames = @(
    'aliasctl',
    'aliaspad',
    'candidatectl',
    'personalctl',
    'researchctl',
    'wishctl',
    'wishpad'
)
$toolNames = @(
    'aliasctl',
    'aliaspad',
    'candidatectl',
    'personalctl',
    'researchctl',
    'wishctl',
    'wishpad',
    'ziranma-launcher'
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
    $manifestSchema = $lines[0].TrimEnd("`r")
    if ($manifestSchema -eq "schema=$bundleSchema") {
        $manifestToolNames = @($toolNames)
    } elseif ($manifestSchema -eq "schema=$legacyBundleSchema") {
        $manifestToolNames = @($legacyToolNames)
    } else {
        throw 'The user tool manifest schema is invalid.'
    }
    if ($lines.Count -ne $manifestToolNames.Count + 1) {
        throw 'The user tool manifest shape is invalid.'
    }
    for ($index = 0; $index -lt $manifestToolNames.Count; $index++) {
        $name = "$($manifestToolNames[$index]).exe"
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
    $expectedNames = @('manifest.zut') + @($manifestToolNames | ForEach-Object { "$_.exe" })
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

function Publish-DesktopLauncher {
    param([Parameter(Mandatory = $true)][string]$BundleId)

    Assert-Bundle -BundleId $BundleId
    $source = Join-Path (Join-Path $buildsRoot $BundleId) 'ziranma-launcher.exe'
    Assert-NormalFile -Path $source -Label 'Bundled desktop launcher'
    $expected = Get-Sha256Hex -Path $source
    New-Item -ItemType Directory -Path $desktopLauncherRoot -Force | Out-Null
    Assert-NormalDirectory -Path $desktopLauncherRoot -Label 'Desktop launcher root'
    if (Test-Path -LiteralPath $desktopLauncherPath) {
        Assert-NormalFile -Path $desktopLauncherPath -Label 'Desktop launcher'
        if ((Get-Sha256Hex -Path $desktopLauncherPath) -eq $expected) {
            return
        }
    }

    $temporary = Join-Path $desktopLauncherRoot ('.launcher.tmp-' + [Guid]::NewGuid().ToString('N'))
    $backup = Join-Path $desktopLauncherRoot ('.launcher.backup-' + [Guid]::NewGuid().ToString('N'))
    $replaced = $false
    try {
        [IO.File]::Copy($source, $temporary, $false)
        Assert-NormalFile -Path $temporary -Label 'Temporary desktop launcher'
        if ((Get-Sha256Hex -Path $temporary) -ne $expected) {
            throw 'The temporary desktop launcher digest is invalid.'
        }
        if (Test-Path -LiteralPath $desktopLauncherPath -PathType Leaf) {
            [IO.File]::Replace($temporary, $desktopLauncherPath, $backup)
        } else {
            [IO.File]::Move($temporary, $desktopLauncherPath)
        }
        $replaced = $true
        Assert-NormalFile -Path $desktopLauncherPath -Label 'Desktop launcher'
        if ((Get-Sha256Hex -Path $desktopLauncherPath) -ne $expected) {
            throw 'The installed desktop launcher digest is invalid.'
        }
    } catch {
        if ($replaced -and (Test-Path -LiteralPath $backup -PathType Leaf)) {
            if (Test-Path -LiteralPath $desktopLauncherPath -PathType Leaf) {
                Remove-Item -LiteralPath $desktopLauncherPath -Force
            }
            Move-Item -LiteralPath $backup -Destination $desktopLauncherPath
        } elseif ($replaced -and (Test-Path -LiteralPath $desktopLauncherPath -PathType Leaf)) {
            Remove-Item -LiteralPath $desktopLauncherPath -Force
        }
        throw
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) {
            Remove-Item -LiteralPath $temporary -Force
        }
        if (Test-Path -LiteralPath $backup -PathType Leaf) {
            Remove-Item -LiteralPath $backup -Force
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
        ($previous -ne '-' -and
            (-not (Test-BundleId -Value $previous) -or $previous -eq $current))) {
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
        (-not [string]::IsNullOrEmpty($Previous) -and
            (-not (Test-BundleId -Value $Previous) -or $Previous -eq $Current))) {
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

function Get-DirectoryUsage {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return [PSCustomObject]@{
            Bytes = [long]0
            Files = [long]0
            Directories = [long]0
        }
    }
    Assert-NormalDirectory -Path $Path -Label $Label
    $pending = New-Object 'Collections.Generic.Stack[string]'
    $pending.Push($Path)
    [long]$bytes = 0
    [long]$files = 0
    [long]$directories = 0
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Label contains a reparse point; refusing an ambiguous size report."
            }
            if ($item.PSIsContainer) {
                $directories++
                $pending.Push($item.FullName)
            } else {
                $files++
                $bytes += [long]$item.Length
            }
        }
    }
    return [PSCustomObject]@{
        Bytes = $bytes
        Files = $files
        Directories = $directories
    }
}

function Format-ByteSize {
    param([Parameter(Mandatory = $true)][long]$Bytes)

    $units = @('B', 'KiB', 'MiB', 'GiB', 'TiB')
    [double]$value = $Bytes
    $unitIndex = 0
    while ($value -ge 1024 -and $unitIndex -lt $units.Count - 1) {
        $value /= 1024
        $unitIndex++
    }
    if ($unitIndex -eq 0) {
        return "$Bytes B"
    }
    return [string]::Format(
        [Globalization.CultureInfo]::InvariantCulture,
        '{0:0.00} {1}',
        $value,
        $units[$unitIndex]
    )
}

function Show-SpaceUsage {
    $state = Read-SlotState
    $cargoUsage = Get-DirectoryUsage -Path $cargoTarget -Label 'User tool Cargo target'
    [long]$bundleBytes = 0
    [long]$currentBytes = 0
    [long]$previousBytes = 0
    [long]$unreferencedBytes = 0
    [long]$unrecognizedBytes = 0
    [long]$bundleCount = 0
    [long]$unreferencedCount = 0
    [long]$unrecognizedCount = 0
    [long]$otherRootBytes = 0
    [long]$otherRootCount = 0
    [long]$auxiliaryBytes = 0
    $currentFound = $false
    $previousFound = $false

    if (Test-Path -LiteralPath $UserToolsRoot) {
        Assert-NormalDirectory -Path $UserToolsRoot -Label 'User tool root'
        foreach ($item in @(Get-ChildItem -LiteralPath $UserToolsRoot -Force)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'User tool root contains a reparse point; refusing an ambiguous size report.'
            }
            if ($item.Name -in @('builds', 'cargo-target')) {
                continue
            }
            if ($item.Name -in @('slots.zut', 'refresh.lock')) {
                if (-not $item.PSIsContainer) {
                    $auxiliaryBytes += [long]$item.Length
                }
                continue
            }
            if ($item.PSIsContainer) {
                $usage = Get-DirectoryUsage -Path $item.FullName -Label 'Unmanaged user tool entry'
                $otherRootBytes += [long]$usage.Bytes
            } else {
                $otherRootBytes += [long]$item.Length
            }
            $otherRootCount++
        }
    }

    if (Test-Path -LiteralPath $buildsRoot) {
        Assert-NormalDirectory -Path $buildsRoot -Label 'User tool builds root'
        foreach ($item in @(Get-ChildItem -LiteralPath $buildsRoot -Force)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'User tool builds root contains a reparse point; refusing an ambiguous size report.'
            }
            if ($item.PSIsContainer) {
                $usage = Get-DirectoryUsage -Path $item.FullName -Label 'User tool bundle entry'
                $entryBytes = [long]$usage.Bytes
            } else {
                $entryBytes = [long]$item.Length
            }
            if ($item.PSIsContainer -and (Test-BundleId -Value $item.Name)) {
                $bundleCount++
                $bundleBytes += $entryBytes
                if ($null -ne $state -and $item.Name -eq $state.Current) {
                    $currentFound = $true
                    $currentBytes = $entryBytes
                } elseif ($null -ne $state -and $null -ne $state.Previous -and
                    $item.Name -eq $state.Previous) {
                    $previousFound = $true
                    $previousBytes = $entryBytes
                } else {
                    $unreferencedCount++
                    $unreferencedBytes += $entryBytes
                }
            } else {
                $unrecognizedCount++
                $unrecognizedBytes += $entryBytes
            }
        }
    }

    [long]$totalBytes = [long]$cargoUsage.Bytes + $bundleBytes + $unrecognizedBytes +
        $otherRootBytes + $auxiliaryBytes
    Write-Host 'IME user tool disk usage'
    Write-Host "Root: $UserToolsRoot"
    Write-Host "Total footprint: $(Format-ByteSize -Bytes $totalBytes)"
    Write-Host "Cargo cache: $(Format-ByteSize -Bytes $cargoUsage.Bytes) ($($cargoUsage.Files) files; rebuildable, retained)"
    Write-Host "Immutable bundles: $bundleCount, $(Format-ByteSize -Bytes $bundleBytes)"
    if ($null -eq $state) {
        Write-Host 'Current bundle: none'
        Write-Host 'Previous bundle: none'
    } else {
        if ($currentFound) {
            Write-Host "Current bundle: $(Short-Id -Value $state.Current), $(Format-ByteSize -Bytes $currentBytes)"
        } else {
            Write-Host "Current bundle: $(Short-Id -Value $state.Current), missing from builds"
        }
        if ($null -eq $state.Previous) {
            Write-Host 'Previous bundle: none'
        } elseif ($previousFound) {
            Write-Host "Previous bundle: $(Short-Id -Value $state.Previous), $(Format-ByteSize -Bytes $previousBytes)"
        } else {
            Write-Host "Previous bundle: $(Short-Id -Value $state.Previous), missing from builds"
        }
    }
    Write-Host "Unreferenced bundles: $unreferencedCount, $(Format-ByteSize -Bytes $unreferencedBytes)"
    Write-Host "Unrecognized build entries: $unrecognizedCount, $(Format-ByteSize -Bytes $unrecognizedBytes)"
    Write-Host "Other root entries: $otherRootCount, $(Format-ByteSize -Bytes $otherRootBytes) (unmanaged, retained)"
    Write-Host "Potential reclaim: $(Format-ByteSize -Bytes $unreferencedBytes) (unreferenced bundles; process use not checked)"
    Write-Host 'No files were deleted'
    Write-Host 'TSF DLL: unchanged'
    Write-Host 'Administrator: not required'
    Write-Host 'This action: read only'
}

function Get-RunningToolBundleIds {
    $bundleIds = New-Object 'Collections.Generic.HashSet[string]' (
        [StringComparer]::OrdinalIgnoreCase
    )
    $canonicalBuildsRoot = [IO.Path]::GetFullPath($buildsRoot).TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    foreach ($tool in $toolNames) {
        foreach ($process in @(Get-Process -Name $tool -ErrorAction SilentlyContinue)) {
            try {
                $executablePath = $process.Path
            } catch {
                throw "A running $tool process could not be inspected; cleanup was not started."
            }
            if ([string]::IsNullOrWhiteSpace($executablePath)) {
                throw "A running $tool process has no inspectable path; cleanup was not started."
            }
            $executable = [IO.Path]::GetFullPath($executablePath)
            $bundle = [IO.Directory]::GetParent($executable)
            if ($null -eq $bundle -or -not (Test-BundleId -Value $bundle.Name)) {
                continue
            }
            $parent = $bundle.Parent
            if ($null -ne $parent -and
                [string]::Equals(
                    $parent.FullName.TrimEnd(
                        [IO.Path]::DirectorySeparatorChar,
                        [IO.Path]::AltDirectorySeparatorChar
                    ),
                    $canonicalBuildsRoot,
                    [StringComparison]::OrdinalIgnoreCase
                )) {
                [void]$bundleIds.Add($bundle.Name)
            }
        }
    }
    return ,$bundleIds
}

function Remove-UnreferencedBundles {
    Assert-NormalDirectory -Path $UserToolsRoot -Label 'User tool root'
    Assert-NormalDirectory -Path $buildsRoot -Label 'User tool builds root'
    $lock = $null
    try {
        $lock = Open-RefreshLock
        $state = Read-SlotState
        if ($null -eq $state) {
            throw 'There is no current user tool bundle to protect; cleanup was not started.'
        }
        Assert-Bundle -BundleId $state.Current
        if ($null -ne $state.Previous) {
            Assert-Bundle -BundleId $state.Previous
        }

        $candidates = @()
        [long]$unrecognizedCount = 0
        foreach ($item in @(Get-ChildItem -LiteralPath $buildsRoot -Force | Sort-Object Name)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'User tool builds root contains a reparse point; cleanup was not started.'
            }
            if (-not $item.PSIsContainer -or -not (Test-BundleId -Value $item.Name)) {
                $unrecognizedCount++
                continue
            }
            if ($item.Name -eq $state.Current -or
                ($null -ne $state.Previous -and $item.Name -eq $state.Previous)) {
                continue
            }
            Assert-Bundle -BundleId $item.Name
            $usage = Get-DirectoryUsage -Path $item.FullName -Label 'Unreferenced user tool bundle'
            $candidates += [PSCustomObject]@{
                Id = $item.Name
                Path = $item.FullName
                Bytes = [long]$usage.Bytes
            }
        }

        $running = Get-RunningToolBundleIds
        [long]$removedCount = 0
        [long]$removedBytes = 0
        [long]$inUseCount = 0
        [long]$inUseBytes = 0
        foreach ($candidate in $candidates) {
            if ($running.Contains($candidate.Id)) {
                $inUseCount++
                $inUseBytes += $candidate.Bytes
                continue
            }
            $running = Get-RunningToolBundleIds
            if ($running.Contains($candidate.Id)) {
                $inUseCount++
                $inUseBytes += $candidate.Bytes
                continue
            }
            $expected = Join-Path $buildsRoot $candidate.Id
            if (-not [string]::Equals(
                [IO.Path]::GetFullPath($candidate.Path),
                [IO.Path]::GetFullPath($expected),
                [StringComparison]::OrdinalIgnoreCase
            )) {
                throw 'An unreferenced bundle path changed during cleanup.'
            }
            Assert-Bundle -BundleId $candidate.Id
            Remove-Item -LiteralPath $candidate.Path -Recurse -Force
            if (Test-Path -LiteralPath $candidate.Path) {
                throw 'An unreferenced bundle could not be removed completely.'
            }
            $removedCount++
            $removedBytes += $candidate.Bytes
        }

        $stateAfter = Read-SlotState
        if ($null -eq $stateAfter -or
            $stateAfter.Current -ne $state.Current -or
            $stateAfter.Previous -ne $state.Previous) {
            throw 'The protected user tool slots changed during cleanup.'
        }
        Assert-Bundle -BundleId $stateAfter.Current
        if ($null -ne $stateAfter.Previous) {
            Assert-Bundle -BundleId $stateAfter.Previous
        }
        Write-Host 'IME user tool cleanup completed'
        Write-Host "Removed bundles: $removedCount, $(Format-ByteSize -Bytes $removedBytes)"
        Write-Host "Running bundles retained: $inUseCount, $(Format-ByteSize -Bytes $inUseBytes)"
        Write-Host "Unrecognized build entries retained: $unrecognizedCount"
        Write-Host "Current protected: $(Short-Id -Value $stateAfter.Current)"
        Write-Host "Previous protected: $(Short-Id -Value $stateAfter.Previous)"
        Write-Host 'Cargo cache and other root entries: unchanged'
        Write-Host 'TSF DLL: unchanged'
        Write-Host 'Administrator: not required'
    } finally {
        if ($null -ne $lock) {
            $lock.Dispose()
        }
    }
}

function Show-Status {
    $state = Read-SlotState
    Write-Host 'IME user tool status'
    if ($null -eq $state) {
        Write-Host 'Current: not published; launchers use target/release'
        Write-Host 'Previous: none'
        Write-Host 'Tools: 8 managed tools'
    } else {
        Assert-Bundle -BundleId $state.Current
        if ($null -ne $state.Previous) {
            Assert-Bundle -BundleId $state.Previous
        }
        Write-Host "Current: $(Short-Id -Value $state.Current) (verified)"
        Write-Host "Previous: $(Short-Id -Value $state.Previous)"
        Write-Host 'Tools: verified executables'
    }
    if (Test-Path -LiteralPath $desktopLauncherPath -PathType Leaf) {
        Assert-NormalFile -Path $desktopLauncherPath -Label 'Desktop launcher'
        Write-Host 'Desktop launcher: installed'
    } else {
        Write-Host 'Desktop launcher: not installed; refresh to publish it'
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

if ($SpaceOnly) {
    Show-SpaceUsage
    exit 0
}

if ($Cleanup) {
    Remove-UnreferencedBundles
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
Assert-NormalFile -Path $manifestPath -Label 'Cargo manifest'
New-Item -ItemType Directory -Path $UserToolsRoot -Force | Out-Null
New-Item -ItemType Directory -Path $buildsRoot -Force | Out-Null
New-Item -ItemType Directory -Path $cargoTarget -Force | Out-Null
Assert-NormalDirectory -Path $UserToolsRoot -Label 'User tool root'
Assert-NormalDirectory -Path $buildsRoot -Label 'User tool builds root'
Assert-NormalDirectory -Path $cargoTarget -Label 'User tool Cargo target'
$lock = $null
try {
    $lock = Open-RefreshLock
    $arguments = @(
        'build',
        '--manifest-path', $manifestPath,
        '--release',
        '--locked',
        '--offline',
        '--target-dir', $cargoTarget
    )
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
    Publish-DesktopLauncher -BundleId $bundleId
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
    Write-Host 'Tools: alias, candidate, personal, research, wish, and desktop launch management'
    Write-Host 'TSF DLL: unchanged'
    Write-Host 'Administrator: not required'
    Write-Host 'Existing tool processes: unchanged; reopen them when convenient'
} finally {
    if ($null -ne $lock) {
        $lock.Dispose()
    }
}
