# Builds the Magpie installer. Puts Rust's cargo on PATH first, since a fresh shell may not have it.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location (Split-Path $PSScriptRoot)
# Do not terminate a user's running app or pending work to produce an installer.
$ErrorActionPreference = 'Stop'
npm run typecheck
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
npm run test:renderer
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
npm run build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($LASTEXITCODE -eq 0) { Get-ChildItem src-tauri\target\release\bundle\nsis\*.exe | Select-Object FullName, Length }
