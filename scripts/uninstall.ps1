param(
    [switch]$KeepSettings
)

$ErrorActionPreference = "Stop"

$InstallDir = $PSScriptRoot
$DataDir = Join-Path $env:LOCALAPPDATA "HiyoDyna"
$SettingsDir = Join-Path $env:APPDATA "HiyoDyna"
$StartupDir = [Environment]::GetFolderPath("Startup")
$LegacyShortcut = if ([string]::IsNullOrWhiteSpace($StartupDir)) { $null } else { Join-Path $StartupDir "HiyoDyna updater.lnk" }
$LegacyPid = Join-Path $DataDir "watch-update.pid"
$LegacyWatcher = Join-Path $InstallDir "watch-update.ps1"

function Stop-LegacyWatcher {
    if (Test-Path -LiteralPath $LegacyPid) {
        $watcherPid = 0
        $rawPid = Get-Content -LiteralPath $LegacyPid -Raw -ErrorAction SilentlyContinue
        if (-not [string]::IsNullOrWhiteSpace($rawPid)) {
            [void][int]::TryParse($rawPid.Trim(), [ref]$watcherPid)
        }
        if ($watcherPid -gt 0) {
            $process = Get-CimInstance Win32_Process -Filter "ProcessId = $watcherPid" -ErrorAction SilentlyContinue
            if ($process -and $process.CommandLine -and
                $process.CommandLine.IndexOf("watch-update.ps1", [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                Stop-Process -Id $watcherPid -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath $LegacyPid -Force -ErrorAction SilentlyContinue
    }
    Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            $_.CommandLine -and
            $_.CommandLine.IndexOf($LegacyWatcher, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Remove-LegacyWatcher {
    if ($LegacyShortcut) {
        Remove-Item -LiteralPath $LegacyShortcut -Force -ErrorAction SilentlyContinue
    }
    foreach ($name in @("watch-update.ps1", "watch-update.vbs", "update-state.json")) {
        Remove-Item -LiteralPath (Join-Path $InstallDir $name) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath (Join-Path $DataDir "pause") -Force -ErrorAction SilentlyContinue
}

function Stop-Apps {
    $processes = @(Get-Process -Name "HiyoDyna", "hiyodyna" -ErrorAction SilentlyContinue)
    foreach ($process in $processes) {
        $null = $process.CloseMainWindow()
    }
    if ($processes.Count -eq 0) {
        return
    }
    $deadline = (Get-Date).AddSeconds(3)
    while ((Get-Date) -lt $deadline) {
        $remaining = @(Get-Process -Name "HiyoDyna", "hiyodyna" -ErrorAction SilentlyContinue)
        if ($remaining.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 200
    }
    Get-Process -Name "HiyoDyna", "hiyodyna" -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

Stop-LegacyWatcher
Stop-Apps
Remove-LegacyWatcher

$desktopShortcut = Join-Path ([Environment]::GetFolderPath("Desktop")) "HiyoDyna.lnk"
Remove-Item -LiteralPath $desktopShortcut -Force -ErrorAction SilentlyContinue
$startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "HiyoDyna"
Remove-Item -LiteralPath $startMenuDir -Recurse -Force -ErrorAction SilentlyContinue

Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" `
    -Name "HiyoDyna" -ErrorAction SilentlyContinue
Remove-Item -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HiyoDyna" `
    -Recurse -Force -ErrorAction SilentlyContinue

Set-Location $env:TEMP
for ($attempt = 1; $attempt -le 10; $attempt++) {
    try {
        Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction Stop
        break
    } catch {
        Start-Sleep -Milliseconds 300
    }
}

if (Test-Path -LiteralPath $InstallDir) {
    Write-Host "HiyoDyna could not remove $InstallDir"
    exit 1
}

Remove-Item -LiteralPath (Join-Path $DataDir "update-state.ini") -Force -ErrorAction SilentlyContinue
if (-not $KeepSettings) {
    Remove-Item -LiteralPath $SettingsDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $DataDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "HiyoDyna was uninstalled."
if ($KeepSettings) {
    Write-Host "Settings and fonts were kept in $SettingsDir and $DataDir."
}
