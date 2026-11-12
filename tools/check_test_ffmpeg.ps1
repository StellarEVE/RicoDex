$ErrorActionPreference = "Stop"

$testFiles = @(
    Get-ChildItem -Path "crates" -Recurse -File -Filter "*.rs" |
        Where-Object { $_.FullName -match '[\\/]+tests[\\/].+\.rs$' }
)

if ($testFiles.Count -eq 0) {
    throw "No Rust test source files were found under crates/*/tests."
}

$problems = @()
foreach ($file in $testFiles) {
    $source = Get-Content -LiteralPath $file.FullName -Raw

    if ($source -match '\blibx264\b') {
        if ($source -notmatch 'RICODEX_TEST_FFMPEG') {
            $problems += "$($file.FullName): references libx264 but does not use RICODEX_TEST_FFMPEG."
        }

        if ([regex]::IsMatch($source, 'Command::new\(\s*"ffmpeg(?:\.exe)?"\s*\)')) {
            $problems += "$($file.FullName): hard-codes ffmpeg instead of using the configured test executable."
        }
    }
}

if ($problems.Count -gt 0) {
    $problems | ForEach-Object { Write-Error $_ }
    throw "H.264 fixture audit failed."
}

Write-Host "H.264 fixture audit passed across $($testFiles.Count) Rust test source files."
