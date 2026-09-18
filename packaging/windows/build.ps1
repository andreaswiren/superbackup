<#
.SYNOPSIS
    Build the superbackup MSI.

.DESCRIPTION
    Takes an already-built superbackup.exe and wraps it. It does not build the
    executable: the release workflow builds each target on its own runner and
    this runs afterwards, and a script that rebuilt would produce an installer
    around a different binary from the one that was tested and attested.

    WiX v4+ is a dotnet tool. The script installs it into a local tool manifest
    rather than globally, so a machine that already has another version keeps
    it and CI gets a pinned one.

.PARAMETER ExePath
    The superbackup.exe to package.

.PARAMETER Version
    Three numbers. MSI product versions are `major.minor.build` and ignore
    anything after — a prerelease suffix has to be stripped rather than passed,
    or the installer silently compares versions differently from the tag.

.PARAMETER OutFile
    Where to write the .msi.

.PARAMETER Arch
    The architecture the packaged binary is for. It has to match: an MSI built
    as x64 around an ARM64 binary installs and then will not run, and nothing
    in the installer notices.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$OutFile,
    [ValidateSet('x64', 'arm64')][string]$Arch = 'x64'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot | Split-Path -Parent

if (-not (Test-Path $ExePath)) { throw "no executable at $ExePath" }

# `0.11.1-rc.1` is not an MSI version. Strip the suffix and keep the numbers,
# so the installer's upgrade logic compares what it can actually compare.
$msiVersion = ($Version -replace '^v', '') -replace '[-+].*$', ''
if ($msiVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw "version '$Version' does not reduce to major.minor.patch (got '$msiVersion')"
}

$build = Join-Path $root 'target/msi'
New-Item -ItemType Directory -Force -Path $build | Out-Null

# WiX shows the licence in a dialog and wants RTF. Wrapping the plain text
# keeps one licence file in the repository rather than two that can disagree.
$licence = Get-Content (Join-Path $root 'LICENSE') -Raw
$escaped = $licence -replace '\\', '\\\\' -replace '([{}])', '\$1'
$rtfBody = ($escaped -split "`r?`n") -join '\par '
$rtf = '{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 ' + $rtfBody + '}'
$rtfPath = Join-Path $build 'LICENSE.rtf'
Set-Content -Path $rtfPath -Value $rtf -Encoding ASCII

Push-Location $root
try {
    # `--local` looks for `.config/dotnet-tools.json` and creates the manifest
    # beside the *current directory* when told to, which is why this pushes to
    # the repository root first: run from anywhere else, `dotnet new
    # tool-manifest` drops a stray manifest wherever it was invoked.
    if (-not (Test-Path (Join-Path $root '.config/dotnet-tools.json'))) {
        New-Item -ItemType Directory -Force -Path (Join-Path $root '.config') | Out-Null
        dotnet new tool-manifest --force | Out-Null
        if (Test-Path (Join-Path $root 'dotnet-tools.json')) {
            Move-Item (Join-Path $root 'dotnet-tools.json') `
                      (Join-Path $root '.config/dotnet-tools.json') -Force
        }
    }
    # Pinned. An installer built by whatever WiX happened to be newest is an
    # installer nobody can reproduce.
    # A half-finished restore leaves the package file locked, and every later
    # attempt then fails with "the process cannot access the file" rather than
    # with anything about what went wrong. Retried once with the cached package
    # cleared, which is the only thing that fixes it.
    dotnet tool install wix --version 5.0.2 --local 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Remove-Item (Join-Path $env:USERPROFILE '.nuget/packages/wix') -Recurse -Force -ErrorAction SilentlyContinue
        dotnet tool install wix --version 5.0.2 --local
        if ($LASTEXITCODE -ne 0) { throw 'could not install the WiX tool' }
    }
    dotnet tool run wix -- extension add --global WixToolset.UI.wixext/5.0.2 2>&1 | Out-Null
    dotnet tool run wix -- extension add --global WixToolset.Util.wixext/5.0.2 2>&1 | Out-Null

    dotnet tool run wix -- build `
        (Join-Path $PSScriptRoot 'superbackup.wxs') `
        -arch $Arch `
        -ext WixToolset.UI.wixext `
        -ext WixToolset.Util.wixext `
        -d "Version=$msiVersion" `
        -d "ExePath=$((Resolve-Path $ExePath).Path)" `
        -d "IconPath=$(Join-Path $root 'assets/icons/superbackup.ico')" `
        -d "LicensePath=$(Join-Path $root 'LICENSE')" `
        -d "LicenseRtfPath=$rtfPath" `
        -o $OutFile
}
finally {
    Pop-Location
}

if (-not (Test-Path $OutFile)) { throw "wix reported success but produced no $OutFile" }
Write-Output "built $OutFile"
