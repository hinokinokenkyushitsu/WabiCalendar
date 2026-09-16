# Install WabiCalendar and its `wabi` command line tool.
#
#   irm https://raw.githubusercontent.com/hinokinokenkyushitsu/WabiCalendar/main/install.ps1 | iex
#
# Environment:
#   WABICALENDAR_VERSION   a tag such as v0.1.0, instead of the latest release
#   WABICALENDAR_BIN_DIR   where wabi.exe goes, instead of %LOCALAPPDATA%\WabiCalendar\bin
#
# Everything is a function until the very last line, for the same reason the
# shell script does it: a download cut off halfway through should define some
# functions and run none of them.

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 may still default to TLS 1.0, which GitHub refuses.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
# Invoke-WebRequest spends most of its time drawing this.
$ProgressPreference = 'SilentlyContinue'

$Repo = 'hinokinokenkyushitsu/WabiCalendar'

function Get-ReleaseAssets {
    $version = $env:WABICALENDAR_VERSION
    $url = if ($version) {
        "https://api.github.com/repos/$Repo/releases/tags/$version"
    } else {
        "https://api.github.com/repos/$Repo/releases/latest"
    }

    try {
        $release = Invoke-RestMethod -Uri $url -Headers @{ 'User-Agent' = 'wabicalendar-install' }
    } catch {
        throw "No release to install from at $url"
    }

    # Checksums are found from their subject's name, never matched as assets in
    # their own right -- wabi-windows-x86_64.exe.sha256 matches every pattern
    # wabi-windows-x86_64.exe does.
    $release.assets | Where-Object { $_.name -notlike '*.sha256' -and $_.name -notlike '*.sig' }
}

function Get-Verified {
    param($Asset, $Directory)

    $file = Join-Path $Directory $Asset.name
    Invoke-WebRequest -Uri $Asset.browser_download_url -OutFile $file

    $sums = "$($Asset.browser_download_url).sha256"
    try {
        $published = (Invoke-WebRequest -Uri $sums -UseBasicParsing).Content
    } catch {
        throw "$($Asset.name) has no published checksum; refusing to install it"
    }

    # The file records "<hash>  <name>"; only the hash matters here.
    $expected = ($published -split '\s+')[0]
    $actual = (Get-FileHash -Path $file -Algorithm SHA256).Hash

    if ($actual -ne $expected) {
        throw "$($Asset.name) does not match its published checksum"
    }
    Write-Host "  verified $($Asset.name)"
    $file
}

function Install-App {
    param($Assets, $Directory)

    $asset = $Assets | Where-Object { $_.name -like '*-setup.exe' } | Select-Object -First 1
    if (-not $asset) { throw 'This release has no Windows installer' }

    $installer = Get-Verified -Asset $asset -Directory $Directory

    # /S is NSIS's silent switch. The bundle installs for the current user, so
    # nothing here asks to be elevated.
    $process = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "The installer exited with $($process.ExitCode)"
    }
    Write-Host '  WabiCalendar installed'
}

function Install-Cli {
    param($Assets, $Directory, $BinDir)

    $asset = $Assets | Where-Object { $_.name -like 'wabi-windows-*' } | Select-Object -First 1
    if (-not $asset) { throw 'This release has no wabi build for Windows' }

    $file = Get-Verified -Asset $asset -Directory $Directory

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item -Path $file -Destination (Join-Path $BinDir 'wabi.exe') -Force
    Write-Host "  wabi.exe -> $BinDir"
}

function Add-ToPath {
    param($BinDir)

    # The user's own Path, not the process's: this has to outlive the window it
    # ran in.
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = if ($current) { $current -split ';' } else { @() }

    if ($entries -contains $BinDir) {
        return
    }

    $updated = if ($current) { "$current;$BinDir" } else { $BinDir }
    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
    Write-Host ''
    Write-Host "$BinDir was added to your PATH. Open a new terminal to reach wabi."
}

function Install-WabiCalendar {
    $binDir = if ($env:WABICALENDAR_BIN_DIR) {
        $env:WABICALENDAR_BIN_DIR
    } else {
        Join-Path $env:LOCALAPPDATA 'WabiCalendar\bin'
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $work | Out-Null

    try {
        $assets = Get-ReleaseAssets
        if (-not $assets) { throw 'That release has no downloadable files' }

        Write-Host 'Installing WabiCalendar for windows...'
        Install-App -Assets $assets -Directory $work
        Install-Cli -Assets $assets -Directory $work -BinDir $binDir
        Add-ToPath -BinDir $binDir

        Write-Host ''
        Write-Host 'Done.'
    } finally {
        Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
    }
}

Install-WabiCalendar
