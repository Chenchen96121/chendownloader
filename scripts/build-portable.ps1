[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$VendorRoot,

    [string]$GalleryDlSourceArchive,

    [string]$TikTokCorePath,

    [string]$OutputRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.release"))
}
if ([string]::IsNullOrWhiteSpace($GalleryDlSourceArchive)) {
    throw "GalleryDlSourceArchive is required and must point to the pinned gallery-dl 1.32.15 source archive."
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Program,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code $LASTEXITCODE`: $Program $($Arguments -join ' ')"
    }
}

function Test-PathWithin {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Parent,

        [Parameter(Mandatory = $true)]
        [string]$Candidate
    )

    $normalizedParent = $Parent.TrimEnd([IO.Path]::DirectorySeparatorChar)
    $normalizedCandidate = $Candidate.TrimEnd([IO.Path]::DirectorySeparatorChar)
    $parentWithSeparator = $normalizedParent + [IO.Path]::DirectorySeparatorChar
    return $normalizedCandidate.Equals($normalizedParent, [StringComparison]::OrdinalIgnoreCase) -or
        $normalizedCandidate.StartsWith($parentWithSeparator, [StringComparison]::OrdinalIgnoreCase)
}

function New-DeterministicZip {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDirectory,

        [Parameter(Mandatory = $true)]
        [string]$DestinationZip
    )

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [IO.File]::Open($DestinationZip, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $archive = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create, $false)
        try {
            $sourceParent = Split-Path -Parent $SourceDirectory
            Get-ChildItem -LiteralPath $SourceDirectory -File -Recurse |
                Sort-Object { $_.FullName.Substring($sourceParent.Length + 1).Replace("\", "/") } |
                ForEach-Object {
                    $entryName = $_.FullName.Substring($sourceParent.Length + 1).Replace("\", "/")
                    $entry = $archive.CreateEntry($entryName, [IO.Compression.CompressionLevel]::Optimal)
                    $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
                    $input = [IO.File]::OpenRead($_.FullName)
                    try {
                        $output = $entry.Open()
                        try {
                            $input.CopyTo($output)
                        }
                        finally {
                            $output.Dispose()
                        }
                    }
                    finally {
                        $input.Dispose()
                    }
                }
        }
        finally {
            $archive.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }
}

if (-not [IO.Path]::IsPathRooted($VendorRoot)) {
    throw "VendorRoot must be an absolute directory outside the source tree."
}
if (-not [IO.Path]::IsPathRooted($GalleryDlSourceArchive)) {
    throw "GalleryDlSourceArchive must be an absolute file path outside the source tree."
}
if (-not [IO.Path]::IsPathRooted($OutputRoot)) {
    throw "OutputRoot must be an absolute directory (the default .release directory is allowed)."
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$resolvedVendorRoot = (Resolve-Path -LiteralPath $VendorRoot).Path
$resolvedGalleryDlSourceArchive = (Resolve-Path -LiteralPath $GalleryDlSourceArchive).Path
$resolvedOutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$defaultOutputRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot ".release"))
$isCustomOutputRoot = -not $resolvedOutputRoot.Equals($defaultOutputRoot, [StringComparison]::OrdinalIgnoreCase)

$filesystemRoot = [IO.Path]::GetPathRoot($resolvedOutputRoot)
if ($resolvedOutputRoot.TrimEnd([IO.Path]::DirectorySeparatorChar).Equals(
        $filesystemRoot.TrimEnd([IO.Path]::DirectorySeparatorChar),
        [StringComparison]::OrdinalIgnoreCase
    )) {
    throw "OutputRoot may not be a filesystem root or drive root."
}
$userProfile = [IO.Path]::GetFullPath($env:USERPROFILE)
if ($resolvedOutputRoot.Equals($userProfile, [StringComparison]::OrdinalIgnoreCase)) {
    throw "OutputRoot may not be the protected user profile directory."
}
$protectedRoots = @(
    $env:SystemRoot,
    $env:ProgramFiles,
    ${env:ProgramFiles(x86)},
    $env:ProgramData,
    $env:LOCALAPPDATA,
    $env:APPDATA
) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { [IO.Path]::GetFullPath($_) }
foreach ($protectedRoot in $protectedRoots) {
    if (Test-PathWithin -Parent $protectedRoot -Candidate $resolvedOutputRoot) {
        throw "OutputRoot may not be inside a protected system or profile directory."
    }
}
if ($isCustomOutputRoot -and (Test-Path -LiteralPath $resolvedOutputRoot)) {
    $outputItem = Get-Item -LiteralPath $resolvedOutputRoot
    if (-not $outputItem.PSIsContainer) {
        throw "A custom OutputRoot must be a fresh empty directory."
    }
    if (Get-ChildItem -LiteralPath $resolvedOutputRoot -Force | Select-Object -First 1) {
        throw "A custom OutputRoot must be fresh and empty."
    }
}

if ((Get-Item -LiteralPath $resolvedVendorRoot).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw "VendorRoot may not be a symlink, junction, or other reparse point."
}
$galleryDlSourceItem = Get-Item -LiteralPath $resolvedGalleryDlSourceArchive
if (-not $galleryDlSourceItem.PSIsContainer -and ($galleryDlSourceItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "GalleryDlSourceArchive may not be a symlink or other reparse point."
}
if ($galleryDlSourceItem.PSIsContainer) {
    throw "GalleryDlSourceArchive must be a regular file."
}
if (
    (Test-Path -LiteralPath $resolvedOutputRoot) -and
    ((Get-Item -LiteralPath $resolvedOutputRoot).Attributes -band [IO.FileAttributes]::ReparsePoint)
) {
    throw "OutputRoot may not be a symlink, junction, or other reparse point."
}
if (Test-PathWithin -Parent $repositoryRoot -Candidate $resolvedVendorRoot) {
    throw "VendorRoot must be outside the tracked source tree."
}
if (Test-PathWithin -Parent $repositoryRoot -Candidate $resolvedGalleryDlSourceArchive) {
    throw "GalleryDlSourceArchive must be outside the tracked source tree."
}
if (
    $resolvedOutputRoot -ne $defaultOutputRoot -and
    (Test-PathWithin -Parent $repositoryRoot -Candidate $resolvedOutputRoot)
) {
    throw "A custom OutputRoot must be outside the tracked source tree; omit it to use .release."
}
if (Test-PathWithin -Parent $resolvedVendorRoot -Candidate $resolvedOutputRoot) {
    throw "OutputRoot may not be inside VendorRoot."
}

$applicationVersion = (Get-Content -LiteralPath (Join-Path $repositoryRoot "package.json") -Raw | ConvertFrom-Json).version
if ($applicationVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw "package.json contains an invalid portable release version."
}
if ([version]$applicationVersion -ge [version]'1.1.0') {
    if ([string]::IsNullOrWhiteSpace($TikTokCorePath) -or -not [IO.Path]::IsPathRooted($TikTokCorePath)) { throw 'TikTokCorePath must point to the separately built TikTok component.' }
    $tikTokInput=Get-Item -LiteralPath $TikTokCorePath
    if ($tikTokInput.PSIsContainer -or ($tikTokInput.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'TikTokCorePath must be a regular executable file.' }
}
$releaseName = "chendownloader-$applicationVersion-windows-x64"
$stagingRoot = Join-Path $resolvedOutputRoot $releaseName
$zipPath = Join-Path $resolvedOutputRoot "$releaseName.zip"
$hashPath = "$zipPath.sha256"
$applicationPath = Join-Path $repositoryRoot "src-tauri\target\release\chendownloader.exe"

New-Item -ItemType Directory -Force -Path $resolvedOutputRoot | Out-Null
foreach ($generatedPath in @($stagingRoot, $zipPath, $hashPath)) {
    if (Test-Path -LiteralPath $generatedPath) {
        if ($isCustomOutputRoot) {
            throw "The fresh custom OutputRoot changed before packaging; refusing to remove existing content."
        }
        Remove-Item -LiteralPath $generatedPath -Recurse -Force
    }
}

Push-Location $repositoryRoot
try {
    Invoke-Checked -Program "npm.cmd" -Arguments @("run", "test:packaging")
    Invoke-Checked -Program "npm.cmd" -Arguments @("test")
    Invoke-Checked -Program "npm.cmd" -Arguments @("run", "build")

    Push-Location (Join-Path $repositoryRoot "src-tauri")
    try {
        Invoke-Checked -Program "cargo" -Arguments @("test", "--all-features")
        Invoke-Checked -Program "cargo" -Arguments @("clippy", "--all-targets", "--all-features", "--", "-D", "warnings")
    }
    finally {
        Pop-Location
    }

    $previousRustFlags = $env:RUSTFLAGS
    $portableRustFlags = "-C target-feature=+crt-static -C link-arg=/Brepro"
    try {
        $env:RUSTFLAGS = if ([string]::IsNullOrWhiteSpace($previousRustFlags)) {
            $portableRustFlags
        }
        else {
            "$previousRustFlags $portableRustFlags"
        }
        Invoke-Checked -Program "npm.cmd" -Arguments @("run", "tauri", "--", "build", "--no-bundle")
    }
    finally {
        if ($null -eq $previousRustFlags) {
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        }
        else {
            $env:RUSTFLAGS = $previousRustFlags
        }
    }

    Invoke-Checked -Program "node" -Arguments @("scripts/portable-package.mjs", "stage", "--app", $applicationPath, "--vendor", $resolvedVendorRoot, "--gallery-dl-source", $resolvedGalleryDlSourceArchive, "--output", $stagingRoot, "--tiktok-core", $TikTokCorePath)
    New-DeterministicZip -SourceDirectory $stagingRoot -DestinationZip $zipPath

    $auditRoot = Join-Path $resolvedOutputRoot ".audit-$([Guid]::NewGuid().ToString('N'))"
    try {
        [IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $auditRoot)
        $expandedRoot = Join-Path $auditRoot $releaseName
        Invoke-Checked -Program "node" -Arguments @("scripts/portable-package.mjs", "audit", "--portable", $expandedRoot, "--source", $repositoryRoot, "--source", $resolvedVendorRoot, "--source", (Split-Path -Parent $resolvedGalleryDlSourceArchive), "--source", $resolvedOutputRoot)
    }
    finally {
        if (Test-Path -LiteralPath $auditRoot) {
            Remove-Item -LiteralPath $auditRoot -Recurse -Force
        }
    }

    $hashJson = & node scripts/portable-package.mjs hash --file $zipPath
    if ($LASTEXITCODE -ne 0) {
        throw "Portable archive SHA-256 calculation failed with exit code $LASTEXITCODE"
    }
    $sha256 = ($hashJson | ConvertFrom-Json).sha256
    if ($sha256 -notmatch '^[0-9a-f]{64}$') {
        throw "Portable archive SHA-256 calculation returned an invalid digest"
    }
    [IO.File]::WriteAllText($hashPath, "$sha256  $([IO.Path]::GetFileName($zipPath))`n", [Text.UTF8Encoding]::new($false))

    Write-Host "Portable release created: $zipPath"
    Write-Host "SHA-256 record: $hashPath"
}
finally {
    Pop-Location
}
