import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { assertGuestIdentity, loadFrozenStage, verifyWithReleaseTool, createFixtureHandler, manifestPath, modes } from './update-fixture-server.mjs';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const script = fileURLToPath(new URL('./update-fixture-server.mjs', import.meta.url));
const verifier = process.env.GIVEAWAY_SCOUT_TEST_RELEASE_VERIFIER;

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'giveaway-scout-update-fixture-test-'));
  const filename = 'Giveaway Scout_1.2.3_x64-setup.exe';
  const hosted = filename.replaceAll(' ', '.');
  const bytes = Buffer.from('Ephemeral signed fixture; this is not an executable installer.');
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const keyId = Buffer.alloc(8, 0x34);
  const publicPacket = Buffer.concat([Buffer.from('Ed'), keyId, Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url')]);
  const key = Buffer.from(`untrusted comment: ephemeral test key\n${publicPacket.toString('base64')}\n`).toString('base64');
  // A one-time test key signs the same Minisign prehash/global-comment format
  // consumed by the existing Rust verifier. No real key or installer is read.
  const dataSignature = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
  const packet = Buffer.concat([Buffer.from('ED'), keyId, dataSignature]);
  const trustedComment = `timestamp:0\tfile:${filename}\tversion:1.2.3`;
  const globalSignature = sign(null, Buffer.concat([dataSignature, Buffer.from(trustedComment)]), privateKey);
  const signature = Buffer.from(`untrusted comment: ephemeral test signature\n${packet.toString('base64')}\ntrusted comment: ${trustedComment}\n${globalSignature.toString('base64')}\n`).toString('base64');
  const manifest = { version: '1.2.3', notes: 'Local fixture', pub_date: '2020-01-01T00:00:00Z',
    platforms: { 'windows-x86_64': { url: `https://github.com/gogolevmatvey/giveaway-scout/releases/download/v1.2.3/${hosted}`, signature } } };
  const report = { schemaVersion: 1, releaseProfile: 'windows-x64', version: '1.2.3', updaterPublicKeySha256: hash(key),
    updaterSignaturesVerified: true, files: [{ target: 'windows-x86_64', filename, downloadFilename: hosted,
      size: bytes.length, sha256: hash(bytes) }] };
  fs.mkdirSync(path.join(root, 'packages'));
  fs.writeFileSync(path.join(root, 'packages', hosted), bytes);
  fs.writeFileSync(path.join(root, 'packages', hosted + '.sig'), signature);
  fs.writeFileSync(path.join(root, 'updater.pub'), key);
  const save = () => {
    fs.writeFileSync(path.join(root, 'latest.json'), JSON.stringify(manifest, null, 2));
    fs.writeFileSync(path.join(root, 'verification.json'), JSON.stringify(report, null, 2));
    return { inputDirectory: root, expectedVersion: '1.2.3',
      expectedManifestSha256: hash(fs.readFileSync(path.join(root, 'latest.json'))),
      expectedVerificationSha256: hash(fs.readFileSync(path.join(root, 'verification.json'))), publicKey: path.join(root, 'updater.pub') };
  };
  return { root, filename, hosted, bytes, manifest, report, save, options: save(),
    clean: () => fs.rmSync(root, { recursive: true, force: true }) };
}

function request(stage, url, { mode = 'normal', method = 'GET', host = 'github.com' } = {}) {
  let status, headers, body;
  const events = [];
  createFixtureHandler(stage, { mode, record: event => events.push(event) })(
    { url, method, headers: { host } },
    { writeHead: (s, h) => { status = s; headers = h; }, end: bytes => { body = bytes; } });
  return { status, headers, body, events };
}

test('normal fixture preserves the exact frozen production bytes and HEAD size', () => {
  const f = fixture();
  try {
    const stage = loadFrozenStage(f.options);
    for (const [url, bytes] of [[manifestPath, stage.manifestBytes], [stage.packagePath, f.bytes],
      [stage.packagePath + '.sig', stage.signatureBytes]]) {
      const result = request(stage, url);
      assert.equal(result.status, 200);
      assert.deepEqual(result.body, bytes);
      assert.equal(result.headers['content-length'], bytes.length);
      assert.equal(result.headers['cache-control'], 'no-store');
      assert.equal(result.events.length, 1);
      assert.equal(request(stage, url, { method: 'HEAD' }).body, undefined);
    }
    fs.writeFileSync(path.join(f.root, 'packages', f.hosted), 'changed after loading');
    assert.deepEqual(request(stage, stage.packagePath).body, f.bytes);
    assert.throws(() => loadFrozenStage(f.options), /hash mismatch/);
  } finally { f.clean(); }
});

test('only exact allowlisted paths, GitHub host and GET/HEAD are served; logs omit raw URLs', () => {
  const f = fixture();
  try {
    const stage = loadFrozenStage(f.options);
    for (const url of ['/', '/private-config.json', '/../../private-config.json', '/%2e%2e/latest.json',
      `${manifestPath}?access_token=TOP_SECRET`, `${stage.packagePath}/..`, `https://github.com${manifestPath}`]) {
      const result = request(stage, url);
      assert.equal(result.status, 404);
      assert.equal(JSON.stringify(result.events).includes('TOP_SECRET'), false);
      assert.equal(result.events[0].route, 'rejected');
    }
    assert.equal(request(stage, manifestPath, { host: 'other.example' }).status, 421);
    assert.equal(request(stage, manifestPath, { method: 'POST' }).status, 405);
    assert.equal(request(stage, manifestPath, { host: 'github.com:443' }).status, 200);
  } finally { f.clean(); }
});

test('explicit negative modes are deterministic and normal mode remains frozen', () => {
  const f = fixture();
  try {
    const stage = loadFrozenStage(f.options);
    assert.equal(request(stage, manifestPath, { mode: 'manifest-503' }).status, 503);
    assert.equal(request(stage, stage.packagePath, { mode: 'package-503' }).status, 503);
    assert.equal(request(stage, manifestPath, { mode: 'manifest-oversize' }).headers['content-length'], 1024 * 1024 + 1);
    assert.equal(request(stage, stage.packagePath, { mode: 'package-oversize' }).headers['content-length'], 256 * 1024 * 1024 + 1);
    assert.notEqual(JSON.parse(request(stage, manifestPath, { mode: 'invalid-signature' }).body).platforms['windows-x86_64'].signature,
      stage.manifest.platforms['windows-x86_64'].signature);
    assert.equal(request(stage, stage.packagePath, { mode: 'truncated-package' }).body.length, f.bytes.length - 1);
    assert.deepEqual(request(stage, manifestPath).body, stage.manifestBytes);
    assert.throws(() => createFixtureHandler(stage, { mode: 'unknown' }), /Unknown fixture mode/);
    assert.equal(modes.length, 7);
  } finally { f.clean(); }
});

test('hash pins, version, filenames, public key and exact production URL are mandatory', () => {
  const f = fixture();
  try {
    assert.throws(() => loadFrozenStage({ ...f.options, expectedVersion: '1.2.4' }));
    assert.throws(() => loadFrozenStage({ ...f.options, expectedManifestSha256: 'a'.repeat(64) }));
    assert.throws(() => loadFrozenStage({ ...f.options, expectedVerificationSha256: 'A'.repeat(64) }));
    const original = structuredClone(f.manifest);
    for (const url of [original.platforms['windows-x86_64'].url.replace('https:', 'http:'),
      original.platforms['windows-x86_64'].url + '?token=secret',
      original.platforms['windows-x86_64'].url.replace('github.com', 'example.invalid'),
      original.platforms['windows-x86_64'].url.replace('/v1.2.3/', '/v1.2.4/')]) {
      f.manifest.platforms['windows-x86_64'].url = url;
      assert.throws(() => loadFrozenStage(f.save()), /exact production/);
    }
    Object.assign(f.manifest, original);
    f.report.files[0].downloadFilename = '../outside.exe';
    assert.throws(() => loadFrozenStage(f.save()), /filename/);
    f.report.files[0].downloadFilename = f.hosted;
    f.report.updaterPublicKeySha256 = '0'.repeat(64);
    assert.throws(() => loadFrozenStage(f.save()), /fingerprint/);
    fs.writeFileSync(path.join(f.root, 'INCOMPLETE'), 'incomplete');
    assert.throws(() => loadFrozenStage(f.save()), /Incomplete/);
  } finally { f.clean(); }
});

test('VM identity guard rejects physical hosts, wrong guest name/UUID and pre-Windows-11 guests', () => {
  const config = { computerName: os.hostname(), vmUuid: '12345678-abcd-1234-abcd-123456789abc' };
  const identity = { manufacturer: 'Microsoft Corporation', model: 'Virtual Machine', computerName: os.hostname(),
    vmUuid: config.vmUuid, productType: 1, buildNumber: '26200' };
  assert.doesNotThrow(() => assertGuestIdentity(identity, config));
  for (const change of [{ model: 'Surface' }, { manufacturer: 'Other' }, { computerName: 'OTHER-GUEST' },
    { vmUuid: '12345678-abcd-1234-abcd-123456789abd' }, { productType: 3 }, { buildNumber: '19045' }]) {
    assert.throws(() => assertGuestIdentity({ ...identity, ...change }, config), /guest name and UUID/);
  }
  assert.throws(() => assertGuestIdentity(identity, { ...config, vmUuid: '00000000-0000-0000-0000-000000000000' }));
});

test('inspection is read-only and does not start a listener or create TLS files', () => {
  const f = fixture();
  try {
    const before = fs.readdirSync(f.root).sort();
    const run = spawnSync(process.execPath, [script, '--input-directory', f.root, '--expected-version', '1.2.3',
      '--expected-manifest-sha256', f.options.expectedManifestSha256, '--expected-verification-sha256', f.options.expectedVerificationSha256,
      '--public-key', f.options.publicKey, '--release-verifier', f.options.publicKey,
      '--expected-verifier-sha256', hash(fs.readFileSync(f.options.publicKey)), '--inspect-only'], { encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(fs.readdirSync(f.root).sort(), before);
    assert.equal(JSON.parse(run.stdout).cryptographicVerificationPerformed, false);
    assert.equal(run.stdout.includes('PRIVATE'), false);
  } finally { f.clean(); }
});

test('existing project verifier authenticates package and signed version/filename; altered bytes are refused',
  { skip: !verifier && 'Set GIVEAWAY_SCOUT_TEST_RELEASE_VERIFIER to an existing checked verifier; no Rust build is performed.' }, () => {
    const f = fixture();
    try {
      const stage = loadFrozenStage(f.options);
      const pin = hash(fs.readFileSync(verifier));
      assert.doesNotThrow(() => verifyWithReleaseTool(stage, verifier, pin));
      assert.throws(() => verifyWithReleaseTool({ ...stage, packageBytes: Buffer.from('altered') }, verifier, pin), /rejected/);
      assert.throws(() => verifyWithReleaseTool({ ...stage, manifest: { ...stage.manifest, version: '1.2.4' } }, verifier, pin), /rejected/);
      assert.throws(() => verifyWithReleaseTool({ ...stage, signedFilename: 'Giveaway Scout_1.2.4_x64-setup.exe' }, verifier, pin), /rejected/);
      assert.throws(() => verifyWithReleaseTool(stage, verifier, '0'.repeat(64)), /hash mismatch/);
    } finally { f.clean(); }
  });
