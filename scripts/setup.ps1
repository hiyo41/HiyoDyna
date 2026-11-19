param(
    [string]$RepoPath = (Split-Path -Parent $PSScriptRoot),
    [switch]$NoAutostart
)

$ErrorActionPreference = "Stop"

$RepoPath = (Resolve-Path -LiteralPath $RepoPath).Path
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\HiyoDyna"
$DataDir = Join-Path $env:LOCALAPPDATA "HiyoDyna"
$ExePath = Join-Path $InstallDir "HiyoDyna.exe"
$BuiltExe = Join-Path $RepoPath "target\release\hiyodyna.exe"
$LegacyShortcut = Join-Path ([Environment]::GetFolderPath("Startup")) "HiyoDyna updater.lnk"
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
    Remove-Item -LiteralPath $LegacyShortcut -Force -ErrorAction SilentlyContinue
    foreach ($name in @("watch-update.ps1", "watch-update.vbs", "update-state.json")) {
        Remove-Item -LiteralPath (Join-Path $InstallDir $name) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath (Join-Path $DataDir "pause") -Force -ErrorAction SilentlyContinue
}

function Stop-Apps {
    $processes = @(Get-Process -Name "HiyoDyna", "hiyodyna" -ErrorAction SilentlyContinue)
    if ($processes.Count -eq 0) {
        return
    }
    foreach ($process in $processes) {
        $null = $process.CloseMainWindow()
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

function Get-CargoPath {
    $command = Get-Command cargo -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    $fallback = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path -LiteralPath $fallback) {
        return $fallback
    }
    throw "cargo was not found; install Rust or add cargo to PATH"
}

Write-Host "Preparing HiyoDyna from $RepoPath"
Stop-LegacyWatcher
Stop-Apps
Remove-LegacyWatcher

$cargo = Get-CargoPath
Push-Location -LiteralPath $RepoPath
try {
    & $cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
if (-not (Test-Path -LiteralPath $BuiltExe)) {
    throw "release executable was not produced at $BuiltExe"
}

$versionLine = Select-String -Path (Join-Path $RepoPath "Cargo.toml") `
    -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
$version = if ($versionLine) { $versionLine.Matches[0].Groups[1].Value } else { "0.0.0" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
foreach ($name in @("HiyoDyna.update.part", "HiyoDyna.update.exe", "HiyoDyna.update.txt", "HiyoDyna.exe.old")) {
    Remove-Item -LiteralPath (Join-Path $InstallDir $name) -Force -ErrorAction SilentlyContinue
}
Remove-Item -LiteralPath (Join-Path $DataDir "update-state.ini") -Force -ErrorAction SilentlyContinue
Copy-Item -LiteralPath $BuiltExe -Destination $ExePath -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "uninstall.ps1") `
    -Destination (Join-Path $InstallDir "uninstall.ps1") -Force
$version | Set-Content -LiteralPath (Join-Path $InstallDir "version.txt") -Encoding UTF8
[ordered]@{
    installDir = $InstallDir
    exePath = $ExePath
    version = $version
    installedAt = (Get-Date).ToString("o")
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $InstallDir "install.json") -Encoding UTF8

$shell = New-Object -ComObject WScript.Shell
$startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "HiyoDyna"
New-Item -ItemType Directory -Force -Path $startMenuDir | Out-Null
$shortcut = $shell.CreateShortcut((Join-Path $startMenuDir "HiyoDyna.lnk"))
$shortcut.TargetPath = $ExePath
$shortcut.WorkingDirectory = $InstallDir
$shortcut.Description = "HiyoDyna"
$shortcut.IconLocation = "$ExePath,0"
$shortcut.Save()

$desktopShortcutPath = Join-Path ([Environment]::GetFolderPath("Desktop")) "HiyoDyna.lnk"
$desktopShortcut = $shell.CreateShortcut($desktopShortcutPath)
$desktopShortcut.TargetPath = $ExePath
$desktopShortcut.WorkingDirectory = $InstallDir
$desktopShortcut.Description = "HiyoDyna"
$desktopShortcut.IconLocation = "$ExePath,0"
$desktopShortcut.Save()

$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
New-Item -Path $runKey -Force | Out-Null
if ($NoAutostart) {
    Remove-ItemProperty -Path $runKey -Name "HiyoDyna" -ErrorAction SilentlyContinue
} else {
    Set-ItemProperty -Path $runKey -Name "HiyoDyna" -Value "`"$ExePath`""
}

$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HiyoDyna"
New-Item -Path $uninstallKey -Force | Out-Null
Set-ItemProperty -Path $uninstallKey -Name "DisplayName" -Value "HiyoDyna"
Set-ItemProperty -Path $uninstallKey -Name "DisplayVersion" -Value $version
Set-ItemProperty -Path $uninstallKey -Name "DisplayIcon" -Value $ExePath
Set-ItemProperty -Path $uninstallKey -Name "InstallLocation" -Value $InstallDir
Set-ItemProperty -Path $uninstallKey -Name "Publisher" -Value "HiyoDyna"
Set-ItemProperty -Path $uninstallKey -Name "UninstallString" `
    -Value "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$InstallDir\uninstall.ps1`""

Start-Process -FilePath $ExePath -WorkingDirectory $InstallDir

Write-Host "HiyoDyna $version installed to $InstallDir"
Write-Host "Auto-update: in-app (startup and every 30 minutes)"
Write-Host "Start with Windows: $(if ($NoAutostart) { 'disabled' } else { 'enabled' })"
