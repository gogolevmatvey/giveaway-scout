import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { artifact, desktop, tauri, windowsToolchain, run } from './windows-ui-toolchain.mjs';

const {environment, cargo} = windowsToolchain();
const mode = process.argv[2];
if (mode === '--prepare') {
  run(cargo, ['fetch'], tauri, environment);
} else if (mode === '--check') {
  const productionTree=spawnSync(cargo,['tree','--locked','--edges','normal','--prefix','none','-p','giveaway-scout'],{cwd:tauri,env:environment,encoding:'utf8'});
  if(productionTree.error || productionTree.status!==0 || productionTree.stdout.includes('tauri-plugin-wdio-webdriver')) throw new Error('Production dependency tree must exclude the test WebDriver');
  console.log('PASS: default production dependency tree excludes the test WebDriver');
  run(cargo, ['fmt','--all','--','--check'], tauri, environment);
  run(cargo, ['test','--workspace','--locked'], tauri, environment);
  run(cargo, ['clippy','--workspace','--all-targets','--locked','--','-D','warnings'], tauri, environment);
  run(cargo, ['test','--lib','--features','ui-tests','--locked'], tauri, environment);
  run(cargo, ['clippy','--all-targets','--features','ui-tests','--locked','--','-D','warnings'], tauri, environment);
} else if (!mode) {
  const built = path.join(environment.CARGO_TARGET_DIR,'debug/giveaway-scout.exe');
  for (const candidate of [built, artifact]) {
    try { fs.closeSync(fs.openSync(candidate,'r+')); }
    catch (error) { if (error.code !== 'ENOENT') throw new Error(`Cannot replace a running/locked test EXE: ${candidate}`,{cause:error}); }
  }
  run(process.execPath, ['node_modules/typescript/bin/tsc','--noEmit'], desktop, environment);
  run(process.execPath, ['node_modules/typescript/bin/tsc','--noEmit','-p','tsconfig.node.json'], desktop, environment);
  run(process.execPath, ['node_modules/vite/bin/vite.js','build'], desktop, environment);
  run(process.execPath, ['node_modules/@tauri-apps/cli/tauri.js','build','--debug','--no-bundle','--features','ui-tests','--config','src-tauri/tauri.ui-tests.conf.json','--','--locked'], desktop, environment);
  fs.mkdirSync(path.dirname(artifact),{recursive:true});
  fs.copyFileSync(built,artifact);
  fs.writeFileSync(artifact+'.json',JSON.stringify({kind:'isolated-ui-tests-v1',sha256:createHash('sha256').update(fs.readFileSync(artifact)).digest('hex')},null,2)+'\n');
  console.log('Test-only executable prepared:',artifact);
} else {
  throw new Error('Usage: build-native-ui.mjs [--prepare | --check]');
}
