param(
    [switch]$Once,
    [switch]$RecordOnly
)

$ErrorActionPreference = "Stop"

$InstallDir = $PSScriptRoot
$DataDir = Join-Path $env:LOCALAPPDATA "HiyoDyna"
$LogPath = Join-Path $DataDir "auto-update.log"
$PausePath = Join-Path $DataDir "pause"
$PidPath = Join-Path $DataDir "watch-update.pid"
$InstallPath = Join-Path $InstallDir "install.json"
$StatePath = Join-Path $InstallDir "update-state.json"

New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

function Write-Log {
    param([string]$Message)
    if (Test-Path -LiteralPath $LogPath) {
        $log = Get-Item -LiteralPath $LogPath
        if ($log.Length -gt 1MB) {
            $old = "$LogPath.old"
            Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
            Move-Item -LiteralPath $LogPath -Destination $old -Force -ErrorAction SilentlyContinue
        }
    }
    $line = "[{0}] {1}" -f (Get-Date -Format "s"), $Message
    Add-Content -LiteralPath $LogPath -Value $line -Encoding UTF8
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
    return $null
}

function Get-SourceFingerprint {
    param([string]$Repo)
    $files = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    foreach ($relative in @("src", "assets")) {
        $directory = Join-Path $Repo $relative
        if (Test-Path -LiteralPath $directory) {
            Get-ChildItem -LiteralPath $directory -Recurse -File | ForEach-Object { $files.Add($_) }
        }
    }
    foreach ($name in @("Cargo.toml", "Cargo.lock")) {
        $file = Join-Path $Repo $name
        if (Test-Path -LiteralPath $file) {
            $files.Add((Get-Item -LiteralPath $file))
        }
    }
    $lines = foreach ($file in ($files | Sort-Object FullName)) {
        $relative = $file.FullName.Substring($Repo.Length).TrimStart([char[]]@('\', '/')).Replace('\', '/')
        "{0}|{1}|{2}" -f $relative, $file.Length, $file.LastWriteTimeUtc.Ticks
    }
    $text = ($lines -join "`n")
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)
        ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Get-Hash {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return ""
    }
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-State {
    if (-not (Test-Path -LiteralPath $StatePath)) {
        return $null
    }
    try {
        return Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
    } catch {
        return $null
    }
}

function Save-State {
    param(
        [string]$Fingerprint,
        [string]$FailedFingerprint,
        [string]$InstalledHash,
        [string]$Result
    )
    $payload = [ordered]@{
        fingerprint = $Fingerprint
        failedFingerprint = $FailedFingerprint
        installedHash = $InstalledHash
        lastResult = $Result
        updatedAt = (Get-Date).ToString("o")
    }
    $payload | ConvertTo-Json | Set-Content -LiteralPath $StatePath -Encoding UTF8
}

function Invoke-Build {
    param([string]$Repo)
    $cargo = Get-CargoPath
    if (-not $cargo) {
        Write-Log "cargo was not found; install Rust or add cargo to PATH"
        return $false
    }
    Write-Log "building $Repo"
    $started = Get-Date
    $code = 1
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    Push-Location -LiteralPath $Repo
    try {
        & $cargo build --release --locked 2>&1 | ForEach-Object { Write-Log ([string]$_) }
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        Pop-Location
    }
    $seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    if ($code -eq 0) {
        Write-Log "build succeeded in ${seconds}s"
        return $true
    }
    Write-Log "build failed with exit code $code"
    return $false
}

function Stop-InstalledApp {
    $processes = @(Get-Process -Name "HiyoDyna" -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -ceq "HiyoDyna" })
    if ($processes.Count -eq 0) {
        return
    }
    Write-Log "closing the running app"
    foreach ($process in $processes) {
        $null = $process.CloseMainWindow()
    }
    $deadline = (Get-Date).AddSeconds(3)
    while ((Get-Date) -lt $deadline) {
        $remaining = @(Get-Process -Name "HiyoDyna" -ErrorAction SilentlyContinue |
            Where-Object { $_.ProcessName -ceq "HiyoDyna" })
        if ($remaining.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 200
    }
    Get-Process -Name "HiyoDyna" -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -ceq "HiyoDyna" } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

function Copy-Binary {
    param([string]$Source, [string]$Destination)
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force
            return $true
        } catch {
            Write-Log "copy attempt $attempt failed: $($_.Exception.Message)"
            Start-Sleep -Milliseconds 300
        }
    }
    return $false
}

function Start-InstalledApp {
    param([string]$Exe)
    if (Test-Path -LiteralPath $Exe) {
        Start-Process -FilePath $Exe -WorkingDirectory (Split-Path -Parent $Exe)
        Write-Log "restarted the app"
    }
}

function Invoke-UpdateCheck {
    param(
        [string]$Repo,
        [string]$Exe,
        [string]$BuiltExe
    )
    $fingerprint = Get-SourceFingerprint $Repo
    $state = Read-State
    $installedHash = Get-Hash $Exe

    if ($RecordOnly) {
        Save-State -Fingerprint $fingerprint -FailedFingerprint "" -InstalledHash $installedHash -Result "recorded"
        Write-Log "recorded baseline $($fingerprint.Substring(0, 12))"
        return
    }

    $previousFingerprint = [string]$state.fingerprint
    $failedFingerprint = [string]$state.failedFingerprint
    $previousHash = [string]$state.installedHash

    $sourceChanged = $fingerprint -ne $previousFingerprint
    $needsRepair = ($fingerprint -eq $previousFingerprint) -and
        ((-not (Test-Path -LiteralPath $Exe)) -or ($installedHash -ne $previousHash))

    if ($fingerprint -eq $failedFingerprint -and ($sourceChanged -or $needsRepair)) {
        Write-Log "source matches a failed build; waiting for the next edit"
        return
    }
    if (-not $sourceChanged -and -not $needsRepair) {
        return
    }

    if ($sourceChanged) {
        Start-Sleep -Seconds 2
        $stable = Get-SourceFingerprint $Repo
        if ($stable -ne $fingerprint) {
            return
        }
    }

    if (-not (Invoke-Build $Repo)) {
        Save-State -Fingerprint $previousFingerprint -FailedFingerprint $fingerprint `
            -InstalledHash $previousHash -Result "build failed"
        return
    }

    $builtHash = Get-Hash $BuiltExe
    if (-not $builtHash) {
        Save-State -Fingerprint $previousFingerprint -FailedFingerprint $fingerprint `
            -InstalledHash $previousHash -Result "missing build output"
        Write-Log "build output missing: $BuiltExe"
        return
    }

    if ($builtHash -ne $installedHash) {
        Stop-InstalledApp
        if (-not (Copy-Binary $BuiltExe $Exe)) {
            Save-State -Fingerprint $previousFingerprint -FailedFingerprint $fingerprint `
                -InstalledHash $previousHash -Result "copy failed"
            return
        }
        $installedHash = Get-Hash $Exe
        Start-InstalledApp $Exe
    } else {
        Write-Log "binary unchanged; no restart needed"
    }

    Save-State -Fingerprint $fingerprint -FailedFingerprint "" -InstalledHash $installedHash -Result "success"
    Write-Log "installed $($fingerprint.Substring(0, 12))"
}

try {
    if (-not (Test-Path -LiteralPath $InstallPath)) {
        Write-Log "install.json is missing at $InstallPath"
        return
    }
    $install = Get-Content -LiteralPath $InstallPath -Raw | ConvertFrom-Json
    $repo = [string]$install.repoPath
    $exe = [string]$install.exePath
    if (-not $exe) {
        $exe = Join-Path $InstallDir "HiyoDyna.exe"
    }
    if (-not (Test-Path -LiteralPath $repo)) {
        Write-Log "repository is missing at $repo"
        return
    }
    $built = Join-Path $repo "target\release\hiyodyna.exe"

    $created = $false
    $mutex = New-Object System.Threading.Mutex($true, "Local\HiyoDynaAutoUpdate", [ref]$created)
    if (-not $created) {
        return
    }
    try {
        if (-not $Once -and -not $RecordOnly) {
            Set-Content -LiteralPath $PidPath -Value $PID -Encoding ASCII
            Write-Log "watcher started for $repo"
        }
        if ($RecordOnly) {
            Invoke-UpdateCheck -Repo $repo -Exe $exe -BuiltExe $built
            return
        }

        $paused = $false
        while ($true) {
            if (Test-Path -LiteralPath $PausePath) {
                if (-not $paused) {
                    Write-Log "paused"
                    $paused = $true
                }
                if ($Once) {
                    break
                }
                Start-Sleep -Seconds 2
                continue
            }
            if ($paused) {
                Write-Log "resumed"
                $paused = $false
            }
            Invoke-UpdateCheck -Repo $repo -Exe $exe -BuiltExe $built
            if ($Once) {
                break
            }
            Start-Sleep -Seconds 2
        }
    } finally {
        if ($created) {
            $mutex.ReleaseMutex()
        }
        $mutex.Dispose()
        if (-not $Once -and -not $RecordOnly) {
            Remove-Item -LiteralPath $PidPath -Force -ErrorAction SilentlyContinue
        }
    }
} catch {
    Write-Log "watcher stopped: $($_.Exception.Message)"
    throw
}
