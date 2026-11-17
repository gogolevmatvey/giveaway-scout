param(
    [string]$Setup,
    [ValidateSet('auto', 'ru', 'en')][string]$Language = 'auto',
    [string]$OutputDirectory,
    [switch]$MeasureOnly
)

$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$project = Join-Path $workspace 'tests/GiveawayScout.InstallerUi/GiveawayScout.InstallerUi.csproj'
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $workspace ('artifacts/installer-ui/' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff') + '-' + $Language)
}
if (-not $MeasureOnly -and -not $Setup) {
    throw 'Provide -Setup <workspace setup.exe>, or -MeasureOnly. No installation will be started.'
}

# Keep SDK first-run state and temporary files in the workspace; do not add a
# tools path, generate development certificates, or alter persistent settings.
$toolState = Join-Path $workspace '.tools/dotnet-installer-ui'
$toolTemp = Join-Path $workspace 'artifacts/installer-ui-tools/tmp'
New-Item -ItemType Directory -Path $toolState, $toolTemp -Force | Out-Null
$environment = @{
    DOTNET_CLI_HOME = $toolState
    DOTNET_CLI_TELEMETRY_OPTOUT = '1'
    DOTNET_SKIP_FIRST_TIME_EXPERIENCE = 'true'
    DOTNET_GENERATE_ASPNET_CERTIFICATE = 'false'
    DOTNET_ADD_GLOBAL_TOOLS_TO_PATH = 'false'
    TEMP = $toolTemp
    TMP = $toolTemp
}
$previous = @{}
try {
    foreach ($key in $environment.Keys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process')
    }
    $dotnet = (Get-Command dotnet -ErrorAction Stop).Source
    & $dotnet build $project -p:RestoreLockedMode=true -p:RestoreSources= --ignore-failed-sources -v:minimal
    if ($LASTEXITCODE -ne 0) { throw "Installer UI harness build failed: $LASTEXITCODE" }
    $arguments = @(
        (Join-Path $workspace 'tests/GiveawayScout.InstallerUi/bin/Debug/net10.0-windows/GiveawayScout.InstallerUi.dll'),
        '--workspace', $workspace, '--output', $OutputDirectory, '--language', $Language
    )
    if ($MeasureOnly) { $arguments += '--measure-only' }
    else {
        $setupPath = if ([IO.Path]::IsPathRooted($Setup)) { $Setup } else { Join-Path $workspace $Setup }
        $arguments += @('--setup', [IO.Path]::GetFullPath($setupPath))
    }
    & $dotnet @arguments
    if ($LASTEXITCODE -ne 0) { throw "Installer UI acceptance failed: $LASTEXITCODE. Inspect $OutputDirectory" }
}
finally {
    foreach ($key in $previous.Keys) {
        [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process')
    }
}
