// Builds a bundle-only overlay. Plain cargo test/build never needs the CLI payload.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { binaryTarget } from './ci-artifacts.mjs';

const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tauri = path.join(repository, 'src/GiveawayScout.Desktop/src-tauri');

export function windowsComponentsOverlay(cliExecutable) {
  assert(path.isAbsolute(cliExecutable), 'CLI payload must use an absolute path');
  return {
    bundle: {
      resources: { [cliExecutable]: 'giveaway-scout-cli.exe' },
      windows: { nsis: { template: 'installer/components.nsi' } },
    },
  };
}

export function prepareWindowsComponents(targetDirectory = process.env.CARGO_TARGET_DIR) {
  assert.equal(process.platform, 'win32', 'Windows component packaging requires Windows');
  assert.equal(process.arch, 'x64', 'Component setup is supported on Windows x64');
  assert(targetDirectory, 'CARGO_TARGET_DIR is required');
  const directory = path.resolve(targetDirectory);
  const relative = path.relative(tauri, directory);
  assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative),
    'Bundle overlay must stay in a project-local Cargo target directory');
  const executable = path.join(directory, 'release/giveaway-scout-cli.exe');
  assert(fs.lstatSync(executable).isFile(), 'Build the release CLI before packaging');
  assert.equal(binaryTarget(fs.readFileSync(executable)), 'windows-x86_64');
  const version = JSON.parse(fs.readFileSync(path.join(tauri, 'tauri.conf.json'), 'utf8')).version;
  assert.equal(execFileSync(executable, ['--version'], { encoding: 'utf8', windowsHide: true }).trim(),
    `giveaway-scout-cli ${version}`, 'CLI and setup versions differ');
  const config = path.join(directory, 'components-bundle.conf.json');
  fs.writeFileSync(config, JSON.stringify(windowsComponentsOverlay(executable), null, 2) + '\n');
  return config;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv.length, 2, 'No arguments expected; set CARGO_TARGET_DIR');
  console.log(prepareWindowsComponents());
}
