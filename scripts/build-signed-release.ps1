[CmdletBinding()]
param(
    [ValidatePattern('\A[a-zA-Z0-9-]{0,32}\z')]
    [string]$BuildLabel = "signed",
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$key = Join-Path $env:LOCALAPPDATA "GiveawayScoutSigning\updater.key"
$config = Join-Path $PSScriptRoot "..\src\GiveawayScout.Desktop\src-tauri\release-windows-updater.conf.json"
if (-not (Test-Path -LiteralPath $key -PathType Leaf)) {
    throw "Existing updater signing key was not found."
}
$password = $null
try {
    $password = Read-Host "Пароль ключа обновлений" -AsSecureString
    $env:TAURI_SIGNING_PRIVATE_KEY = $key
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [System.Net.NetworkCredential]::new("", $password).Password
    & (Join-Path $PSScriptRoot "build-windows.ps1") -BuildLabel $BuildLabel -UpdaterConfig $config -SkipTests:$SkipTests
}
finally {
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    if ($password) { $password.Dispose() }
}
