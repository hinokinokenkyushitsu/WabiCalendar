# Remove WabiCalendar, the `wabi` command, and everything either of them left on
# this machine.
#
#   irm https://raw.githubusercontent.com/hinokinokenkyushitsu/WabiCalendar/main/uninstall.ps1 | iex
#
# Environment (settings rather than parameters, because `| iex` has no way to
# pass parameters through):
#   WABICALENDAR_PURGE = 1     also delete the vault -- your calendar files and
#                           pomodoro records. Asked for separately, and never
#                           without printing which directory is about to go.
#   WABICALENDAR_YES = 1       do not ask
#   WABICALENDAR_DRY_RUN = 1   list what would go and remove nothing
#   WABICALENDAR_BIN_DIR       where wabi.exe was put, instead of the default
#
# Everything is a function until the very last line, for the same reason the
# shell script does it: a download cut off halfway through should leave a pile
# of functions that never ran, not half an uninstall.

$ErrorActionPreference = 'Stop'

$Ident = 'com.hinoki.wabicalendar'
$Product = 'WabiCalendar'

function Test-Flag {
    param($Name)
    $value = [Environment]::GetEnvironmentVariable($Name)
    $value -and $value -ne '0' -and $value -ne 'false'
}

function Get-ConfigDir {
    # Mirrors dirs::config_dir(), which on Windows is Roaming AppData.
    Join-Path $env:APPDATA $Ident
}

function Get-VaultPath {
    $conf = Join-Path (Get-ConfigDir) 'settings.toml'
    if (-not (Test-Path -LiteralPath $conf)) { return $null }
    foreach ($line in Get-Content -LiteralPath $conf) {
        if ($line -match '^\s*vault_path\s*=\s*"(.*)"\s*$') { return $matches[1] }
    }
    $null
}

# The installer entry is looked up rather than guessed: it carries both the
# uninstaller's path and the directory it was installed into, and NSIS needs the
# second one to be told to finish before returning.
function Get-InstallEntry {
    $roots = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
    )
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        foreach ($key in Get-ChildItem $root) {
            $entry = Get-ItemProperty $key.PSPath
            if ($entry.DisplayName -like "$Product*") { return $entry }
        }
    }
    $null
}

# The tray process rewrites timer.json every ten seconds, so anything deleted
# while it is alive comes straight back. The executable is named after the Cargo
# binary, not the product, so both spellings are asked for; Windows process
# names are case-insensitive, which is the only reason one list covers it.
function Stop-App {
    $running = Get-Process -Name 'wabicalendar', $Product -ErrorAction SilentlyContinue
    if (-not $running) { return }

    Write-Host '  asking WabiCalendar to quit'
    foreach ($p in $running) { $null = $p.CloseMainWindow() }
    Start-Sleep -Seconds 2

    $running = Get-Process -Name 'wabicalendar', $Product -ErrorAction SilentlyContinue
    if ($running) {
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
    }
}

function Uninstall-Bundle {
    param($Entry, $DryRun)

    $command = $Entry.QuietUninstallString
    if (-not $command) { $command = $Entry.UninstallString }
    if (-not $command) { return }

    if ($command -match '^\s*"([^"]+)"\s*(.*)$') {
        $exe = $matches[1]; $rest = $matches[2]
    } else {
        $exe = $command.Trim(); $rest = ''
    }

    if ($DryRun) {
        Write-Host "  would run $exe /S"
        return
    }
    if (-not (Test-Path -LiteralPath $exe)) {
        Write-Host "  the recorded uninstaller is gone: $exe"
        return
    }

    $arguments = @()
    if ($rest) { $arguments += $rest }
    if ($arguments -notcontains '/S') { $arguments += '/S' }
    # Without _?= an NSIS uninstaller copies itself to the temp directory and
    # returns at once, so -Wait would wait on nothing and the files below would
    # be counted before they were gone.
    if ($Entry.InstallLocation) { $arguments += "_?=$($Entry.InstallLocation.TrimEnd('\'))" }

    $process = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "The uninstaller exited with $($process.ExitCode)"
    }
    Write-Host '  removed WabiCalendar'

    # _?= is what leaves this behind, so it is ours to clear up.
    if ($Entry.InstallLocation -and (Test-Path -LiteralPath $Entry.InstallLocation)) {
        Remove-Item -LiteralPath $Entry.InstallLocation -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Remove-Autostart {
    param($DryRun)
    $keys = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run',
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
    )
    foreach ($key in $keys) {
        if (-not (Test-Path $key)) { continue }
        $property = Get-ItemProperty -Path $key -Name $Product -ErrorAction SilentlyContinue
        if (-not $property) { continue }
        if ($DryRun) {
            Write-Host "  would remove $key\$Product"
        } else {
            Remove-ItemProperty -Path $key -Name $Product -ErrorAction SilentlyContinue
            Write-Host "  removed $key\$Product"
        }
    }
}

# Unlike the shell script, this does edit something it was not asked about --
# but only because install.ps1 wrote it in the first place. The .sh installer
# never touched a shell profile, so uninstall.sh never touches one either.
function Remove-FromPath {
    param($BinDir, $DryRun)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $current) { return }

    $target = $BinDir.TrimEnd('\')
    $kept = @($current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $target })
    $updated = $kept -join ';'
    if ($updated -eq $current) { return }

    if ($DryRun) {
        Write-Host "  would take $BinDir off your PATH"
        return
    }
    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
    Write-Host "  took $BinDir off your PATH"
}

function Remove-Paths {
    param($Paths, $DryRun)
    foreach ($path in $Paths) {
        if (-not $path) { continue }
        if (-not (Test-Path -LiteralPath $path)) { continue }
        if ($DryRun) {
            Write-Host "  would remove $path"
        } else {
            Remove-Item -LiteralPath $path -Recurse -Force
            Write-Host "  removed $path"
        }
    }
}

function Confirm-Action {
    param($Question)
    if (Test-Flag 'WABICALENDAR_YES') { return $true }
    try {
        $reply = Read-Host "$Question [y/N]"
    } catch {
        throw 'nothing to ask at; set WABICALENDAR_YES=1 if this is what you meant'
    }
    $reply -in @('y', 'Y', 'yes', 'YES')
}

function Remove-Vault {
    param($Vault, $DryRun)

    if (-not $Vault) {
        Write-Host ''
        Write-Host 'No vault path was recorded, so there is nothing to purge.'
        return
    }
    if (-not (Test-Path -LiteralPath $Vault)) {
        Write-Host ''
        Write-Host "WABICALENDAR_PURGE: $Vault is not there."
        return
    }

    Write-Host ''
    Write-Host 'WABICALENDAR_PURGE will delete your calendar files and pomodoro records:'
    Write-Host ''
    Write-Host "  $Vault"
    Write-Host ''

    if ($DryRun) {
        Write-Host "  would remove $Vault"
        return
    }
    # Its own question, after its own path. Agreeing to uninstall a program is
    # not agreeing to throw away the documents it was used to write.
    if (-not (Confirm-Action 'Delete it?')) {
        Write-Host 'The vault was left alone.'
        return
    }
    Remove-Item -LiteralPath $Vault -Recurse -Force
    Write-Host "  removed $Vault"
}

function Uninstall-WabiCalendar {
    $dryRun = Test-Flag 'WABICALENDAR_DRY_RUN'
    $purge = Test-Flag 'WABICALENDAR_PURGE'

    $binDir = $env:WABICALENDAR_BIN_DIR
    if (-not $binDir) { $binDir = Join-Path $env:LOCALAPPDATA 'WabiCalendar\bin' }

    # Read before anything is deleted: afterwards nothing is left that knows
    # where the vault was.
    $vault = Get-VaultPath
    $entry = Get-InstallEntry

    $paths = @(
        (Join-Path $binDir 'wabi.exe'),
        (Get-ConfigDir),
        # WebView2's own storage, which the app never writes to directly.
        (Join-Path $env:LOCALAPPDATA $Ident)
    )
    $present = @($paths | Where-Object { Test-Path -LiteralPath $_ })

    if (-not $entry -and -not $present) {
        Write-Host 'WabiCalendar is not installed here; nothing to remove.'
        if ($purge) { Remove-Vault -Vault $vault -DryRun $dryRun }
        return
    }

    Write-Host $(if ($dryRun) { 'These would be removed:' } else { 'These will be removed:' })
    if ($entry) { Write-Host "  $($entry.DisplayName) $($entry.DisplayVersion)" }
    foreach ($path in $present) { Write-Host "  $path" }

    if (-not $dryRun) {
        if (-not (Confirm-Action 'Remove them?')) {
            throw 'nothing was removed'
        }
        Stop-App
    }

    if ($entry) { Uninstall-Bundle -Entry $entry -DryRun $dryRun }
    Remove-Autostart -DryRun $dryRun
    Remove-Paths -Paths $present -DryRun $dryRun
    Remove-FromPath -BinDir $binDir -DryRun $dryRun

    # The bin directory is ours only if we made it and nothing else is in it.
    if (-not $dryRun -and (Test-Path -LiteralPath $binDir)) {
        if (-not (Get-ChildItem -LiteralPath $binDir -Force)) {
            Remove-Item -LiteralPath $binDir -Force -ErrorAction SilentlyContinue
        }
    }

    if ($purge) {
        Remove-Vault -Vault $vault -DryRun $dryRun
    } elseif ($vault -and (Test-Path -LiteralPath $vault)) {
        Write-Host ''
        Write-Host 'Your calendar files and pomodoro records were left alone, in:'
        Write-Host ''
        Write-Host "  $vault"
        Write-Host ''
        Write-Host 'They are plain .ics and .jsonl files and are yours. To delete them too:'
        Write-Host ''
        Write-Host "  Remove-Item -Recurse -Force '$vault'"
    }

    Write-Host ''
    Write-Host 'Done.'
}

Uninstall-WabiCalendar
