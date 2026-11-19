$ErrorActionPreference = "Stop"

Get-ChildItem -LiteralPath $PSScriptRoot -Filter "*.ps1" | ForEach-Object {
    [void][scriptblock]::Create((Get-Content -LiteralPath $_.FullName -Raw))
}

cargo fmt --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo test
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo build --locked
exit $LASTEXITCODE
