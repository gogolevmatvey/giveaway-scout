import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { binaryTarget, onlyPackage, requireHost, requireRunner, requireVersions, targets } from './ci-artifacts.mjs';

test('exactly the five planned native targets are supported',()=>{
  assert.equal(Object.keys(targets).length,5);
  for (const [id,target] of Object.entries(targets)) assert.equal(requireHost(id,target.platform,target.arch),target);
});
test('emulation, wrong OS and unknown targets are rejected',()=>{
  assert.throws(()=>requireHost('windows-aarch64','win32','x64'));
  assert.throws(()=>requireHost('darwin-aarch64','linux','arm64'));
  assert.throws(()=>requireHost('../anything','linux','x64'));
});
test('GitHub runner hardware must match even if an emulated process reports ARM64',()=>{
  requireRunner('linux-aarch64','ARM64');
  requireRunner('windows-x86_64','X64');
  assert.throws(()=>requireRunner('linux-aarch64','X64'));
  assert.throws(()=>requireRunner('darwin-aarch64',undefined));
});
test('native ELF architecture is read from bytes',()=>{
  for (const [machine,id] of [[62,'linux-x86_64'],[183,'linux-aarch64']]) {
    const bytes=Buffer.alloc(64); bytes.set([0x7f,69,76,70,2,1]); bytes.writeUInt16LE(machine,18);
    assert.equal(binaryTarget(bytes),id);
  }
});
test('native Windows architecture is read from PE, not the filename',()=>{
  for (const [machine,id] of [[0x8664,'windows-x86_64'],[0xaa64,'windows-aarch64']]) {
    const bytes=Buffer.alloc(128); bytes.write('MZ'); bytes.writeUInt32LE(80,0x3c); bytes.write('PE\0\0',80); bytes.writeUInt16LE(machine,84);
    assert.equal(binaryTarget(bytes),id);
  }
});
test('macOS requires ARM64 Mach-O; malformed binaries do not pass',()=>{
  const bytes=Buffer.alloc(64); bytes.writeUInt32LE(0xfeedfacf,0); bytes.writeUInt32LE(0x100000c,4);
  assert.equal(binaryTarget(bytes),'darwin-aarch64');
  assert.throws(()=>binaryTarget(Buffer.from('not an executable')));
  const truncated=Buffer.alloc(64); truncated.write('MZ'); truncated.writeUInt32LE(10000,0x3c);
  assert.throws(()=>binaryTarget(truncated));
});
test('app/tool versions must be pinned and consistent',()=>{
  const pkg={version:'0.1.0',packageManager:'pnpm@12.10.1'};
  const cargo='version = "0.1.0"\n'; const toolchain='channel = "1.99.1"\n';
  assert.equal(requireVersions(pkg,{version:'0.1.0'},cargo,toolchain).rust,'1.99.1');
  assert.throws(()=>requireVersions(pkg,{version:'0.2.0'},cargo,toolchain));
  assert.throws(()=>requireVersions({...pkg,packageManager:'pnpm@latest'},{version:'0.1.0'},cargo,toolchain));
});
test('artifact allowlist rejects absent and ambiguous package outputs',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'giveaway-scout-ci-test-'));
  try {
    assert.throws(()=>onlyPackage(root,'.deb'));
    fs.writeFileSync(path.join(root,'one.deb'),'fixture');
    assert.equal(onlyPackage(root,'.deb'),path.join(root,'one.deb'));
    fs.writeFileSync(path.join(root,'two.deb'),'fixture');
    assert.throws(()=>onlyPackage(root,'.deb'));
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
});

test('source ignore rules exclude local data and signing secrets but keep public sources and keys',()=>{
  const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'giveaway-scout-ignore-test-'));
  const project=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
  try {
    const gitDirectory=path.join(temporary,'test.git');
    const globalConfig=path.join(temporary,'empty-gitconfig');
    fs.writeFileSync(globalConfig,'');
    const env={...process.env,GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_GLOBAL:globalConfig};
    const initialized=spawnSync('git',['init','--bare','--quiet',gitDirectory],{env,encoding:'utf8'});
    assert.equal(initialized.status,0,initialized.stderr);
    const ignored=[
      'dist/win-x64/GiveawayScout.exe','release-staging/latest.json',
      'artifacts/internal/windows-x86_64/candidate.json',
      'private/profile/settings.json','giveaway-scout.db','giveaway-scout.db-wal',
      'giveaway-scout.db.corrupt-test.backup','profile.sqlite3',
      'Giveaway Scout.log','.env','.env.production','updater.key',
      'updater.key.enc','windows.pfx','apple.p12','signing.pem',
      'src/GiveawayScout.Desktop/node_modules/example/package.json',
      'src/GiveawayScout.Desktop/src-tauri/target-windows-x64-next/release/giveaway-scout.exe',
    ];
    const included=[
      'README.md','Plan.md','.env.example','updater.pub',
      'src/GiveawayScout.Desktop/src/App.tsx',
      'src/GiveawayScout.Desktop/src-tauri/Cargo.lock',
      'src/GiveawayScout.Desktop/src-tauri/src/lib.rs',
      'src/GiveawayScout.Desktop/src-tauri/linux/com.giveawayscout.desktop.service',
      'src/GiveawayScout.Desktop/src-tauri/crates/giveaway-scout-notifications/src/lib.rs',
    ];
    for(const file of [...ignored,...included]) {
      const result=spawnSync('git',[
        `--git-dir=${gitDirectory}`,`--work-tree=${project}`,
        'check-ignore','--no-index','--quiet','--',file,
      ],{cwd:project,env,encoding:'utf8'});
      assert.equal(result.status,ignored.includes(file)?0:1,`${file}: ${result.stderr}`);
    }
  } finally { fs.rmSync(temporary,{recursive:true,force:true}); }
});
