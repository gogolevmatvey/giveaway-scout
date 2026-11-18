# Run in an isolated Windows 11 Hyper-V guest after taking a checkpoint.
# Default mode performs only read-only identity, Node and frozen-input checks.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ExpectedComputerName,
    [Parameter(Mandatory = $true)][guid]$ExpectedVmUuid,
    [Parameter(Mandatory = $true)][string]$StateDirectory,
    [string]$InputDirectory,
    [string]$ExpectedVersion,
    [ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedManifestSha256,
    [ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedVerificationSha256,
    [string]$PublicKey,
    [string]$ReleaseVerifier,
    [ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedVerifierSha256,
    [string]$NodePath,
    [ValidateSet('normal', 'manifest-503', 'package-503', 'manifest-oversize', 'package-oversize', 'invalid-signature', 'truncated-package')]
    [string]$Mode = 'normal',
    [switch]$ExecuteInTestVm,
    [switch]$Cleanup
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-TestGuest {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'A Windows test VM is required.' }
    $system = Get-CimInstance -ClassName Win32_ComputerSystem
    $product = Get-CimInstance -ClassName Win32_ComputerSystemProduct
    $os = Get-CimInstance -ClassName Win32_OperatingSystem
    if ($system.Manufacturer -ne 'Microsoft Corporation' -or $system.Model -ne 'Virtual Machine' -or
        $system.Name -ine $ExpectedComputerName -or $env:COMPUTERNAME -ine $ExpectedComputerName -or
        [guid]$product.UUID -ne $ExpectedVmUuid -or $ExpectedVmUuid -eq [guid]::Empty -or
        $ExpectedVmUuid -eq [guid]'ffffffff-ffff-ffff-ffff-ffffffffffff' -or
        $os.ProductType -ne 1 -or [int]$os.BuildNumber -lt 22000) {
        throw 'Guest guard failed: expected Windows 11 Hyper-V computer name and UUID must match CIM. No system change is allowed.'
    }
}

function Assert-LocalPath([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value) -or -not [IO.Path]::IsPathRooted($Value) -or $Value.StartsWith('\\')) {
        throw 'Use an explicit absolute local path.'
    }
    $resolved = [IO.Path]::GetFullPath($Value)
    $cursor = $resolved
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw 'Reparse points are not accepted in the private state path.'
            }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
    return $resolved
}

function Set-PrivateDirectoryAcl([string]$Directory) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    foreach ($sid in @($identity, [Security.Principal.SecurityIdentifier]::new('S-1-5-18'),
        [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl',
            'ContainerInherit, ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Directory -AclObject $acl
}

function Write-NewJson([string]$Filename, $Value) {
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($Value | ConvertTo-Json -Depth 8))
    $file = [IO.File]::Open($Filename, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
}

function Set-OwnHostsBlock([string]$Marker, [bool]$Add) {
    $hostsPath = Join-Path $env:windir 'System32\drivers\etc\hosts'
    $encoding = [Text.Encoding]::GetEncoding(28591) # Preserve every existing byte, including any BOM.
    $block = "`r`n# BEGIN $Marker`r`n127.0.0.1 github.com`r`n# END $Marker`r`n"
    # Hold a write-denying handle over the read/modify/write operation. Re-read
    # now rather than overwriting the earlier preflight snapshot.
    $file = [IO.File]::Open($hostsPath, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::Read)
    try {
        if ($file.Length -gt 1048576) { throw 'Unexpected hosts file size.' }
        $bytes = New-Object byte[] ([int]$file.Length)
        $offset = 0
        while ($offset -lt $bytes.Length) {
            $read = $file.Read($bytes, $offset, $bytes.Length - $offset)
            if ($read -eq 0) { throw 'Cannot read the complete hosts file.' }
            $offset += $read
        }
        $text = $encoding.GetString($bytes)
        if ($Add) {
            if ($text.Contains($Marker) -or $text -match '(?im)^[ \t]*[^#\r\n]+[ \t]github\.com(?:[ \t#]|\r?$)') {
                throw 'An existing github.com hosts mapping must be reviewed first.'
            }
            $text += $block
        } elseif ($text.Contains($block)) { $text = $text.Replace($block, '') }
        elseif ($text.Contains($Marker)) { throw 'Fixture hosts block was edited; cleanup refused to alter other entries.' }
        else { return }
        $updated = $encoding.GetBytes($text)
        $file.Position = 0
        $file.Write($updated, 0, $updated.Length)
        $file.SetLength($updated.Length)
        $file.Flush($true)
    } finally { $file.Dispose() }
}

function Remove-OwnFixture([string]$Directory) {
    $statePath = Join-Path $Directory 'fixture-state.json'
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) { throw 'No fixture state exists; cleanup refused.' }
    $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    if ($state.schemaVersion -ne 1 -or $state.computerName -ine $ExpectedComputerName -or
        [guid]$state.vmUuid -ne $ExpectedVmUuid -or [IO.Path]::GetFullPath($state.stateDirectory) -ine $Directory -or
        $state.fixtureId -notmatch '^[0-9a-f]{32}$') { throw 'Fixture ownership check failed.' }
    $marker = "GiveawayScoutUpdateFixture:$($state.fixtureId)"
    $cleanupErrors = [Collections.Generic.List[string]]::new()
    $processFile = Join-Path $Directory 'server-process.json'
    try { if (Test-Path -LiteralPath $processFile -PathType Leaf) {
        $saved = Get-Content -LiteralPath $processFile -Raw | ConvertFrom-Json
        $running = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $([int]$saved.pid)"
        if ($running) {
            $privateConfig = Join-Path $Directory 'private-config.json'
            if ($saved.privateConfig -ine $privateConfig -or $running.ExecutablePath -ine $saved.executable -or
                -not $running.CommandLine.Contains($privateConfig)) { throw 'Server process ownership check failed; cleanup refused.' }
            Stop-Process -Id $saved.pid -ErrorAction Stop
        }
    } } catch { $cleanupErrors.Add('server process ownership or stop') }
    try { Set-OwnHostsBlock $marker $false } catch { $cleanupErrors.Add('hosts block ownership or removal') }
    foreach ($entry in @(@{ Store = 'Root'; Thumbprint = $state.rootThumbprint }, @{ Store = 'My'; Thumbprint = $state.rootThumbprint },
        @{ Store = 'My'; Thumbprint = $state.leafThumbprint })) {
        try { if ($entry.Thumbprint) {
            if ($entry.Thumbprint -notmatch '^[A-Fa-f0-9]{40}$') { throw 'Invalid stored certificate thumbprint.' }
            $certificatePath = "Cert:\LocalMachine\$($entry.Store)\$($entry.Thumbprint)"
            $certificate = Get-Item -LiteralPath $certificatePath -ErrorAction SilentlyContinue
            if ($certificate) {
                # A .cer import does not carry FriendlyName. The root subject
                # contains this run random marker, including after a failed import.
                $isRoot = $entry.Thumbprint -eq $state.rootThumbprint
                if (($isRoot -and $certificate.Subject -ne "CN=$marker") -or
                    (-not $isRoot -and $certificate.FriendlyName -ne $marker)) { throw 'Certificate ownership check failed.' }
                if ($entry.Store -eq 'My') { Remove-Item -LiteralPath $certificatePath -DeleteKey }
                else { Remove-Item -LiteralPath $certificatePath }
            }
        } } catch { $cleanupErrors.Add("certificate $($entry.Store) ownership or removal") }
    }
    foreach ($name in @('github-test.pfx', 'private-config.json', 'root-public.cer')) {
        try {
            $filename = Join-Path $Directory $name
            if (Test-Path -LiteralPath $filename -PathType Leaf) { Remove-Item -LiteralPath $filename }
        } catch { $cleanupErrors.Add("private export $name removal") }
    }
    try { Clear-DnsClientCache } catch { $cleanupErrors.Add('DNS cache refresh') }
    if ($cleanupErrors.Count) {
        throw "Fixture cleanup incomplete: $($cleanupErrors -join '; '). Restore the test VM checkpoint or review this run state before rerunning cleanup."
    }
    Write-Host 'Removed this fixture hosts entry, test certificates and private TLS export. Request logs and state remain for evidence.'
}

Assert-TestGuest
$StateDirectory = Assert-LocalPath $StateDirectory
if ($Cleanup) {
    if (-not $ExecuteInTestVm) { Write-Host 'Preflight only. Cleanup requires -ExecuteInTestVm in this verified guest.'; return }
    $administrator = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $administrator.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run elevated inside the test VM.' }
    Remove-OwnFixture $StateDirectory
    return
}
foreach ($required in @($InputDirectory, $ExpectedVersion, $ExpectedManifestSha256, $ExpectedVerificationSha256,
    $PublicKey, $ReleaseVerifier, $ExpectedVerifierSha256, $NodePath)) {
    if ([string]::IsNullOrWhiteSpace($required)) { throw 'All frozen-input, verifier and Node arguments are required.' }
}
$InputDirectory = (Resolve-Path -LiteralPath $InputDirectory).Path
if ($StateDirectory.StartsWith($InputDirectory.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or
    $StateDirectory -ieq $InputDirectory) { throw 'Private state must be outside the read-only release input.' }
if (Test-Path -LiteralPath $StateDirectory) { throw 'Use a new private state directory for each fixture run.' }
$nodeVersion = & $NodePath -p 'process.versions.node'
if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^24\.') { throw 'Use the checked Node 24 runtime.' }
$server = Join-Path $PSScriptRoot 'update-fixture-server.mjs'
$arguments = @($server, '--input-directory', $InputDirectory, '--expected-version', $ExpectedVersion,
    '--expected-manifest-sha256', $ExpectedManifestSha256, '--expected-verification-sha256', $ExpectedVerificationSha256,
    '--public-key', $PublicKey, '--release-verifier', $ReleaseVerifier, '--expected-verifier-sha256', $ExpectedVerifierSha256,
    '--mode', $Mode)
& $NodePath @arguments --inspect-only
if ($LASTEXITCODE -ne 0) { throw 'Frozen input preflight failed.' }
if (-not $ExecuteInTestVm) {
    Write-Host 'Preflight passed. No certificates, files, hosts entries or listeners were created. Take a VM checkpoint before executing.'
    return
}
$administrator = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $administrator.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run elevated inside the test VM.' }
$hostsPath = Join-Path $env:windir 'System32\drivers\etc\hosts'
$encoding = [Text.Encoding]::GetEncoding(28591)
$hostsText = $encoding.GetString([IO.File]::ReadAllBytes($hostsPath))
if ($hostsText -match '(?im)^[ \t]*[^#\r\n]+[ \t]github\.com(?:[ \t#]|\r?$)') { throw 'An existing github.com hosts mapping must be reviewed first.' }
if (Get-NetTCPConnection -LocalPort 443 -State Listen -ErrorAction SilentlyContinue) { throw 'Port 443 is already in use in this guest.' }
New-Item -ItemType Directory -Path $StateDirectory -ErrorAction Stop | Out-Null
Set-PrivateDirectoryAcl $StateDirectory
$fixtureId = [guid]::NewGuid().ToString('N')
$marker = "GiveawayScoutUpdateFixture:$fixtureId"
$statePath = Join-Path $StateDirectory 'fixture-state.json'
$state = [ordered]@{ schemaVersion = 1; fixtureId = $fixtureId; computerName = $ExpectedComputerName;
    vmUuid = $ExpectedVmUuid.ToString(); stateDirectory = $StateDirectory; rootThumbprint = $null; leafThumbprint = $null }
Write-NewJson $statePath $state
try {
    # Both TLS private keys are generated only in the verified guest. The CA key cannot be exported.
    $root = New-SelfSignedCertificate -Type Custom -Subject "CN=$marker" -FriendlyName $marker -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 `
        -KeyUsage CertSign, CRLSign, DigitalSignature -KeyExportPolicy NonExportable -CertStoreLocation Cert:\LocalMachine\My `
        -TextExtension @('2.5.29.19={critical}{text}ca=1&pathlength=0') -NotAfter (Get-Date).AddDays(7)
    $state.rootThumbprint = $root.Thumbprint
    $state | ConvertTo-Json | Set-Content -LiteralPath $statePath -Encoding UTF8
    $leaf = New-SelfSignedCertificate -Type Custom -Subject 'CN=github.com' -DnsName 'github.com' -FriendlyName $marker `
        -Signer $root -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 -KeyExportPolicy Exportable -KeyUsage DigitalSignature, KeyEncipherment `
        -CertStoreLocation Cert:\LocalMachine\My -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.1', '2.5.29.19={critical}{text}ca=0') `
        -NotAfter (Get-Date).AddDays(2)
    $state.leafThumbprint = $leaf.Thumbprint
    $state | ConvertTo-Json | Set-Content -LiteralPath $statePath -Encoding UTF8
    $random = New-Object byte[] 32
    $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $generator.GetBytes($random) } finally { $generator.Dispose() }
    $passwordText = [Convert]::ToBase64String($random)
    $password = ConvertTo-SecureString -String $passwordText -AsPlainText -Force
    Export-PfxCertificate -Cert $leaf -FilePath (Join-Path $StateDirectory 'github-test.pfx') -Password $password -ChainOption EndEntityCertOnly | Out-Null
    Export-Certificate -Cert $root -FilePath (Join-Path $StateDirectory 'root-public.cer') | Out-Null
    $trustedRoot = Import-Certificate -FilePath (Join-Path $StateDirectory 'root-public.cer') -CertStoreLocation Cert:\LocalMachine\Root
    $trustedRoot.FriendlyName = $marker
    $configPath = Join-Path $StateDirectory 'private-config.json'
    Write-NewJson $configPath @{ schemaVersion = 1; fixtureId = $fixtureId; guestGuardVerified = $true;
        computerName = $ExpectedComputerName; vmUuid = $ExpectedVmUuid.ToString(); pfxPassword = $passwordText }
    $passwordText = $null
    $password.Dispose()
    Set-OwnHostsBlock $marker $true
    Clear-DnsClientCache
    Write-Host 'Serving inside this test VM. Stop with Ctrl+C; cleanup removes this run TLS trust and hosts block. Other GitHub traffic is unavailable while mapped.'
    & $NodePath @arguments --private-config $configPath
    if ($LASTEXITCODE -ne 0) { throw 'Fixture server failed.' }
} finally {
    # Manual recovery after termination: repeat identity/state arguments with -Cleanup -ExecuteInTestVm.
    Remove-OwnFixture $StateDirectory
}
