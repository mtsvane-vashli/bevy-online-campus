param(
  [string]$Address = "0.0.0.0",
  [int]$Port = 5000,
  [string]$LogLevel = "warn"
)

$ErrorActionPreference = "Stop"

$env:SERVER_ADDR = "$Address:$Port"
$env:RUST_LOG = $LogLevel

Write-Host "SERVER_ADDR=$($env:SERVER_ADDR)  RUST_LOG=$($env:RUST_LOG)" -ForegroundColor Cyan

$exePaths = @(
  Join-Path $PSScriptRoot "server.exe",
  Join-Path $PSScriptRoot "target\release\server.exe"
)

$exe = $exePaths | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($exe) {
  Write-Host "Running executable: $exe" -ForegroundColor Green
  & $exe
} else {
  Write-Host "Executable not found. Falling back to cargo run --release --bin server" -ForegroundColor Yellow
  cargo run --release --bin server
}

