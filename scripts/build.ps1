# Builds the Magpie installer. Puts Rust's cargo on PATH first, since a fresh shell may not have it.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location (Split-Path $PSScriptRoot)
# A running Magpie locks magpie.exe, and the build fails with "Access is denied".
Stop-Process -Name magpie -ErrorAction SilentlyContinue
npm run build
if ($LASTEXITCODE -eq 0) { Get-ChildItem src-tauri\target\release\bundle\nsis\*.exe | Select-Object FullName, Length }
