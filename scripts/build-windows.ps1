[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$CheckOnly,
    [switch]$DeferUpdaterSigning,
    [string]$UpdaterConfig = "",
    [ValidatePattern('\A[a-zA-Z0-9-]{0,32}\z')]
    [string]$BuildLabel = ""
)

$ErrorActionPreference = "Stop"

if ($DeferUpdaterSigning -and -not $UpdaterConfig) {
    throw "-DeferUpdaterSigning requires -UpdaterConfig."
}

$pnpm = Join-Path $env:APPDATA "npm\pnpm.cmd"
$node = "C:\Program Files\nodejs\node.exe"
$desktopDirectory = Resolve-Path (Join-Path $PSScriptRoot "..\src\GiveawayScout.Desktop")
$tauriDirectory = Join-Path $desktopDirectory "src-tauri"
$targetDirectoryName = "target-windows-x64"
if ($BuildLabel) {
    $targetDirectoryName += "-$BuildLabel"
}
$targetDirectory = Join-Path $tauriDirectory $targetDirectoryName
$releaseExecutable = Join-Path $targetDirectory "release\giveaway-scout.exe"

if (-not $CheckOnly) {
    $runningCopies = Get-CimInstance Win32_Process -Filter "Name = 'giveaway-scout.exe'" |
        Where-Object { $_.ExecutablePath -eq $releaseExecutable }
    if ($runningCopies) {
        throw "Cannot replace the running executable: $releaseExecutable. Quit Giveaway Scout from the tray, or build into another directory with -BuildLabel next. Use -CheckOnly to run checks without replacing the EXE."
    }
}
$frontendBin = Join-Path $desktopDirectory "node_modules\.bin"
$toolchainFile = Join-Path $desktopDirectory "rust-toolchain.toml"
$toolchainVersion = Select-String -Path $toolchainFile -Pattern '^channel\s*=\s*"([^"]+)"$' |
    ForEach-Object { $_.Matches[0].Groups[1].Value } |
    Select-Object -First 1

if (-not $toolchainVersion) {
    throw "Rust toolchain version was not found in $toolchainFile"
}

$toolchainBin = Join-Path $env:USERPROFILE ".rustup\toolchains\$toolchainVersion-x86_64-pc-windows-msvc\bin"
$cargo = Join-Path $toolchainBin "cargo.exe"
$rustc = Join-Path $toolchainBin "rustc.exe"

foreach ($tool in @($cargo, $rustc, $pnpm, $node)) {
    if (-not (Test-Path $tool)) {
        throw "Required build tool was not found: $tool"
    }
}

$visualStudio = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\BuildTools" |
    Sort-Object FullName -Descending |
    Select-Object -First 1
if (-not $visualStudio) {
    throw "Visual Studio Build Tools were not found."
}

$msvcTools = Get-ChildItem (Join-Path $visualStudio.FullName "VC\Tools\MSVC") |
    Sort-Object { [version]$_.Name } -Descending |
    Select-Object -First 1
$windowsSdkRoot = "${env:ProgramFiles(x86)}\Windows Kits\10"
$windowsSdkVersion = Get-ChildItem (Join-Path $windowsSdkRoot "Lib") |
    Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Name } -Descending |
    Select-Object -First 1
if (-not $msvcTools -or -not $windowsSdkVersion) {
    throw "MSVC tools or the Windows SDK were not found."
}

$msvcBin = Join-Path $msvcTools.FullName "bin\Hostx64\x64"
$sdkBin = Join-Path $windowsSdkRoot "bin\$($windowsSdkVersion.Name)\x64"
$env:VCToolsInstallDir = "$($msvcTools.FullName)\"
$env:WindowsSdkDir = "$windowsSdkRoot\"
$env:WindowsSDKVersion = "$($windowsSdkVersion.Name)\"
$env:LIB = @(
    (Join-Path $msvcTools.FullName "lib\x64"),
    (Join-Path $windowsSdkVersion.FullName "um\x64"),
    (Join-Path $windowsSdkVersion.FullName "ucrt\x64")
) -join ";"
$env:INCLUDE = @(
    (Join-Path $msvcTools.FullName "include"),
    (Join-Path $windowsSdkRoot "Include\$($windowsSdkVersion.Name)\ucrt"),
    (Join-Path $windowsSdkRoot "Include\$($windowsSdkVersion.Name)\shared"),
    (Join-Path $windowsSdkRoot "Include\$($windowsSdkVersion.Name)\um"),
    (Join-Path $windowsSdkRoot "Include\$($windowsSdkVersion.Name)\winrt"),
    (Join-Path $windowsSdkRoot "Include\$($windowsSdkVersion.Name)\cppwinrt")
) -join ";"
$env:RUSTC = $rustc
$env:PATHEXT = (@(".COM", ".EXE", ".BAT", ".CMD") + @($env:PATHEXT -split ";") | Select-Object -Unique) -join ";"
$env:PATH = "$frontendBin;$toolchainBin;$msvcBin;$sdkBin;C:\Program Files\nodejs;$env:PATH"
$env:CARGO_TARGET_DIR = $targetDirectory

Write-Host "Using Rust $toolchainVersion, MSVC $($msvcTools.Name), and Windows SDK $($windowsSdkVersion.Name)"

function Invoke-BuildCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,

        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory
    )

    $process = Start-Process `
        -FilePath $env:ComSpec `
        -ArgumentList @("/d", "/s", "/c", $Command) `
        -WorkingDirectory $WorkingDirectory `
        -NoNewWindow `
        -Wait `
        -PassThru

    if ($process.ExitCode -ne 0) {
        exit $process.ExitCode
    }
}

function Invoke-CheckedProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$ProcessArguments,

        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory
    )

    $commandFile = Join-Path $env:TEMP "giveaway-scout-$([guid]::NewGuid()).cmd"
    $exitCodeFile = Join-Path $env:TEMP "giveaway-scout-$([guid]::NewGuid()).exit"
    $argumentText = ($ProcessArguments | ForEach-Object {
        '"' + $_.Replace('"', '""') + '"'
    }) -join " "
    $commandText = @"
@echo off
pushd "$WorkingDirectory"
"$FilePath" $argumentText
set "GIVEAWAY_SCOUT_EXIT=%ERRORLEVEL%"
popd
> "$exitCodeFile" echo %GIVEAWAY_SCOUT_EXIT%
exit /b %GIVEAWAY_SCOUT_EXIT%
"@

    try {
        Set-Content -Path $commandFile -Value $commandText -Encoding ASCII
        $process = Start-Process `
            -FilePath $env:ComSpec `
            -ArgumentList @("/d", "/s", "/c", "`"$commandFile`"") `
            -NoNewWindow `
            -PassThru
        $process.WaitForExit()
        if (-not (Test-Path $exitCodeFile)) {
            throw "Build command did not report an exit code: $FilePath"
        }
        $exitCode = [int](Get-Content -Raw $exitCodeFile).Trim()
    }
    finally {
        Remove-Item $commandFile, $exitCodeFile -Force -ErrorAction SilentlyContinue
    }

    if ($exitCode -ne 0) {
        exit $exitCode
    }
}

if ($UpdaterConfig) {
    $UpdaterConfig = (Resolve-Path -LiteralPath $UpdaterConfig).Path
    Invoke-CheckedProcess -FilePath $node -ProcessArguments @(
        (Join-Path $PSScriptRoot "prepare-github-updates.mjs"), "--verify", $UpdaterConfig
    ) -WorkingDirectory $desktopDirectory
    if (-not $CheckOnly -and -not $DeferUpdaterSigning -and -not $env:TAURI_SIGNING_PRIVATE_KEY) {
        throw "A signed release requires TAURI_SIGNING_PRIVATE_KEY. Do not put the private key in the repository."
    }
}

Write-Host "Restoring Windows frontend dependencies..."
Invoke-BuildCommand -Command "`"$pnpm`" install --frozen-lockfile --force" -WorkingDirectory $desktopDirectory

if (-not $SkipTests) {
    Invoke-CheckedProcess -FilePath $node -ProcessArguments @(
        "--test", (Join-Path $PSScriptRoot "prepare-github-updates.test.mjs"), (Join-Path $PSScriptRoot "installer-config.test.mjs")
    ) -WorkingDirectory $desktopDirectory
    Write-Host "Running Rust tests..."
    Invoke-CheckedProcess -FilePath $cargo -ProcessArguments @("test", "--workspace", "--locked") -WorkingDirectory $tauriDirectory

    Write-Host "Running frontend tests..."
    Invoke-CheckedProcess -FilePath $node -ProcessArguments @(".\node_modules\vitest\vitest.mjs", "run") -WorkingDirectory $desktopDirectory
}

Write-Host "Building the frontend..."
Invoke-CheckedProcess -FilePath $node -ProcessArguments @(".\node_modules\typescript\bin\tsc", "--noEmit") -WorkingDirectory $desktopDirectory
Invoke-CheckedProcess -FilePath $node -ProcessArguments @(".\node_modules\typescript\bin\tsc", "--noEmit", "-p", "tsconfig.node.json") -WorkingDirectory $desktopDirectory
Invoke-CheckedProcess -FilePath $node -ProcessArguments @(".\node_modules\vite\bin\vite.js", "build") -WorkingDirectory $desktopDirectory

if ($CheckOnly) {
    Write-Host "Windows checks completed. Release files were not rebuilt."
    exit 0
}

Write-Host "Building the NSIS installer..."
$tauriArguments = @(".\node_modules\@tauri-apps\cli\tauri.js", "build", "--bundles", "nsis", "--config", "src-tauri\build-windows.conf.json")
if ($UpdaterConfig) { $tauriArguments += @("--config", $UpdaterConfig) }
if ($DeferUpdaterSigning) {
    $tauriArguments += @("--config", (Join-Path $tauriDirectory "build-deferred-signing.conf.json"))
}
$tauriArguments += @("--", "--locked")
Invoke-CheckedProcess -FilePath $node -ProcessArguments $tauriArguments -WorkingDirectory $desktopDirectory
Invoke-CheckedProcess -FilePath $node -ProcessArguments @(
    (Join-Path $PSScriptRoot "check-installer-bundle.mjs")
) -WorkingDirectory $desktopDirectory
Write-Host "Portable executable: $releaseExecutable"
Write-Host "NSIS installer directory: $(Join-Path $targetDirectory 'release\bundle\nsis')"
if ($DeferUpdaterSigning) {
    Write-Host "Updater channel is included, but the installer still needs its updater signature before publication."
}
