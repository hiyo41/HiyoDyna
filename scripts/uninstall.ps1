param(
    [switch]$KeepSettings
)

$ErrorActionPreference = "Stop"

$InstallDir = $PSScriptRoot
$DataDir = Join-Path $env:LOCALAPPDATA "HiyoDyna"
$SettingsDir = Join-Path $env:APPDATA "HiyoDyna"
$WatcherPath = Join-Path $InstallDir "watch-update.ps1"

function Stop-Watcher {
    $pidFile = Join-Path $DataDir "watch-update.pid"
    if (Test-Path -LiteralPath $pidFile) {
        $watcherPid = 0
        [void][int]::TryParse((Get-Content -LiteralPath $pidFile -Raw).Trim(), [ref]$watcherPid)
        if ($watcherPid -gt 0) {
            Stop-Process -Id $watcherPid -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $pidFile -Force -ErrorAction SilentlyContinue
    }
    Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            $_.CommandLine -and
            $_.CommandLine.IndexOf($WatcherPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
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

Stop-Watcher
Stop-Apps

$startupShortcut = Join-Path ([Environment]::GetFolderPath("Startup")) "HiyoDyna updater.lnk"
Remove-Item -LiteralPath $startupShortcut -Force -ErrorAction SilentlyContinue
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

if (-not $KeepSettings) {
    Remove-Item -LiteralPath $SettingsDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $DataDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "HiyoDyna was uninstalled."
if ($KeepSettings) {
    Write-Host "Settings and fonts were kept in $SettingsDir and $DataDir."
}
