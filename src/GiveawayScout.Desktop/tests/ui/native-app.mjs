import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import { createHash, randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { once } from 'node:events';
import { remote } from 'webdriverio';
import { artifact } from '../../scripts/windows-ui-toolchain.mjs';

export const marker = 'Giveaway Scout isolated UI tests v1\n';
export async function verifiedArtifact() {
  if(process.platform !== 'win32') throw new Error('Use Windows Node.js, not WSL');
  const info = JSON.parse(await fs.readFile(artifact+'.json','utf8'));
  const sha = createHash('sha256').update(await fs.readFile(artifact)).digest('hex');
  if(info.kind !== 'isolated-ui-tests-v1' || info.sha256 !== sha) throw new Error('Test executable identity mismatch; run pnpm build:ui:native');
  return artifact;
}

export async function waitUntil(action, label, timeout = 30_000) {
  const started = Date.now();
  while(Date.now()-started < timeout) {
    if(await action()) return;
    await delay(100);
  }
  throw new Error('Timed out: '+label);
}

export async function createProfile() {
  const root = path.join(await fs.realpath(os.tmpdir()),'giveaway-scout-e2e-'+randomUUID());
  await fs.mkdir(root);
  await fs.writeFile(path.join(root,'.giveaway-scout-ui-test'),marker);
  await setScenario(root,'baseline');
  return root;
}

export async function setScenario(root, scenario) {
  if(!['baseline','newGiveaway','sourceFailure','zerosGiveaway','zerosConnectionLimit'].includes(scenario)) throw new Error('Unknown scenario');
  const pending = path.join(root,'scenario.next.json');
  await fs.writeFile(pending,JSON.stringify(scenario));
  await fs.rename(pending,path.join(root,'scenario.json'));
}

export async function readEvents(root) {
  try { return (await fs.readFile(path.join(root,'events.jsonl'),'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse); }
  catch(error) { if(error.code === 'ENOENT') return []; throw error; }
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve,reject) => {server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

export async function launchApp(root) {
  const executable = await verifiedArtifact();
  const port = await freePort();
  const child = spawn(executable,[],{
    env:{...process.env,GIVEAWAY_SCOUT_UI_ROOT:root,TAURI_WEBDRIVER_PORT:String(port),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--force-renderer-accessibility'},
    stdio:['ignore','pipe','pipe'],windowsHide:true,
  });
  let log = '';
  child.stdout.on('data',data => {log += data;});
  child.stderr.on('data',data => {log += data;});
  let spawnError;
  child.on('error',error => {spawnError=error;});
  let browser;
  try {
    await waitUntil(async () => {
      if(spawnError) throw spawnError;
      if(child.exitCode !== null) throw new Error(`Test app exited ${child.exitCode}: ${log.slice(-5000)}`);
      try { return (await fetch(`http://127.0.0.1:${port}/status`,{signal:AbortSignal.timeout(500)})).ok; }
      catch { return false; }
    },'test app WebDriver startup',45_000);
    browser = await remote({hostname:'127.0.0.1',port,path:'/',logLevel:'error',connectionRetryCount:0,connectionRetryTimeout:15_000,waitforTimeout:15_000,
      capabilities:{browserName:'tauri','wdio:enforceWebDriverClassic':true}});
    await browser.$('.header-button').waitForEnabled();
    const identity=await browser.execute(() => window.__TAURI_INTERNALS__.invoke('get_ui_test_identity'));
    if(identity.processId!==child.pid || identity.profileId!==path.basename(root)) throw new Error('WebDriver connected to a different process or profile; refusing UI actions');
    const frontend = await browser.execute(() => ({title:document.title,app:document.querySelector('main')?.dataset.app}));
    if(frontend.title !== 'Giveaway Scout' || frontend.app !== 'giveaway-scout') throw new Error('Unexpected application frontend');
  } catch(error) {
    if(browser) await browser.deleteSession().catch(()=>{});
    if(child.exitCode === null && child.signalCode === null) {
      const exited=once(child,'exit');child.kill();await exited;
    }
    await fs.writeFile(path.join(root,'startup-failure.log'),log);
    throw error;
  }
  return {
    browser, child,
    async stop() {
      try {
        // A native window close uses the app's normal exit path. No image-name kill.
        await browser.closeWindow();
        await waitUntil(() => child.exitCode !== null,'test app clean exit',10_000);
      } finally {
        // closeWindow exits the server as well; DELETE after that is expected to
        // fail with ECONNREFUSED and would only add a misleading error to logs.
        if(child.exitCode === null && child.signalCode === null) {
          const exited = once(child,'exit');
          child.kill();
          await exited;
        }
        await fs.appendFile(path.join(root,'process.log'),log);
      }
    },
  };
}

export async function preserveProfile(root, report) {
  if(path.dirname(root) !== await fs.realpath(os.tmpdir()) || !path.basename(root).startsWith('giveaway-scout-e2e-') || await fs.readFile(path.join(root,'.giveaway-scout-ui-test'),'utf8') !== marker) throw new Error('Refusing unsafe test profile cleanup');
  // Keep synthetic DB, events and logs as evidence. Never copy a real user profile.
  await fs.cp(root,path.join(report,'profile'),{recursive:true});
  await fs.rm(root,{recursive:true});
}
