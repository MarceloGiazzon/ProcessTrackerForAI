# Runs every check a change must pass, in the same order as CI, and stops at
# the first failure. Usage (from the repository root, in PowerShell):
#
#   ./scripts/verify.ps1
#
# It checks formatting, lints for Windows, Linux and macOS (cross-checking
# needs no Linux or Mac machine), and runs all tests on this machine.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Step($name, [scriptblock]$cmd) {
    Write-Host "==> $name" -ForegroundColor Cyan
    & $cmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $name" -ForegroundColor Red
        exit 1
    }
}

$targets = @("x86_64-unknown-linux-musl", "aarch64-apple-darwin")
$installed = rustup target list --installed
foreach ($t in $targets) {
    if (-not ($installed -contains $t)) {
        Step "add Rust target $t (one-time download)" { rustup target add $t }
    }
}

Step "format check (fix with: cargo fmt)" { cargo fmt --check }
Step "lint (this machine)" { cargo clippy --all-targets -- -D warnings }
foreach ($t in $targets) {
    Step "lint ($t)" { cargo clippy --all-targets --target $t -- -D warnings }
}
Step "tests" { cargo test }
Write-Host "ALL CHECKS PASSED" -ForegroundColor Green
