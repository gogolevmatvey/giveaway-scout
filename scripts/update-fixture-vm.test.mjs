// Exercise helper functions against disposable files and mocked certificate
// cmdlets. These tests never use the real hosts file, certificate store or DNS.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const helper = fileURLToPath(new URL('./initialize-update-test-vm.ps1', import.meta.url));
const quote = value => `'${value.replaceAll("'", "''")}'`;
function powershellTest(body) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'giveaway-scout-fixture-ps-test-'));
  const source = `
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile(${quote(helper)}, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Helper parse failed.' }
foreach ($name in @('Assert-TestGuest', 'Set-OwnHostsBlock', 'Remove-OwnFixture')) {
    $functionAst = $ast.Find({ param($entry) $entry -is [Management.Automation.Language.FunctionDefinitionAst] -and $entry.Name -eq $name }, $true)
    . ([scriptblock]::Create($functionAst.Extent.Text))
}
$temporary = ${quote(root)}
$env:windir = $temporary
$hostsDirectory = Join-Path $temporary 'System32\\drivers\\etc'
New-Item -Path $hostsDirectory -ItemType Directory -Force | Out-Null
$hostsPath = Join-Path $hostsDirectory 'hosts'
${body}
Write-Output 'PASS'
`;
  try {
    const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand',
      Buffer.from(source, 'utf16le').toString('base64')], { encoding: 'utf8', windowsHide: true, timeout: 30_000 });
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    assert.match(result.stdout, /PASS/);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
}

test('PowerShell 5 helper preserves unrelated hosts bytes and refuses edited owned blocks', { skip: process.platform !== 'win32' }, () => {
  powershellTest(`
$original = [byte[]]@(239, 187, 191, 35, 32, 101, 120, 105, 115, 116, 105, 110, 103, 13, 10)
[IO.File]::WriteAllBytes($hostsPath, $original)
$marker = 'GiveawayScoutUpdateFixture:01234567890123456789012345678901'
Set-OwnHostsBlock $marker $true
$other = '127.0.0.2 unrelated.example' + "\r\n"
[IO.File]::AppendAllText($hostsPath, $other, [Text.Encoding]::ASCII)
Set-OwnHostsBlock $marker $false
$expected = [Convert]::ToBase64String($original + [Text.Encoding]::ASCII.GetBytes($other))
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($hostsPath)) -ne $expected) { throw 'Unrelated hosts bytes changed.' }
foreach ($mapping in @('127.0.0.5 github.com', '127.0.0.5 github.com#comment')) {
    [IO.File]::WriteAllText($hostsPath, $mapping + "\r\n")
    $refused = $false
    try { Set-OwnHostsBlock $marker $true } catch { $refused = $true }
    if (-not $refused) { throw 'Existing GitHub mapping was accepted.' }
}
$edited = '# BEGIN ' + $marker + "\r\n" + '# edited'
[IO.File]::WriteAllText($hostsPath, $edited)
$refused = $false
try { Set-OwnHostsBlock $marker $false } catch { $refused = $true }
if (-not $refused -or [IO.File]::ReadAllText($hostsPath) -ne $edited) { throw 'Edited hosts block was changed.' }
`);
});

test('guest guard checks CIM identity before any system mutation', { skip: process.platform !== 'win32' }, () => {
  powershellTest(`
$ExpectedComputerName = $env:COMPUTERNAME
$ExpectedVmUuid = [guid]'12345678-abcd-1234-abcd-123456789abc'
$script:model = 'Virtual Machine'
function Get-CimInstance {
    param([string]$ClassName)
    switch ($ClassName) {
        'Win32_ComputerSystem' { @{ Manufacturer = 'Microsoft Corporation'; Model = $script:model; Name = $env:COMPUTERNAME } }
        'Win32_ComputerSystemProduct' { @{ UUID = '12345678-abcd-1234-abcd-123456789abc' } }
        'Win32_OperatingSystem' { @{ ProductType = 1; BuildNumber = '26200' } }
        default { throw 'Unexpected CIM call.' }
    }
}
Assert-TestGuest
$script:model = 'Physical desktop'
$refused = $false
try { Assert-TestGuest } catch { $refused = $true }
if (-not $refused) { throw 'Physical desktop was accepted.' }
`);
});

test('cleanup attempts all owned trust/key/private-file removals after a hosts cleanup failure', { skip: process.platform !== 'win32' }, () => {
  powershellTest(`
$ExpectedComputerName = $env:COMPUTERNAME
$ExpectedVmUuid = [guid]'12345678-abcd-1234-abcd-123456789abc'
$fixtureId = '01234567890123456789012345678901'
$marker = 'GiveawayScoutUpdateFixture:' + $fixtureId
$stateDirectory = Join-Path $temporary 'state'
New-Item -Path $stateDirectory -ItemType Directory | Out-Null
$state = @{ schemaVersion = 1; fixtureId = $fixtureId; computerName = $ExpectedComputerName;
    vmUuid = $ExpectedVmUuid.ToString(); stateDirectory = $stateDirectory;
    rootThumbprint = ('A' * 40); leafThumbprint = ('B' * 40) }
$state | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stateDirectory 'fixture-state.json')
foreach ($name in @('github-test.pfx', 'private-config.json', 'root-public.cer')) {
    [IO.File]::WriteAllText((Join-Path $stateDirectory $name), 'ephemeral fixture')
}
[IO.File]::WriteAllText($hostsPath, '# BEGIN ' + $marker + ' edited')
$script:removed = [Collections.Generic.List[string]]::new()
$script:dnsRefreshed = $false
function Get-Item {
    [CmdletBinding()]param([string]$LiteralPath)
    if (-not $LiteralPath.StartsWith('Cert:\\LocalMachine\\')) { throw 'Unexpected real file lookup.' }
    if ($LiteralPath.EndsWith('A' * 40)) { return @{ FriendlyName = ''; Subject = 'CN=' + $marker } }
    return @{ FriendlyName = $marker; Subject = 'CN=github.com' }
}
function Remove-Item {
    [CmdletBinding()]param([string]$LiteralPath, [switch]$DeleteKey)
    if ($LiteralPath.StartsWith('Cert:\\LocalMachine\\')) {
        if ($LiteralPath.Contains('\\My\\') -and -not $DeleteKey) { throw 'Private certificate key was not removed.' }
        $script:removed.Add($LiteralPath)
    } else { Microsoft.PowerShell.Management\\Remove-Item -LiteralPath $LiteralPath }
}
function Clear-DnsClientCache { $script:dnsRefreshed = $true }
$refused = $false
try { Remove-OwnFixture $stateDirectory } catch {
    if ($_.Exception.Message -notlike '*hosts block*') { throw }
    $refused = $true
}
if (-not $refused -or $script:removed.Count -ne 3 -or -not $script:dnsRefreshed) { throw 'Cleanup stopped before attempting independent resources.' }
foreach ($name in @('github-test.pfx', 'private-config.json', 'root-public.cer')) {
    if (Test-Path -LiteralPath (Join-Path $stateDirectory $name)) { throw 'Private export remained.' }
}
`);
});
