// Internal CI only: allowlisted artifacts are not a signed/public release.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const targets = Object.freeze({
  'windows-x86_64': { platform:'win32', arch:'x64', rust:'x86_64-pc-windows-msvc' },
  'windows-aarch64': { platform:'win32', arch:'arm64', rust:'aarch64-pc-windows-msvc' },
  'darwin-aarch64': { platform:'darwin', arch:'arm64', rust:'aarch64-apple-darwin' },
  'linux-x86_64': { platform:'linux', arch:'x64', rust:'x86_64-unknown-linux-gnu' },
  'linux-aarch64': { platform:'linux', arch:'arm64', rust:'aarch64-unknown-linux-gnu' },
});
const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const desktop = path.join(project, 'src/GiveawayScout.Desktop');
const tauri = path.join(desktop, 'src-tauri');

export function requireHost(id, platform, arch) {
  const target = targets[id];
  assert(target, 'Unknown CI target');
  assert.equal(platform, target.platform, 'Wrong operating system');
  assert.equal(arch, target.arch, 'Native architecture required; emulation is not acceptance');
  return target;
}

export function requireRunner(id, runnerArchitecture) {
  const target = targets[id];
  assert(target, 'Unknown CI target');
  assert.equal(runnerArchitecture, target.arch === 'x64' ? 'X64' : 'ARM64',
    'GitHub runner hardware must match the native target; QEMU is not acceptance');
}

export function binaryTarget(bytes) {
  if (bytes.length >= 64 && bytes.subarray(0,4).equals(Buffer.from([0x7f,69,76,70]))) {
    assert.equal(bytes[4],2,'ELF64 required'); assert.equal(bytes[5],1,'Little endian required');
    return ({62:'linux-x86_64',183:'linux-aarch64'})[bytes.readUInt16LE(18)];
  }
  if (bytes.length >= 64 && bytes.subarray(0,2).toString() === 'MZ') {
    const offset = bytes.readUInt32LE(0x3c);
    assert(offset <= bytes.length - 6, 'Truncated PE header');
    assert.equal(bytes.subarray(offset,offset+4).toString(), 'PE\0\0','Invalid PE header');
    return ({0x8664:'windows-x86_64',0xaa64:'windows-aarch64'})[bytes.readUInt16LE(offset+4)];
  }
  if (bytes.length >= 32 && bytes.readUInt32LE(0) === 0xfeedfacf) {
    return ({0x100000c:'darwin-aarch64'})[bytes.readUInt32LE(4)];
  }
  throw new Error('Unsupported native executable format');
}

export function requireVersions(pkg, config, cargoText, toolchainText) {
  const rust = toolchainText.match(/^channel\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"$/m)?.[1];
  const cargoVersion = cargoText.match(/^version\s*=\s*"([^"]+)"$/m)?.[1];
  assert(rust, 'Pinned Rust toolchain required');
  assert.equal(pkg.version, config.version, 'Frontend and Tauri versions differ');
  assert.equal(pkg.version, cargoVersion, 'Frontend and Cargo versions differ');
  assert.match(pkg.packageManager, /^pnpm@\d+\.\d+\.\d+$/, 'Pinned pnpm required');
  return { rust, version:pkg.version, pnpm:pkg.packageManager.slice(5) };
}

function command(executable, args, cwd=tauri) {
  const result = spawnSync(executable, args, { cwd, encoding:'utf8', env:process.env });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${executable} failed: ${result.stderr}`);
  return result.stdout.trim();
}

export function verify(id) {
  const target = requireHost(id, process.platform, process.arch);
  if (process.env.GITHUB_ACTIONS === 'true') requireRunner(id, process.env.RUNNER_ARCH);
  const pins = requireVersions(JSON.parse(fs.readFileSync(path.join(desktop,'package.json'))),
    JSON.parse(fs.readFileSync(path.join(tauri,'tauri.conf.json'))),
    fs.readFileSync(path.join(tauri,'Cargo.toml'),'utf8'),
    fs.readFileSync(path.join(desktop,'rust-toolchain.toml'),'utf8'));
  assert.equal(process.versions.node, '24.21.0', 'Use the pinned Node.js LTS');
  const compiler = command('rustc',['-vV']);
  assert(compiler.split(/\r?\n/).includes(`host: ${target.rust}`), 'Rust host is not the native target');
  assert(compiler.split(/\r?\n/).includes(`release: ${pins.rust}`), 'Unexpected Rust version');
  return {...pins, target};
}

export function onlyPackage(directory, extension) {
  const found = fs.readdirSync(directory).filter(name=>name.endsWith(extension));
  assert.equal(found.length,1,`Exactly one ${extension} package required in ${directory}`);
  const file = path.join(directory,found[0]);
  assert(fs.lstatSync(file).isFile(), 'Artifact must be a regular file, not a symlink');
  return file;
}

export function collect(id) {
  const verified = verify(id);
  assert.equal(process.env.GITHUB_ACTIONS,'true','Artifact collection is CI-only');
  assert.match(process.env.GITHUB_SHA ?? '',/^[a-f0-9]{40}$/,'Exact source commit required');
  const targetDirectory = path.join(tauri,'target-ci',id);
  assert.equal(path.resolve(process.env.CARGO_TARGET_DIR ?? ''), targetDirectory, 'Unexpected Cargo artifact directory');
  const release = path.join(targetDirectory,'release');
  const executable = path.join(release,process.platform === 'win32' ? 'giveaway-scout.exe' : 'giveaway-scout');
  assert(fs.lstatSync(executable).isFile(),'Native executable must be a regular file');
  assert.equal(binaryTarget(fs.readFileSync(executable)),id,'Built binary has the wrong architecture');
  let abi = null;
  let files;
  if (process.platform === 'win32') files = [executable, onlyPackage(path.join(release,'bundle/nsis'),'.exe')];
  else if (process.platform === 'darwin') files = [onlyPackage(path.join(release,'bundle/dmg'),'.dmg')];
  else {
    const deb = onlyPackage(path.join(release,'bundle/deb'),'.deb');
    files = [deb, onlyPackage(path.join(release,'bundle/appimage'),'.AppImage')];
    const code = 'import json,runpy,sys; from pathlib import Path; m=runpy.run_path(sys.argv[1]); p=Path(sys.argv[2]); v,a=m["inspect_package"](p); g=m["inspect_abi"](p,a,m["SUITES"]["bookworm"]); print(json.dumps({"version":v,"arch":a,"glibc":g}))';
    abi = JSON.parse(command('/usr/bin/python3',['-B','-c',code,path.join(project,'scripts/prepare-apt-repository.py'),deb]));
    assert.equal(abi.version,verified.version,'DEB version mismatch');
    assert.equal(abi.arch,process.arch === 'x64' ? 'amd64' : 'arm64','DEB architecture mismatch');
  }
  const output = path.join(project,'artifacts/internal',id);
  assert(!fs.existsSync(output),'Refusing to replace an existing artifact directory');
  fs.mkdirSync(output,{recursive:true});
  const manifest = {schemaVersion:1, target:id, version:verified.version, sourceCommit:process.env.GITHUB_SHA,
    rust:verified.rust, node:process.versions.node, runnerArch:process.env.RUNNER_ARCH, osRelease:os.release(), abi,
    publicRelease:false, nativeAcceptance:false, platformSigningVerified:false, updaterSigningVerified:false, files:[]};
  for (const file of files) {
    const bytes = fs.readFileSync(file);
    const name = path.basename(file);
    // Hash and publish the same snapshot of bytes, not a second racy read.
    fs.writeFileSync(path.join(output,name),bytes,{flag:'wx'});
    manifest.files.push({name,size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')});
  }
  fs.writeFileSync(path.join(output,'candidate.json'),JSON.stringify(manifest,null,2)+'\n',{flag:'wx'});
  return output;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [mode,id,...extra] = process.argv.slice(2);
  assert.equal(extra.length,0,'Unexpected arguments');
  assert(['verify','collect'].includes(mode),'Use verify|collect and a known target');
  console.log(mode === 'verify' ? verify(id) : collect(id));
}
