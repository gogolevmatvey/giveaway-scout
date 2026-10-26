[CmdletBinding()]
param(
    [string]$PackageDirectory = "",
    [switch]$CheckOnly
)

$ErrorActionPreference = "Stop"
$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($PackageDirectory)) {
    $PackageDirectory = Join-Path $scriptDirectory "..\artifacts\updater-setup-2026-10-25"
}
$PackageDirectory = (Resolve-Path -LiteralPath $PackageDirectory).Path
$node = "C:\Program Files\nodejs\node.exe"
$cli = Join-Path $scriptDirectory "..\src\GiveawayScout.Desktop\node_modules\@tauri-apps\cli\tauri.js"
$key = Join-Path $env:LOCALAPPDATA "GiveawayScoutSigning\updater.key"
$statusPath = Join-Path $PackageDirectory "signing-status.json"
$versions = @("1.0.0", "1.0.1")
$password = $null
$status = @{ completed = $false; versions = $versions; error = $null }

try {
    foreach ($required in @($node, $cli, $key, ($key + ".pub"))) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "Required signing input is missing."
        }
    }
    foreach ($version in $versions) {
        $directory = Join-Path $PackageDirectory $version
        $installer = Join-Path $directory "Giveaway Scout_${version}_x64-setup.exe"
        $info = Get-Content -LiteralPath (Join-Path $directory "unsigned-build.json") -Raw | ConvertFrom-Json
        if ($info.version -ne $version -or $info.installerSha256 -ne (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()) {
            throw "Installer changed since preparation. Signing stopped."
        }
    }

    if ($CheckOnly) {
        Write-Host "PASS: paths, key presence and both installer hashes verified. No password requested and no signature created."
        return
    }

    Write-Host "Будут подписаны подготовленные установщики Giveaway Scout 1.0.0 и 1.0.1."
    Write-Host "Пароль вводится только в этом окне и не сохраняется в файлы."
    $password = Read-Host "Пароль ключа обновлений" -AsSecureString
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [System.Net.NetworkCredential]::new("", $password).Password
    $env:PATHEXT = ".COM;.EXE;.BAT;.CMD"
    foreach ($version in $versions) {
        $installer = Join-Path (Join-Path $PackageDirectory $version) "Giveaway Scout_${version}_x64-setup.exe"
        if (Test-Path -LiteralPath ($installer + ".sig")) {
            Write-Host "Подпись $version уже существует; она будет отдельно проверена перед публикацией."
            continue
        }
        & $node $cli signer sign --private-key-path $key --app-version $version $installer
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath ($installer + ".sig"))) {
            throw "Signing failed. No release was published."
        }
    }
    $status.completed = $true
    Write-Host "Подписи созданы. Криптографическая проверка и публикация выполняются отдельно."
}
catch {
    if ($CheckOnly) { throw }
    $status.error = "Signing did not finish; see the local console."
    Write-Host "Подпись не завершена. Проверьте ввод пароля и наличие файлов; ключ и пароль никому не отправляйте."
}
finally {
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    if ($password) { $password.Dispose() }
    if (-not $CheckOnly) {
        $status | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $statusPath -Encoding UTF8
    }
}
