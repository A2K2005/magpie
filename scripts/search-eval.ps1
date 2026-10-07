param(
    [Parameter(Mandatory = $true)][string]$Models,
    [Parameter(Mandatory = $true)][string]$Manifest,
    [Parameter(Mandatory = $true)][string]$Report,
    [ValidateRange(1, 8)][int]$BatchSize = 8
)
$ErrorActionPreference = 'Stop'
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $cargo)) { throw 'Install the Rust toolchain or add cargo to PATH.' }
} else { $cargo = $cargo.Source }
& $cargo run --manifest-path (Join-Path $PSScriptRoot '..\src-tauri\Cargo.toml') --example search_eval -- $Models $Manifest $Report $BatchSize
if ($LASTEXITCODE -ne 0) { throw "Search evaluation failed ($LASTEXITCODE)." }
