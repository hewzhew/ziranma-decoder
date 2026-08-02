[CmdletBinding()]
param(
    [string]$RepositoryRoot,
    [switch]$RequireClean
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..'
}
$resolvedRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$findings = [System.Collections.Generic.List[string]]::new()

function Add-Finding {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Code,
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $script:findings.Add("$Code`t$Path")
}

function Invoke-GitLines {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $output = @(& git -C $script:resolvedRoot @Arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        $message = ($output | ForEach-Object { $_.ToString() }) -join ' '
        throw "git $($Arguments -join ' ') failed: $message"
    }
    return @($output | ForEach-Object { $_.ToString() })
}

function Resolve-CandidatePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RelativePath
    )

    if ($RelativePath -match '[\x00-\x1f]' -or
        [System.IO.Path]::IsPathRooted($RelativePath) -or
        $RelativePath -match '(^|/)\.\.(/|$)') {
        Add-Finding -Code 'unsafe-candidate-path' -Path $RelativePath
        return $null
    }

    $platformPath = $RelativePath.Replace(
        '/',
        [System.IO.Path]::DirectorySeparatorChar
    )
    $fullPath = [System.IO.Path]::GetFullPath(
        (Join-Path $script:resolvedRoot $platformPath)
    )
    $rootPrefix = $script:resolvedRoot.TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar
    ) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $fullPath.StartsWith(
        $rootPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        Add-Finding -Code 'candidate-escapes-root' -Path $RelativePath
        return $null
    }
    return $fullPath
}

function Assert-RequiredFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RelativePath,
        [Parameter(Mandatory = $true)]
        [System.Collections.Generic.HashSet[string]]$CandidateSet
    )

    if (-not $CandidateSet.Contains($RelativePath)) {
        Add-Finding -Code 'missing-required-file' -Path $RelativePath
    }
}

function Assert-Sha256 {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RelativePath,
        [Parameter(Mandatory = $true)]
        [string]$Expected
    )

    $fullPath = Resolve-CandidatePath -RelativePath $RelativePath
    if ($null -eq $fullPath -or -not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        Add-Finding -Code 'missing-checksummed-file' -Path $RelativePath
        return
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $fullPath).Hash
    if ($actual -ne $Expected) {
        Add-Finding -Code 'sha256-mismatch' -Path $RelativePath
    }
}

$topLevel = @(Invoke-GitLines -Arguments @('rev-parse', '--show-toplevel'))
if ($topLevel.Count -ne 1) {
    throw 'expected exactly one Git top-level path'
}
$resolvedTopLevel = (Resolve-Path -LiteralPath $topLevel[0]).Path
if ($resolvedTopLevel -ne $resolvedRoot) {
    throw "RepositoryRoot must be the Git top level: $resolvedTopLevel"
}

$trackedPaths = @(
    Invoke-GitLines -Arguments @(
        '-c',
        'core.quotepath=false',
        'ls-files',
        '--cached'
    )
)
$untrackedPaths = @(
    Invoke-GitLines -Arguments @(
        '-c',
        'core.quotepath=false',
        'ls-files',
        '--others',
        '--exclude-standard'
    )
)
$candidatePaths = @(
    $trackedPaths + $untrackedPaths |
        Where-Object { $_ -ne '' } |
        Sort-Object -Unique
)
$candidateSet = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($relativePath in $candidatePaths) {
    [void]$candidateSet.Add($relativePath)
}

$statusLines = @(
    Invoke-GitLines -Arguments @(
        '-c',
        'core.quotepath=false',
        'status',
        '--porcelain=v1',
        '--untracked-files=all'
    )
)
if ($RequireClean -and $statusLines.Count -gt 0) {
    Add-Finding -Code 'dirty-worktree' -Path '.'
}

$stagedEntries = Invoke-GitLines -Arguments @('ls-files', '--stage')
foreach ($entry in $stagedEntries) {
    if ($entry.StartsWith('120000 ')) {
        $tab = $entry.IndexOf("`t")
        $path = if ($tab -ge 0) { $entry.Substring($tab + 1) } else { '<unknown>' }
        Add-Finding -Code 'tracked-symbolic-link' -Path $path
    }
}

$forbiddenPathPatterns = @(
    '^(data/private|data/raw|logs|models/private|\.local|tmp)(/|$)',
    '(^|/)\.env(\.|$)',
    '(^|/)(id_rsa|id_ed25519)$',
    '\.(pem|key|pfx|p12|db|sqlite|sqlite3)$'
)
$historyPathsChecked = 0
$historyObjectLines = @(
    Invoke-GitLines -Arguments @(
        '-c',
        'core.quotepath=false',
        'rev-list',
        '--objects',
        '--all'
    )
)
foreach ($objectLine in $historyObjectLines) {
    $separator = $objectLine.IndexOf(' ')
    if ($separator -lt 0 -or $separator -eq ($objectLine.Length - 1)) {
        continue
    }
    $historyPath = $objectLine.Substring($separator + 1)
    $historyPathsChecked += 1
    foreach ($pattern in $forbiddenPathPatterns) {
        if ($historyPath -match $pattern) {
            Add-Finding -Code 'forbidden-history-path' -Path $historyPath
        }
    }
}

$textPatterns = @(
    @{
        Code = 'private-key-material'
        Pattern = '-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----'
    },
    @{
        Code = 'aws-access-key'
        Pattern = '\bAKIA[0-9A-Z]{16}\b'
    },
    @{
        Code = 'github-token'
        Pattern = '\b(gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{20,})\b'
    },
    @{
        Code = 'personal-windows-user-path'
        Pattern = '(?i)\b[A-Z]:\\Users\\[^\\/\r\n]+'
    },
    @{
        Code = 'workspace-specific-path'
        Pattern = '(?i)\bD:\\IME(\\|$)'
    },
    @{
        Code = 'personal-email-in-project-material'
        Pattern = '(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b'
    }
)
$strictUtf8 = [System.Text.UTF8Encoding]::new($false, $true)
$maximumScannedTextBytes = 2MB
$scannedTextFiles = 0
$trustedPublicSnapshotFiles = [System.Collections.Generic.HashSet[string]]::new(
    [string[]]@(
        'data/public/conway-stroke-data/LICENSE.txt',
        'data/public/conway-stroke-data/SOURCE.md',
        'data/public/conway-stroke-data/UPSTREAM_README.md',
        'data/public/conway-stroke-data/sequence-characters.txt',
        'data/public/rime-pinyin-simp/AUTHORS',
        'data/public/rime-pinyin-simp/LICENSE',
        'data/public/rime-pinyin-simp/SOURCE.md',
        'data/public/rime-pinyin-simp/pinyin_simp.dict.yaml',
        'data/public/ud-chinese-gsdsimp/CC-BY-SA-4.0.txt',
        'data/public/ud-chinese-gsdsimp/LICENSE.txt',
        'data/public/ud-chinese-gsdsimp/SOURCE.md',
        'data/public/ud-chinese-gsdsimp/UPSTREAM_README.md',
        'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu',
        'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu'
    ),
    [System.StringComparer]::Ordinal
)

foreach ($relativePath in $candidatePaths) {
    $forbiddenReleasePath = $false
    foreach ($pattern in $forbiddenPathPatterns) {
        if ($relativePath -match $pattern) {
            Add-Finding -Code 'forbidden-release-path' -Path $relativePath
            $forbiddenReleasePath = $true
        }
    }
    if ($forbiddenReleasePath) {
        continue
    }

    $fullPath = Resolve-CandidatePath -RelativePath $relativePath
    if ($null -eq $fullPath) {
        continue
    }
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        Add-Finding -Code 'candidate-file-missing' -Path $relativePath
        continue
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        Add-Finding -Code 'candidate-reparse-point' -Path $relativePath
        continue
    }

    # Exact frozen third-party files are protected by mandatory SHA-256 values
    # below. Project-owned public overlays and any newly added snapshot file
    # still pass through the normal secret and personal-path checks.
    if ($trustedPublicSnapshotFiles.Contains($relativePath)) {
        continue
    }
    if ($item.Length -gt $maximumScannedTextBytes) {
        Add-Finding -Code 'unscanned-large-project-file' -Path $relativePath
        continue
    }

    try {
        $bytes = [System.IO.File]::ReadAllBytes($fullPath)
        if ([System.Array]::IndexOf($bytes, [byte]0) -ge 0) {
            continue
        }
        $text = $strictUtf8.GetString($bytes)
    }
    catch {
        Add-Finding -Code 'invalid-utf8-project-file' -Path $relativePath
        continue
    }

    $scannedTextFiles += 1
    foreach ($check in $textPatterns) {
        if ([System.Text.RegularExpressions.Regex]::IsMatch(
            $text,
            $check.Pattern
        )) {
            Add-Finding -Code $check.Code -Path $relativePath
        }
    }
}

$requiredFiles = @(
    '.github/workflows/ci.yml',
    '.gitattributes',
    '.gitignore',
    'Cargo.lock',
    'Cargo.toml',
    'CONTRIBUTING.md',
    'LICENSE',
    'PRIVACY.md',
    'README.md',
    'THIRD_PARTY_NOTICES.md',
    'data/public/conway-stroke-data/LICENSE.txt',
    'data/public/conway-stroke-data/SOURCE.md',
    'data/public/conway-stroke-data/UPSTREAM_README.md',
    'data/public/conway-stroke-data/sequence-characters.txt',
    'data/public/rime-pinyin-simp/AUTHORS',
    'data/public/rime-pinyin-simp/LICENSE',
    'data/public/rime-pinyin-simp/SOURCE.md',
    'data/public/rime-pinyin-simp/pinyin_simp.dict.yaml',
    'data/public/ud-chinese-gsdsimp/CC-BY-SA-4.0.txt',
    'data/public/ud-chinese-gsdsimp/LICENSE.txt',
    'data/public/ud-chinese-gsdsimp/SOURCE.md',
    'data/public/ud-chinese-gsdsimp/UPSTREAM_README.md',
    'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu',
    'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu',
    'docs/candidate-lab.md',
    'docs/open-source-boundary-audit.md',
    'docs/tsf-alpha.md',
    'docs/tsf-dev-inspection.md',
    'scripts/release-audit.ps1',
    'src/bin/tsf-devctl.rs',
    'src/candidate_lab_cli.rs',
    'src/tsf_alpha.rs'
)
foreach ($requiredFile in $requiredFiles) {
    Assert-RequiredFile -RelativePath $requiredFile -CandidateSet $candidateSet
}

$expectedHashes = @{
    'LICENSE' =
        '3F3D9E0024B1921B067D6F7F88DEB4A60CBE7A78E76C64E3F1D7FC3B779B9D04'
    'data/public/conway-stroke-data/LICENSE.txt' =
        '9BA9550AD48438D0836DDAB3DA480B3B69FFA0AAC7B7878B5A0039E7AB429411'
    'data/public/conway-stroke-data/SOURCE.md' =
        '0FE0FEC10FF29642D95D4EC7130E577948FFBDF98837A72E5ACB3E83C74EEDB4'
    'data/public/conway-stroke-data/UPSTREAM_README.md' =
        '00BE0D69159CCBF87C97A07398878E660BD4F735031F58B8BC110B8CB6640D2C'
    'data/public/conway-stroke-data/sequence-characters.txt' =
        'E712D1AC5B67E4F12B1904AEC020F2CB3E3C36C15FB11BDD7AF671F66B41CA68'
    'data/public/rime-pinyin-simp/AUTHORS' =
        'F4CFF0FCBCA4668AC449C24A53BE547E162BC60CCE63FDC5D5906801A452EDC4'
    'data/public/rime-pinyin-simp/LICENSE' =
        'CFC7749B96F63BD31C3C42B5C471BF756814053E847C10F3EB003417BC523D30'
    'data/public/rime-pinyin-simp/SOURCE.md' =
        'DC3CFD72CBDD7403357094A411BA2EF1DBDCF3595034726289234B11995C13BC'
    'data/public/rime-pinyin-simp/pinyin_simp.dict.yaml' =
        'E341598343A0F0F2035BB1AAFC34A7F3BB7887DEEECB3F60796262AAA2983E6B'
    'data/public/ud-chinese-gsdsimp/CC-BY-SA-4.0.txt' =
        '28A9529C7D0BB4DC51F4BF5C116A3D16EF247A052F7591466768DDF563FD1CF5'
    'data/public/ud-chinese-gsdsimp/LICENSE.txt' =
        '899B1804A12EBC090B96339614EEDE1B64B686721B650A71430B55B5235F7F79'
    'data/public/ud-chinese-gsdsimp/SOURCE.md' =
        'C3BC1A8A5305AECB00AFD939D11CB31A306C45A0D334B79B2AD730B5C745C5C0'
    'data/public/ud-chinese-gsdsimp/UPSTREAM_README.md' =
        '02287BDF80282151D8CA7EF3C3F7A3C3B98609F7266F145E3E3DD0A05693ABD3'
    'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu' =
        '3AF8046A6F32477B4D5CF3DD06BBF38682A380FE77AADE3F68DE97E51AB94900'
    'data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu' =
        '956636FE612A1166E8B19E7413FEE2E73D68231ACA2F0455BE2C616B947D629D'
}
foreach ($relativePath in $expectedHashes.Keys | Sort-Object) {
    Assert-Sha256 -RelativePath $relativePath -Expected $expectedHashes[$relativePath]
}

$cargoToml = Get-Content -LiteralPath (Join-Path $resolvedRoot 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^license = "MPL-2\.0"$') {
    Add-Finding -Code 'cargo-license-mismatch' -Path 'Cargo.toml'
}
if ($cargoToml -notmatch '(?m)^publish = false$') {
    Add-Finding -Code 'crates-io-publish-not-disabled' -Path 'Cargo.toml'
}

$gitignore = Get-Content -LiteralPath (Join-Path $resolvedRoot '.gitignore') -Raw
foreach ($requiredIgnore in @(
    '/data/private/',
    '/data/raw/',
    '/logs/',
    '/models/private/',
    '/.local/',
    '/tmp/'
)) {
    if (-not $gitignore.Contains($requiredIgnore)) {
        Add-Finding -Code 'missing-private-ignore' -Path $requiredIgnore
    }
}

$attributes = Get-Content -LiteralPath (Join-Path $resolvedRoot '.gitattributes') -Raw
foreach ($requiredAttribute in @(
    'data/public/rime-pinyin-simp/* -text',
    'data/public/ud-chinese-gsdsimp/* -text',
    'data/public/conway-stroke-data/* -text'
)) {
    if (-not $attributes.Contains($requiredAttribute)) {
        Add-Finding -Code 'missing-snapshot-attribute' -Path $requiredAttribute
    }
}

$remoteCount = @(
    Invoke-GitLines -Arguments @('remote')
).Count
Write-Output (
    (
        'RELEASE_AUDIT_SUMMARY candidate_files={0} tracked_files={1} ' +
        'untracked_files={2} history_paths_checked={3} ' +
        'scanned_text_files={4} dirty_entries={5} remotes={6} ' +
        'require_clean={7} network=false private_scan=false'
    ) -f @(
        $candidatePaths.Count,
        $trackedPaths.Count,
        $untrackedPaths.Count,
        $historyPathsChecked,
        $scannedTextFiles,
        $statusLines.Count,
        $remoteCount,
        $RequireClean.IsPresent.ToString().ToLowerInvariant()
    )
)

if ($findings.Count -gt 0) {
    foreach ($finding in $findings | Sort-Object -Unique) {
        $parts = $finding.Split("`t", 2)
        [Console]::Error.WriteLine(
            "RELEASE_AUDIT_ERROR code=$($parts[0]) path=`"$($parts[1])`""
        )
    }
    [Console]::Error.WriteLine(
        "RELEASE_AUDIT_FAILED findings=$($findings.Count)"
    )
    exit 1
}

if ($statusLines.Count -gt 0) {
    Write-Warning 'worktree is dirty; rerun with -RequireClean for a final release candidate'
}
Write-Output 'RELEASE_AUDIT_PASSED'
