// Guest-only HTTPS fixture. This does not change application endpoints or policy.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import https from 'node:https';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { parseArgs, isDeepStrictEqual } from 'node:util';
import { pathToFileURL } from 'node:url';
import { endpoint, publicKey } from './prepare-github-updates.mjs';

export const manifestPath = new URL(endpoint).pathname;
export const modes = Object.freeze(['normal', 'manifest-503', 'package-503',
  'manifest-oversize', 'package-oversize', 'invalid-signature', 'truncated-package']);
const maximumPackageBytes = 256 * 1024 * 1024;
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

function requireValue(condition, message) { if (!condition) throw new Error(message); }
function checkedHash(value) {
  requireValue(typeof value === 'string' && /^[a-f0-9]{64}$/.test(value), 'Expected a lowercase SHA256 pin.');
  return value;
}
function readRegular(filename, maximum) {
  const stat = fs.lstatSync(filename);
  requireValue(stat.isFile() && !stat.isSymbolicLink() && stat.size <= maximum, 'Expected a bounded regular file.');
  const bytes = fs.readFileSync(filename);
  requireValue(bytes.length <= maximum, 'Input exceeded its size limit.');
  return bytes;
}
function pinnedFile(filename, maximum, pin) {
  const bytes = readRegular(filename, maximum);
  requireValue(sha256(bytes) === checkedHash(pin), 'Frozen input hash mismatch.');
  return bytes;
}
function portableName(value) {
  requireValue(typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9 ._-]*$/.test(value)
    && path.basename(value) === value && !value.endsWith(' ') && !value.endsWith('.'), 'Invalid package filename.');
  return value;
}

export function assertGuestIdentity(identity, config) {
  requireValue(identity.manufacturer === 'Microsoft Corporation' && identity.model === 'Virtual Machine'
    && identity.computerName?.toLowerCase() === config.computerName?.toLowerCase()
    && identity.computerName?.toLowerCase() === os.hostname().toLowerCase()
    && /^[0-9a-f-]{36}$/i.test(config.vmUuid) && !/^(0|-)+$/.test(config.vmUuid)
    && !/^(f|-)+$/i.test(config.vmUuid) && identity.vmUuid?.toLowerCase() === config.vmUuid.toLowerCase()
    && identity.productType === 1 && Number(identity.buildNumber) >= 22000,
  'Expected Windows 11 Hyper-V guest name and UUID do not match.');
}

function verifyRunningGuest(config) {
  const command = '$s=Get-CimInstance Win32_ComputerSystem; $p=Get-CimInstance Win32_ComputerSystemProduct; '
    + '$o=Get-CimInstance Win32_OperatingSystem; @{manufacturer=$s.Manufacturer; model=$s.Model; '
    + 'computerName=$s.Name; vmUuid=$p.UUID; productType=$o.ProductType; buildNumber=$o.BuildNumber}|ConvertTo-Json -Compress';
  const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command],
    { encoding: 'utf8', windowsHide: true, timeout: 30_000, maxBuffer: 16_384 });
  requireValue(!result.error && result.status === 0, 'Cannot verify Windows VM identity.');
  assertGuestIdentity(JSON.parse(result.stdout.replace(/^\uFEFF/, '')), config);
}

// Pins come from the frozen release record, independently of the mounted input.
// Files are held in memory after verification: later changes cannot change a response.
export function loadFrozenStage(options) {
  const root = fs.realpathSync(options.inputDirectory);
  requireValue(fs.lstatSync(root).isDirectory(), 'Expected a release snapshot directory.');
  requireValue(!fs.existsSync(path.join(root, 'INCOMPLETE')), 'Incomplete release snapshot.');
  requireValue(/^\d+\.\d+\.\d+$/.test(options.expectedVersion), 'Expected a stable release version.');
  const manifestBytes = pinnedFile(path.join(root, 'latest.json'), 1024 * 1024, options.expectedManifestSha256);
  const verificationBytes = pinnedFile(path.join(root, 'verification.json'), 1024 * 1024, options.expectedVerificationSha256);
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  const report = JSON.parse(verificationBytes.toString('utf8'));
  requireValue(manifest.version === options.expectedVersion && report.version === manifest.version
    && report.schemaVersion === 1 && report.releaseProfile === 'windows-x64'
    && report.updaterSignaturesVerified === true && report.files?.length === 1,
  'Expected an already verified Windows x64 release-tools snapshot.');
  const file = report.files[0];
  const signedFilename = `Giveaway Scout_${manifest.version}_x64-setup.exe`;
  requireValue(file.target === 'windows-x86_64' && file.filename === signedFilename,
    'Unexpected signed filename or target.');
  const downloadFilename = portableName(file.downloadFilename);
  requireValue([signedFilename, signedFilename.replaceAll(' ', '.')].includes(downloadFilename), 'Unexpected hosted filename.');
  const platform = manifest.platforms?.['windows-x86_64'];
  requireValue(platform && Object.keys(manifest.platforms).length === 1, 'Expected one Windows manifest platform.');
  const expectedUrl = `https://github.com/gogolevmatvey/giveaway-scout/releases/download/v${manifest.version}/${encodeURIComponent(downloadFilename)}`;
  requireValue(platform.url === expectedUrl, 'Package URL must be the exact production GitHub release path.');
  const packages = path.join(root, 'packages');
  requireValue(fs.lstatSync(packages).isDirectory() && !fs.lstatSync(packages).isSymbolicLink(), 'Package directory symlinks are refused.');
  const packageBytes = pinnedFile(path.join(packages, downloadFilename), maximumPackageBytes, file.sha256);
  requireValue(packageBytes.length > 0 && packageBytes.length === file.size, 'Package size mismatch.');
  const signatureBytes = readRegular(path.join(packages, `${downloadFilename}.sig`), 8192);
  requireValue(signatureBytes.toString('utf8') === platform.signature && typeof platform.signature === 'string',
    'Detached signature must equal the exact frozen manifest signature.');
  const key = publicKey(readRegular(options.publicKey, 8192).toString('utf8'));
  requireValue(sha256(key) === report.updaterPublicKeySha256, 'Updater public key fingerprint mismatch.');
  return { root, manifestBytes, verificationBytes, manifest, report, packageBytes, signatureBytes,
    signedFilename, downloadFilename, key, packagePath: new URL(expectedUrl).pathname };
}

// Reuse the project's Rust verifier, including its Minisign global signature,
// signed version and signed filename checks. No signing key enters this guest.
export function verifyWithReleaseTool(stage, executable, expectedSha256) {
  pinnedFile(executable, 64 * 1024 * 1024, expectedSha256);
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'giveaway-scout-fixture-verification-'));
  try {
    fs.writeFileSync(path.join(temporary, stage.signedFilename), stage.packageBytes, { flag: 'wx' });
    fs.writeFileSync(path.join(temporary, 'package.sig'), stage.signatureBytes, { flag: 'wx' });
    fs.writeFileSync(path.join(temporary, 'updater.pub'), stage.key, { flag: 'wx' });
    const input = { releaseProfile: 'windows-x64', version: stage.manifest.version,
      previousVersion: '0.0.0', notes: stage.manifest.notes, pubDate: stage.manifest.pub_date,
      artifacts: [{ target: 'windows-x86_64', path: stage.signedFilename, signaturePath: 'package.sig',
        url: stage.manifest.platforms['windows-x86_64'].url }] };
    fs.writeFileSync(path.join(temporary, 'release.json'), JSON.stringify(input), { flag: 'wx' });
    const output = path.join(temporary, 'verified');
    const result = spawnSync(executable, ['prepare', '--input', path.join(temporary, 'release.json'),
      '--public-key', path.join(temporary, 'updater.pub'), '--output', output],
    { encoding: 'utf8', timeout: 120_000, windowsHide: true, maxBuffer: 16_384 });
    requireValue(!result.error && result.status === 0, 'Project release verifier rejected the frozen package.');
    const verified = JSON.parse(readRegular(path.join(output, 'latest.json'), 1024 * 1024));
    requireValue(isDeepStrictEqual(verified, stage.manifest), 'Verifier output differs from frozen production manifest.');
  } finally { fs.rmSync(temporary, { recursive: true, force: true }); }
}

export function createFixtureHandler(stage, { mode = 'normal', record = () => {} } = {}) {
  requireValue(modes.includes(mode), 'Unknown fixture mode.');
  return (request, response) => {
    // Compare raw paths, without URL normalization or filesystem resolution.
    const route = request.url === manifestPath ? 'manifest'
      : request.url === stage.packagePath ? 'package'
        : request.url === `${stage.packagePath}.sig` ? 'signature' : 'rejected';
    let status = 200;
    let bytes = route === 'manifest' ? stage.manifestBytes : route === 'package' ? stage.packageBytes : stage.signatureBytes;
    let declaredLength;
    if (!['github.com', 'github.com:443'].includes(request.headers.host)) status = 421;
    else if (!['GET', 'HEAD'].includes(request.method)) status = 405;
    else if (route === 'rejected') status = 404;
    else if (mode === `${route}-503`) status = 503;
    else if (mode === 'manifest-oversize' && route === 'manifest') declaredLength = 1024 * 1024 + 1;
    else if (mode === 'package-oversize' && route === 'package') declaredLength = maximumPackageBytes + 1;
    else if (mode === 'invalid-signature' && route === 'manifest') {
      const altered = structuredClone(stage.manifest);
      altered.platforms['windows-x86_64'].signature = Buffer.from('deliberately invalid test signature').toString('base64');
      bytes = Buffer.from(JSON.stringify(altered));
    } else if (mode === 'truncated-package' && route === 'package') bytes = stage.packageBytes.subarray(0, Math.max(0, stage.packageBytes.length - 1));
    if (status !== 200) bytes = Buffer.from('Fixture request refused.\n');
    record({ at: new Date().toISOString(), route, method: ['GET', 'HEAD'].includes(request.method) ? request.method : 'other',
      status, mode, bytes: request.method === 'HEAD' ? 0 : bytes.length });
    response.writeHead(status, { 'content-type': route === 'manifest' ? 'application/json' : 'application/octet-stream',
      'content-length': declaredLength ?? bytes.length, 'cache-control': 'no-store', connection: 'close' });
    response.end(request.method === 'HEAD' ? undefined : bytes);
  };
}

export async function main(args) {
  const { values } = parseArgs({ args, allowPositionals: false, options: {
    'input-directory': { type: 'string' }, 'expected-version': { type: 'string' },
    'expected-manifest-sha256': { type: 'string' }, 'expected-verification-sha256': { type: 'string' },
    'public-key': { type: 'string' }, 'release-verifier': { type: 'string' },
    'expected-verifier-sha256': { type: 'string' }, 'private-config': { type: 'string' },
    mode: { type: 'string', default: 'normal' }, 'inspect-only': { type: 'boolean' },
  } });
  const stage = loadFrozenStage({ inputDirectory: values['input-directory'], expectedVersion: values['expected-version'],
    expectedManifestSha256: values['expected-manifest-sha256'], expectedVerificationSha256: values['expected-verification-sha256'],
    publicKey: values['public-key'] });
  requireValue(modes.includes(values.mode), 'Unknown fixture mode.');
  pinnedFile(values['release-verifier'], 64 * 1024 * 1024, values['expected-verifier-sha256']);
  if (values['inspect-only']) {
    console.log(JSON.stringify({ inspected: true, cryptographicVerificationPerformed: false,
      version: stage.manifest.version, manifestSha256: sha256(stage.manifestBytes), packageSha256: sha256(stage.packageBytes) }));
    return;
  }
  requireValue(process.platform === 'win32', 'Serving is restricted to a Windows test VM.');
  const config = JSON.parse(readRegular(values['private-config'], 16_384));
  requireValue(config.schemaVersion === 1 && config.guestGuardVerified === true
    && config.computerName?.toLowerCase() === os.hostname().toLowerCase()
    && /^[0-9a-f]{32}$/i.test(config.fixtureId), 'Invalid guest fixture configuration.');
  // The initializer supplies this configuration only after checking Hyper-V CIM
  // identity and an explicit -ExecuteInTestVm. It contains no updater private key.
  verifyRunningGuest(config);
  verifyWithReleaseTool(stage, values['release-verifier'], values['expected-verifier-sha256']);
  const directory = path.dirname(path.resolve(values['private-config']));
  const log = path.join(directory, 'requests.jsonl');
  const countsPath = path.join(directory, 'counts.json');
  const counts = { manifest: 0, package: 0, signature: 0, rejected: 0 };
  const record = event => {
    counts[event.route]++;
    fs.appendFileSync(log, JSON.stringify(event) + '\n', { mode: 0o600 });
    fs.writeFileSync(countsPath + '.tmp', JSON.stringify({ mode: values.mode, counts }), { mode: 0o600 });
    fs.renameSync(countsPath + '.tmp', countsPath);
  };
  fs.writeFileSync(log, '', { flag: 'wx', mode: 0o600 });
  fs.writeFileSync(countsPath, JSON.stringify({ mode: values.mode, counts }), { flag: 'wx', mode: 0o600 });
  const server = https.createServer({ pfx: readRegular(path.join(directory, 'github-test.pfx'), 64 * 1024),
    passphrase: config.pfxPassword, minVersion: 'TLSv1.2' }, createFixtureHandler(stage, { mode: values.mode, record }));
  server.headersTimeout = 10_000;
  server.requestTimeout = 30_000;
  server.on('tlsClientError', () => {}); // Never log credentials or raw URLs.
  fs.writeFileSync(path.join(directory, 'server-process.json'), JSON.stringify({ pid: process.pid,
    executable: process.execPath, privateConfig: path.resolve(values['private-config']) }), { flag: 'wx', mode: 0o600 });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen({ host: '127.0.0.1', port: 443, exclusive: true }, resolve);
  });
  console.log(JSON.stringify({ listening: '127.0.0.1:443', version: stage.manifest.version, mode: values.mode,
    manifestSha256: sha256(stage.manifestBytes), packageSha256: sha256(stage.packageBytes) }));
  const stop = () => { server.close(); server.closeAllConnections(); };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch(() => {
    // Input files and private TLS configuration are deliberately never echoed.
    console.error('Update fixture failed. Check frozen inputs, verifier, guest identity and private TLS configuration.');
    process.exitCode = 1;
  });
}
