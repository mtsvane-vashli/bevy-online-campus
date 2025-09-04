param(
  [string]$Server = "127.0.0.1:5000",
  [switch]$LowGfx,
  [switch]$NoVsync,
  [int]$ClientPort = 0,
  [string]$LogLevel = "warn"
)

$ErrorActionPreference = "Stop"

$env:SERVER_ADDR = $Server
if ($LowGfx) { $env:LOW_GFX = "1" } else { Remove-Item Env:LOW_GFX -ErrorAction SilentlyContinue }
if ($NoVsync) { $env:NO_VSYNC = "1" } else { Remove-Item Env:NO_VSYNC -ErrorAction SilentlyContinue }
if ($ClientPort -gt 0) { $env:CLIENT_PORT = "$ClientPort" } else { Remove-Item Env:CLIENT_PORT -ErrorAction SilentlyContinue }
$env:RUST_LOG = $LogLevel

Write-Host "SERVER_ADDR=$($env:SERVER_ADDR)  LOW_GFX=$($env:LOW_GFX)  NO_VSYNC=$($env:NO_VSYNC)  CLIENT_PORT=$($env:CLIENT_PORT)  RUST_LOG=$($env:RUST_LOG)" -ForegroundColor Cyan

$exePaths = @(
  Join-Path $PSScriptRoot "bevy-online-campus.exe",
  Join-Path $PSScriptRoot "target\release\bevy-online-campus.exe"
)

$exe = $exePaths | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($exe) {
  Write-Host "Running executable: $exe" -ForegroundColor Green
  & $exe
} else {
  Write-Host "Executable not found. Falling back to cargo run --release" -ForegroundColor Yellow
  cargo run --release
}

