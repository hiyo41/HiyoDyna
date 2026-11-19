param(
    [string]$FromTag = "",
    [string]$ToTag = "HEAD",
    [string]$Output = "release-notes.md"
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$repo = if ($env:GITHUB_REPOSITORY) { $env:GITHUB_REPOSITORY } else { "hiyo41/HiyoDyna" }
$range = if ($FromTag) { "$FromTag..HEAD" } else { "HEAD" }

$raw = git log $range --no-merges --pretty=format:"%h%x09%s"
if ($LASTEXITCODE -ne 0) {
    throw "git log failed for range $range"
}

$commits = @()
foreach ($line in ($raw -split "`n")) {
    if (-not $line.Trim()) { continue }
    $parts = $line -split "`t", 2
    if ($parts.Count -ne 2) { continue }
    $commits += [pscustomobject]@{
        Sha = $parts[0]
        Subject = $parts[1]
    }
}

$groups = [ordered]@{
    "Features" = @()
    "Bug Fixes" = @()
    "Performance" = @()
    "Documentation" = @()
    "Build and CI" = @()
    "Other Changes" = @()
}

$categories = @{
    "feat" = "Features"
    "fix" = "Bug Fixes"
    "perf" = "Performance"
    "docs" = "Documentation"
    "build" = "Build and CI"
    "ci" = "Build and CI"
    "chore" = "Build and CI"
}

foreach ($commit in $commits) {
    $category = "Other Changes"
    $description = $commit.Subject
    if ($description -match '^(?<type>[a-zA-Z]+)(\([^)]*\))?!?:\s*(?<desc>.+)$') {
        $type = $Matches["type"].ToLowerInvariant()
        $description = $Matches["desc"]
        if ($categories.ContainsKey($type)) {
            $category = $categories[$type]
        }
    }
    $groups[$category] += "- $description ([$($commit.Sha)](https://github.com/$repo/commit/$($commit.Sha)))"
}

$lines = @("# What's Changed", "")
if ($commits.Count -eq 0) {
    $lines += "No changes since the previous release."
} else {
    foreach ($name in $groups.Keys) {
        if ($groups[$name].Count -gt 0) {
            $lines += "## $name"
            $lines += $groups[$name]
            $lines += ""
        }
    }
    if ($FromTag) {
        $lines += "**Full Changelog**: https://github.com/$repo/compare/$FromTag...$ToTag"
    }
}

$path = [System.IO.Path]::GetFullPath($Output)
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllLines($path, $lines, $utf8)
