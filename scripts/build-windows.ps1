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
    $compiler = rustc --version
    if ($LASTEXITCODE -ne 0 -or $compiler -notmatch '^rustc 1\.99\.1 ') {
        throw 'Select the release toolchain first: rustup override set 1.99.1'
    }
    $commit = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify source commit.' }
    $dirty = git status --porcelain --untracked-files=normal
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Commit public source changes before building.' }
    # A target-specific build keeps proc macros on the host while cc-rs sees
    # crt-static for the bundled C libraries. Do not use target-cpu=native.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    $env:CARGO_ENCODED_RUSTFLAGS = $null
    $env:CARGO_TARGET_DIR = Join-Path $repo 'target'
    $target = 'x86_64-pc-windows-msvc'
    New-Item -ItemType Directory -Force target/release-validation | Out-Null
    $receiptPath = Join-Path $repo 'target/release-validation/build-receipt.json'
    if (Test-Path -LiteralPath $receiptPath) { Remove-Item -LiteralPath $receiptPath }
    cargo build --release --locked --target $target --bin suisuiview --bin suisuiview-cli --message-format=json-render-diagnostics > target/release-validation/build-artifacts.jsonl
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    cargo test --release --locked --target $target --lib --bin suisuiview --bin suisuiview-cli 2>&1 |
        Tee-Object target/release-validation/cargo-test.txt
    if ($LASTEXITCODE -ne 0) { throw 'Regression tests failed.' }
    cargo about generate --locked --target $target --fail --format json -o target/release-validation/licenses.json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency license review failed.' }
    python scripts/release_smoke.py --cli "target/$target/release/suisuiview-cli.exe" --output target/release-validation/decoder-smoke
    if ($LASTEXITCODE -ne 0) { throw 'Decoder/archive smoke tests failed.' }
    $endingCommit = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $endingCommit -ne $commit) { throw 'Source commit changed during validation.' }
    $dirty = git status --porcelain --untracked-files=normal
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Source files changed during validation.' }
    $receiptFiles = @(
        "target/$target/release/suisuiview.exe",
        "target/$target/release/suisuiview-cli.exe",
        'target/release-validation/cargo-test.txt',
        'target/release-validation/build-artifacts.jsonl',
        'target/release-validation/licenses.json',
        'target/release-validation/decoder-smoke/summary.json'
    )
    $hashes = @{}
    foreach ($file in $receiptFiles) {
        $hashes[$file] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    @{ commit = $commit; files = $hashes; regression_tests = 'passed';
       ci_run = $env:GITHUB_RUN_ID; created_utc = [DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $receiptPath -Encoding utf8
    python scripts/package_windows.py
    if ($LASTEXITCODE -ne 0) { throw 'Packaging failed.' }
} finally {
    $env:RUSTFLAGS = $previousFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags
    $env:CARGO_TARGET_DIR = $previousTargetDir
    Pop-Location
}
