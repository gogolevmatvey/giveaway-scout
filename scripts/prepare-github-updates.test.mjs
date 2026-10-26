import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHash, generateKeyPairSync } from 'node:crypto';
import { createConfig, verifyConfig, publicKey, endpoint } from './prepare-github-updates.mjs';

// Disposable public test key. No production key is read or generated.
const rawPublic = generateKeyPairSync('ed25519').publicKey.export({format: 'jwk'}).x;
const bytes = Buffer.concat([Buffer.from('Ed'), Buffer.alloc(8, 1), Buffer.from(rawPublic, 'base64url')]);
const key = Buffer.from(`untrusted comment: minisign public key\n${bytes.toString('base64')}\n`).toString('base64');
const script = fileURLToPath(new URL('./prepare-github-updates.mjs', import.meta.url));

test('Windows overlay uses only agreed GitHub Releases, HTTPS and signed versions', () => {
  const config = createConfig(key);
  assert.equal(endpoint, 'https://github.com/gogolevmatvey/giveaway-scout/releases/latest/download/latest.json');
  assert.deepEqual(config.bundle, {targets: ['nsis'], createUpdaterArtifacts: true});
  assert.equal(verifyConfig(config), createHash('sha256').update(key).digest('hex'));
  assert.equal(publicKey(` ${key}\r\n`), key);
});

test('private, malformed, oversize, raw and filename keys are refused', () => {
  for (const value of [undefined, '', 'placeholder', 'C:/signing/updater.pub', 'x'.repeat(4097),
    Buffer.from('untrusted comment: private key\nRWQfake').toString('base64'),
    `untrusted comment: public key\n${bytes.toString('base64')}`, key.slice(0,-1),
    Buffer.from(`untrusted comment: public key\n${Buffer.alloc(42).toString('base64')}`).toString('base64')]) {
    assert.throws(() => publicKey(value));
  }
});

test('channel changes and weakened or extra configuration are refused', () => {
  for (const mutate of [
    c => c.plugins.updater.endpoints = [endpoint.replace('https:', 'http:')],
    c => c.plugins.updater.endpoints = ['https://github.com/other/repo/releases/latest/download/latest.json'],
    c => c.plugins.updater.requireSignedVersion = false,
    c => c.plugins.updater.allowDowngrades = true,
    c => c.plugins.updater.dangerousInsecureTransportProtocol = true,
    c => c.bundle.createUpdaterArtifacts = false,
    c => c.bundle.targets = ['nsis','appimage'],
    c => c.identifier = 'another.profile',
    c => c.build = {beforeBuildCommand: 'unexpected-command'},
  ]) {
    const config = createConfig(key); mutate(config);
    assert.throws(() => verifyConfig(config));
  }
});

test('CLI creates only a new local overlay, verifies it and refuses overwrite without leaking keys', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'giveaway-scout-updater-config-'));
  try {
    const input = path.join(root, 'test.pub');
    const output = path.join(root, 'release.json');
    fs.writeFileSync(input, key);
    const run = args => spawnSync(process.execPath, [script, ...args], {encoding: 'utf8'});
    const created = run(['--public-key',input,'--output',output]);
    assert.equal(created.status,0,created.stderr);
    assert.equal(created.stdout.includes(key),false);
    assert.equal(run(['--verify',output]).status,0);
    const original = fs.readFileSync(output);
    assert.notEqual(run(['--public-key',input,'--output',output]).status,0);
    assert.deepEqual(fs.readFileSync(output),original);
    assert.notEqual(run(['--verify',output,'--output',output]).status,0);
    assert.notEqual(run(['--unknown','x']).status,0);
    fs.writeFileSync(input,'PRIVATE_TEST_SECRET');
    const failed=run(['--public-key',input,'--output',path.join(root,'bad.json')]);
    assert.notEqual(failed.status,0);
    assert.equal((failed.stdout+failed.stderr).includes('PRIVATE_TEST_SECRET'),false);
    assert.equal(fs.existsSync(path.join(root,'bad.json')),false);
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
});
