// Headless UI contract only: fake Tauri, no update server, installer or user data.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const root = path.join(project, 'src/GiveawayScout.Desktop/dist');
const bridge = `
window.__smoke = { confirmed:false, calls:[], language:'ru', update:{mode:'enabled',phase:'available',currentVersion:'0.1.0',candidate:{token:'test-candidate',version:'0.2.0',notes:'<img src=x onerror="window.__noteExecuted=true">'},downloadedBytes:0,totalBytes:null,lastCheckedAtUtc:null,lastError:null} };
window.confirm = message => { window.__smoke.calls.push({command:'confirm',message}); return window.__smoke.confirmed; };
window.__TAURI_INTERNALS__ = { transformCallback:()=>1, unregisterCallback:()=>{}, invoke:async (command,args)=>{
 const state=window.__smoke;
 if(command==='get_update_state') return structuredClone(state.update);
 if(command==='get_app_state') return {appName:'Giveaway Scout',version:'0.1.0',platform:'windows',architecture:'x86_64',isDevelopment:false};
 if(command==='get_notification_history_page') return {items:[],nextCursor:null};
 if(command==='get_logging_state') return {requestedAtStartup:{maxFileSizeMb:2,fileCount:1,directory:null},effectiveDirectory:'C:/Fixture logs',activeFilePath:'C:/Fixture logs/Giveaway Scout.log',maxFileSizeMb:2,fileCount:1,restartRequired:false,lastError:null,usingFallback:false};
 if(command==='get_storage_snapshot') return {settings:{checkIntervalMinutes:60,sourceCheckIntervals:{freeSteamKeys:60,giveeClub:60,gamesForFarm:60,zeros:60},httpTimeoutSeconds:30,httpRetryCount:2,giveeClubEnabled:true,gamesForFarmEnabled:true,zerosEnabled:false,freeSteamKeysEnabled:true,steamNotificationsEnabled:true,epicGamesNotificationsEnabled:true,gogNotificationsEnabled:true,notificationSoundEnabled:true,startAtLogin:false,startInTray:true,keepRunningInTray:false,languageCode:state.language,verboseLogging:false,logMaxFileSizeMb:2,logFileCount:1,logDirectory:null,initialSyncMode:'silent'},counts:{trackedGiveaways:1,notificationHistory:0,unreadNotifications:0},dataSizeBytes:8192,databaseRecovery:null,initialSyncChoiceRequired:false,monitor:{isChecking:false,lastStartedAtUtc:null,lastCompletedAtUtc:null,lastSuccessfulCheckUtc:null,lastError:null,lastNewItems:0,lastFailureCount:0,sourceFailures:[]}};
 if(command==='install_update') { state.calls.push({command,args}); state.update.phase='downloading'; state.update.downloadedBytes=1024; return new Promise(()=>{}); }
 if(command==='check_for_updates') { state.calls.push({command}); return structuredClone(state.update); }
 if(command==='plugin:event|listen') return 1;
 if(command==='plugin:event|unlisten') return;
 throw new Error('Unexpected smoke command: '+command);
} };
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:()=>{}};
`;

const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, 'http://localhost').pathname;
    if (pathname === '/bridge.js') { response.setHeader('Content-Type', 'text/javascript'); response.end(bridge); return; }
    const filename = path.resolve(root, '.' + (pathname === '/' ? '/index.html' : pathname));
    if (!filename.startsWith(root + path.sep)) { response.writeHead(403).end(); return; }
    let data = await readFile(filename);
    const extension = path.extname(filename);
    response.setHeader('Content-Type', ({'.html':'text/html','.js':'text/javascript','.css':'text/css'})[extension] ?? 'application/octet-stream');
    if (extension === '.html') data = data.toString().replace('<head>', '<head><script src="/bridge.js"></script>');
    response.end(data);
  } catch { response.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const profile = await mkdtemp(path.join(tmpdir(), 'giveaway-scout-update-ui-'));
const executable = process.env.GIVEAWAY_SCOUT_CHROMIUM ?? '/snap/chromium/current/usr/lib/chromium-browser/chrome';
const browser = spawn(executable, ['--headless','--no-sandbox','--disable-dev-shm-usage','--disable-gpu','--no-first-run','--remote-debugging-port=0',`--user-data-dir=${profile}`,'about:blank'], {stdio:['ignore','ignore','pipe']});
let browserClosed = false;
const browserClosedPromise = new Promise(resolve => browser.once('close', () => { browserClosed = true; resolve(); }));
let websocket;
try {
  const endpoint = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Chromium startup timed out')), 20000);
    browser.on('error', error => { clearTimeout(timeout); reject(error); });
    browser.on('exit', code => { clearTimeout(timeout); reject(new Error('Chromium exit ' + code)); });
    browser.stderr.on('data', chunk => { const match = chunk.toString().match(/DevTools listening on (ws:\/\/[^\s]+)/); if (match) { clearTimeout(timeout); resolve(match[1]); } });
  });
  const targets = await (await fetch(endpoint.replace('ws:', 'http:').replace(/\/devtools\/browser\/.+$/, '/json/list'))).json();
  websocket = new WebSocket(targets.find(target => target.type === 'page').webSocketDebuggerUrl);
  await new Promise(resolve => websocket.addEventListener('open', resolve, {once:true}));
  let identifier = 0;
  const pending = new Map();
  websocket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (!message.id) return;
    const entry = pending.get(message.id); pending.delete(message.id);
    if (message.error) entry.reject(message.error); else entry.resolve(message.result);
  });
  const command = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++identifier; pending.set(id, {resolve,reject}); websocket.send(JSON.stringify({id,method,params}));
  });
  const evaluate = async expression => {
    const result = await command('Runtime.evaluate', {expression,returnByValue:true,awaitPromise:true});
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const waitFor = async (expression, label) => {
    for (let attempt=0; attempt<100; attempt++) { if (await evaluate(expression)) return; await delay(100); }
    throw new Error('Timed out: ' + label);
  };
  await command('Page.navigate', {url:`http://127.0.0.1:${server.address().port}/`});
  await waitFor(`!!document.querySelector('.update-banner')`, 'update banner');
  assert.equal(await evaluate(`document.querySelectorAll('[role="dialog"]').length`), 0);
  assert.match(await evaluate(`document.querySelector('.update-banner').textContent`), /Доступно обновление 0\.2\.0/);
  assert.equal(await evaluate(`document.querySelector('.update-banner button').textContent.trim()`), 'Обновить');
  for (const [width,height] of [[950,600],[1100,900]]) {
    await command('Emulation.setDeviceMetricsOverride', {width,height,deviceScaleFactor:1,mobile:false});
    const layout = await evaluate(`(() => {
      const status=document.querySelector('.status-panel').getBoundingClientRect();
      const banner=document.querySelector('.update-banner').getBoundingClientRect();
      const content=document.querySelector('.content-grid').getBoundingClientRect();
      const sources=document.querySelector('.sources-card').getBoundingClientRect();
      const actions=document.querySelector('.source-actions').getBoundingClientRect();
      return {statusBottom:status.bottom,bannerTop:banner.top,bannerBottom:banner.bottom,contentTop:content.top,
        contentBottom:content.bottom,sourcesBottom:sources.bottom,actionsBottom:actions.bottom,
        sourceCount:document.querySelectorAll('.source-row').length,
        scrolls:document.documentElement.scrollHeight>innerHeight};
    })()`);
    assert.ok(layout.statusBottom <= layout.bannerTop, JSON.stringify(layout));
    assert.ok(layout.bannerBottom <= layout.contentTop, JSON.stringify(layout));
    assert.equal(layout.sourceCount, 4, JSON.stringify(layout));
    assert.ok(layout.actionsBottom < layout.sourcesBottom, JSON.stringify(layout));
    assert.ok(layout.sourcesBottom <= layout.contentBottom + 1, JSON.stringify(layout));
    assert.equal(layout.scrolls, false, JSON.stringify(layout));
  }
  await evaluate(`document.querySelector('.update-banner button').click()`);
  assert.equal(await evaluate(`window.__smoke.calls.filter(call=>call.command==='confirm').length`), 1);
  assert.equal(await evaluate(`window.__smoke.calls.filter(call=>call.command==='install_update').length`), 0);
  await evaluate(`document.querySelector('.header-button').click()`);
  await waitFor(`!!document.querySelector('#settings-tab-updatesData')`, 'settings navigation');
  await evaluate(`document.querySelector('#settings-tab-updatesData').click()`);
  await waitFor(`!!document.querySelector('.app-updates .update-candidate .primary-button')?.offsetParent`, 'update settings from banner');
  assert.equal(await evaluate(`window.__smoke.calls.filter(call=>call.command==='install_update').length`), 0);
  assert.equal(await evaluate(`document.querySelector('.update-notes').childElementCount`), 0);
  assert.equal(await evaluate(`window.__noteExecuted===true`), false);
  await evaluate(`document.querySelector('.update-candidate .primary-button').click()`);
  assert.equal(await evaluate(`window.__smoke.calls.filter(call=>call.command==='confirm').length`), 2);
  assert.equal(await evaluate(`window.__smoke.calls.filter(call=>call.command==='install_update').length`), 0);
  await evaluate(`window.__smoke.confirmed=true; document.querySelector('.update-candidate .primary-button').click()`);
  await waitFor(`window.__smoke.calls.some(call=>call.command==='install_update')`, 'confirmed install');
  assert.deepEqual(await evaluate(`window.__smoke.calls.find(call=>call.command==='install_update').args`), {token:'test-candidate',confirmed:true});
  await waitFor(`document.querySelector('.danger-button').disabled && document.querySelector('.source-actions .primary-button').disabled`, 'data operations blocked');
  assert.equal(await evaluate(`document.querySelector('.update-candidate .primary-button').disabled`), true);
  assert.equal(await evaluate(`document.querySelector('.update-banner button').disabled`), true);
  await waitFor(`document.querySelector('.update-banner').textContent.includes('Скачивание:')`, 'banner download progress');
  await command('Page.reload');
  await waitFor(`!!document.querySelector('.update-banner')`, 'reload without real installation');
  await evaluate(`window.__smoke.language='en'; window.__smoke.update.mode='packageManager'`);
  await waitFor(`document.querySelector('.header-button').textContent.includes('Settings')`, 'English UI');
  await evaluate(`document.querySelector('.header-button').click()`);
  await waitFor(`!!document.querySelector('#settings-tab-updatesData')`, 'settings navigation');
  await evaluate(`document.querySelector('#settings-tab-updatesData').click()`);
  await waitFor(`!!document.querySelector('.app-updates')?.offsetParent`, 'update settings section');
  await waitFor(`document.querySelector('.app-updates').textContent.includes('system package manager')`, 'APT message');
  assert.equal(await evaluate(`document.querySelectorAll('.app-updates button').length`), 0);
  assert.equal(await evaluate(`document.querySelectorAll('.update-banner').length`), 0);
  await evaluate(`window.__smoke.update.mode='notConfigured'`);
  await waitFor(`document.querySelector('.app-updates').textContent.includes('not configured')`, 'unconfigured signing message');
  assert.equal(await evaluate(`document.querySelectorAll('.app-updates button').length`), 0);
  console.log('PASS: prominent banner below status at 950x600 and 1100x900, no startup dialog, direct update action, plain release notes, cancel/confirm token, progress and data lock UI, English APT and unconfigured mode. Mock Tauri only; no real updater, browser URLs or user database used.');
} finally {
  websocket?.close();
  // A failed spawn has no PID and never emits 'exit'. Do not wait forever in
  // cleanup when Chromium is absent; kill only this script's own child process.
  if (!browserClosed && browser.pid) {
    browser.kill('SIGTERM');
    await Promise.race([browserClosedPromise, delay(5000, undefined, {ref:false})]);
    if (!browserClosed) {
      browser.kill('SIGKILL');
      await browserClosedPromise;
    }
  }
  await new Promise(resolve => server.close(resolve));
  // Only the exact mkdtemp-created Chromium profile owned by this test is removed.
  await rm(profile, {recursive:true,force:true});
}
