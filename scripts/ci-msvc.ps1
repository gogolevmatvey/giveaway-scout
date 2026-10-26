[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("x64", "arm64")]
    [string]$Architecture,
    [switch]$ExportToGitHub
)
$ErrorActionPreference = "Stop"
# WSL-launched shells can inherit a truncated PATHEXT. Restore only the usual
# command extensions in this process; never edit user/system registry settings.
$extensions = @($env:PATHEXT -split ';' | Where-Object { $_ })
foreach ($extension in @('.EXE', '.COM', '.BAT', '.CMD')) {
    if ($extension -notin $extensions) { $extensions += $extension }
}
$env:PATHEXT = $extensions -join ';'
$hostArchitecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($hostArchitecture -ne $Architecture) {
    throw "Native CI requires $Architecture Windows, not $hostArchitecture/emulation."
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
# Component IDs differ between VS releases. Discover the installation, then
# validate the actual compiler environment rather than a legacy package ID.
$installation = & $vswhere -latest -products '*' -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $installation) { throw "Visual Studio/Build Tools not found." }
$shell = Join-Path $installation "Common7\Tools\Launch-VsDevShell.ps1"
$devArchitecture = if ($Architecture -eq "x64") { "amd64" } else { "arm64" }
& $shell -Arch $devArchitecture -HostArch $devArchitecture -SkipAutomaticLocation
if (-not (Get-Command lib.exe -ErrorAction SilentlyContinue) -or -not $env:LIB) {
    throw "MSVC developer environment did not provide lib.exe/LIB."
}
$expected = if ($Architecture -eq "x64") { @("x64", "amd64") } else { @("arm64") }
if ($env:VSCMD_ARG_HOST_ARCH -notin $expected -or $env:VSCMD_ARG_TGT_ARCH -notin $expected) {
    throw "MSVC configured a different host/target architecture."
}
if ($ExportToGitHub) {
    if ($env:GITHUB_ACTIONS -ne "true" -or -not $env:GITHUB_ENV) { throw "GitHub environment export is CI-only." }
    # Export compiler variables, never dump all environment variables/secrets.
    $names = @("PATH", "PATHEXT", "INCLUDE", "LIB", "LIBPATH", "VCToolsInstallDir", "WindowsSdkDir", "WindowsSDKVersion", "UniversalCRTSdkDir", "UCRTVersion", "VSCMD_ARG_HOST_ARCH", "VSCMD_ARG_TGT_ARCH")
    $lines = [System.Text.StringBuilder]::new()
    foreach ($name in $names) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value) {
            $delimiter = "GIVEAWAY_SCOUT_$([guid]::NewGuid().ToString('N'))"
            [void]$lines.AppendLine("$name<<$delimiter").AppendLine($value).AppendLine($delimiter)
        }
    }
    [IO.File]::AppendAllText($env:GITHUB_ENV, $lines.ToString(), [Text.UTF8Encoding]::new($false))
}
Write-Host "Native MSVC environment ready: $Architecture; target $env:VSCMD_ARG_TGT_ARCH."
