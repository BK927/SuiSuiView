# Build only the reviewed default feature set. PowerShell 7, Rust MSVC,
# Python 3.12+, cargo-about 0.9.2, Pillow 11.3.0 and VS C++ tools are required.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Windows x64 is required.' }
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
$previousFlags = $env:RUSTFLAGS
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$previousTargetDir = $env:CARGO_TARGET_DIR
try {
    # A target-specific build keeps proc macros on the host while cc-rs sees
    # crt-static for the bundled C libraries. Do not use target-cpu=native.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    $env:CARGO_ENCODED_RUSTFLAGS = $null
    $env:CARGO_TARGET_DIR = Join-Path $repo 'target'
    $target = 'x86_64-pc-windows-msvc'
    New-Item -ItemType Directory -Force target/release-validation | Out-Null
    cargo build --release --locked --target $target --bin suisuiview --bin suisuiview-cli
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    cargo test --release --locked --target $target --lib --bin suisuiview --bin suisuiview-cli 2>&1 |
        Tee-Object target/release-validation/cargo-test.txt
    if ($LASTEXITCODE -ne 0) { throw 'Regression tests failed.' }
    cargo about generate --locked --target $target --fail --format json -o target/release-validation/licenses.json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency license review failed.' }
    python scripts/release_smoke.py --cli "target/$target/release/suisuiview-cli.exe" --output target/release-validation/decoder-smoke
    if ($LASTEXITCODE -ne 0) { throw 'Decoder/archive smoke tests failed.' }
    python scripts/package_windows.py
    if ($LASTEXITCODE -ne 0) { throw 'Packaging failed.' }
} finally {
    $env:RUSTFLAGS = $previousFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags
    $env:CARGO_TARGET_DIR = $previousTargetDir
    Pop-Location
}
