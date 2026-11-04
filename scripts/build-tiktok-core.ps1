[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Python,
    [string]$OutputRoot,
    [switch]$SkipOnedir
)
$ErrorActionPreference='Stop'
$lmdRepository=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (!$OutputRoot) { $OutputRoot=Join-Path $lmdRepository '.release\tiktok-build' }
$OutputRoot=[IO.Path]::GetFullPath($OutputRoot)
if (!$OutputRoot.StartsWith($lmdRepository+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Build output must remain within repository' }
$lmdSource=Join-Path $lmdRepository 'tools\tiktok-core'
function Invoke-LmdChecked([string[]]$Arguments) {
    & $Python @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'TikTok core build command failed' }
}
Invoke-LmdChecked @('-c', 'import sys,curl_cffi,PyInstaller; assert sys.version_info[:3]==(3,12,14); assert curl_cffi.__version__=="0.17.0"; assert PyInstaller.__version__=="6.22.3"')
Invoke-LmdChecked @('-m','unittest','discover','-s',(Join-Path $lmdSource 'tests'))
$lmdName='tiktok-core-x86_64-pc-windows-msvc'
$lmdCommon=@('-m','PyInstaller','--noconfirm','--clean','--console','--name',$lmdName,'--paths',$lmdSource,'--collect-all','curl_cffi','--collect-data','certifi','--specpath',(Join-Path $OutputRoot 'spec'))
if (!$SkipOnedir) {
    $lmdOnedir=Join-Path $OutputRoot 'onedir'
    Invoke-LmdChecked ($lmdCommon+@('--onedir','--distpath',$lmdOnedir,'--workpath',(Join-Path $OutputRoot 'work-onedir'),(Join-Path $lmdSource 'main.py')))
    $lmdIdentity=& (Join-Path $lmdOnedir ($lmdName+'\'+$lmdName+'.exe')) --version
    if ($LASTEXITCODE -ne 0 -or $lmdIdentity -ne 'chendownloader-tiktok-core 1.1.0 protocol=1') { throw 'Onedir component identity failed' }
}
$lmdVendor=Join-Path $OutputRoot 'vendor'
Invoke-LmdChecked ($lmdCommon+@('--onefile','--distpath',$lmdVendor,'--workpath',(Join-Path $OutputRoot 'work-onefile'),(Join-Path $lmdSource 'main.py')))
$lmdExecutable=Join-Path $lmdVendor ($lmdName+'.exe')
$lmdIdentity=& $lmdExecutable --version
if ($LASTEXITCODE -ne 0 -or $lmdIdentity -ne 'chendownloader-tiktok-core 1.1.0 protocol=1') { throw 'Onefile component identity failed' }
$lmdManifest=[ordered]@{schemaVersion=1;protocolVersion=1;target='x86_64-pc-windows-msvc';version='1.1.0';artifact=($lmdName+'.exe');sha256=(Get-FileHash -LiteralPath $lmdExecutable -Algorithm SHA256).Hash.ToLowerInvariant()}
$lmdManifestRoot=Join-Path $lmdRepository 'src-tauri\resources\tiktok-core'
[IO.Directory]::CreateDirectory($lmdManifestRoot) | Out-Null
[IO.File]::WriteAllText((Join-Path $lmdManifestRoot 'manifest.json'),($lmdManifest | ConvertTo-Json)+"`n",[Text.UTF8Encoding]::new($false))
$lmdManifest | ConvertTo-Json -Compress
