$ErrorActionPreference = "Stop"

cargo fmt --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo build --locked
exit $LASTEXITCODE
