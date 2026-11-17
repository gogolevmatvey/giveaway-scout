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
  if(!['baseline','newGiveaway','sourceFailure','gamesForFarmFailure','gamesForFarmEmpty','gamesForFarmInvalidFeed','zerosGiveaway','zerosConnectionLimit','orderFirst','orderSecond','freeSteamKeysGiveaway','freeSteamKeysFailure','crossSourceGiveaway','slowGivee','largeHistory','largeHistoryNew'].includes(scenario)) throw new Error('Unknown scenario');
  const pending = path.join(root,'scenario.next.json');
  await fs.writeFile(pending,JSON.stringify(scenario));
  await fs.rename(pending,path.join(root,'scenario.json'));
}

export async function readEvents(root) {
  try { return (await fs.readFile(path.join(root,'events.jsonl'),'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse); }
  catch(error) { if(error.code === 'ENOENT') return []; throw error; }
}

export async function armAttentionRace(root) {
  const token = randomUUID();
  const pending = path.join(root,'attention-race-arm.next.json');
  await fs.writeFile(pending,JSON.stringify(token));
  await fs.rename(pending,path.join(root,'attention-race-arm.json'));
  return token;
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve,reject) => {server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

export async function connectWhenWindowReady(connect, assertAlive, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    assertAlive();
    const remaining = Math.max(1, deadline - Date.now());
    try { return await connect(remaining); }
    catch(error) {
      // The plugin starts /status before Tauri creates the WebView window.
      // Only its explicit missing-window response is a startup readiness retry.
      const knownWindowRace = /^(?:WebDriverError:\s*)?No window could be found\b/i.test(error?.message ?? '') ||
        /^no such window\b/i.test(error?.message ?? '') || error?.error === 'no such window';
      if (!knownWindowRace) throw error;
      assertAlive();
      if (Date.now() >= deadline) throw error;
      await delay(Math.min(150, deadline - Date.now()));
    }
  }
}

export async function launchApp(root) {
  const executable = await verifiedArtifact();
  const port = await freePort();
  const startedAt = Date.now();
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
  let sessionAttempts = 0;
  try {
    await waitUntil(async () => {
      if(spawnError) throw spawnError;
      if(child.exitCode !== null) throw new Error(`Test app exited ${child.exitCode}: ${log.slice(-5000)}`);
      try { return (await fetch(`http://127.0.0.1:${port}/status`,{signal:AbortSignal.timeout(500)})).ok; }
      catch { return false; }
    },'test app WebDriver startup',45_000);
    browser = await connectWhenWindowReady(remaining => {
      sessionAttempts++;
      return remote({hostname:'127.0.0.1',port,path:'/',logLevel:'error',connectionRetryCount:0,connectionRetryTimeout:remaining,waitforTimeout:15_000,
        capabilities:{browserName:'tauri','wdio:enforceWebDriverClassic':true}});
    }, () => {
      if(spawnError) throw spawnError;
      if(child.exitCode !== null || child.signalCode !== null) throw new Error(`Test app exited before window creation: ${log.slice(-5000)}`);
    }, 45_000);
    await browser.$('.header-button').waitForEnabled();
    const identity=await browser.execute(() => window.__TAURI_INTERNALS__.invoke('get_ui_test_identity'));
    if(identity.processId!==child.pid || identity.profileId!==path.basename(root)) throw new Error('WebDriver connected to a different process or profile; refusing UI actions');
    const frontend = await browser.execute(() => ({title:document.title,app:document.querySelector('main')?.dataset.app}));
    if(frontend.title !== 'Giveaway Scout' || frontend.app !== 'giveaway-scout') throw new Error('Unexpected application frontend');
  } catch(error) {
    const profileEntries = {};
    for (const directory of ['data','logs','webview']) {
      profileEntries[directory] = await Promise.all((await fs.readdir(path.join(root,directory)).catch(()=>[])).slice(0,25).map(async name=>{
        const stat=await fs.stat(path.join(root,directory,name)).catch(()=>null);
        return {name,sizeBytes:stat?.size,modifiedAtUtc:stat?.mtime.toISOString(),isDirectory:stat?.isDirectory()};
      }));
    }
    await fs.writeFile(path.join(root,'startup-failure.json'),JSON.stringify({
      processId:child.pid,port,elapsedMs:Date.now()-startedAt,sessionAttempts,
      processAlive:child.exitCode===null&&child.signalCode===null,exitCode:child.exitCode,signalCode:child.signalCode,
      errorName:error?.name,errorMessage:error?.message,profileEntries,
    },null,2)+'\n');
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
