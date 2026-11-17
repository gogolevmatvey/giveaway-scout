// Headless renderer coverage with fake IPC; never opens real folders or production data.
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { startFixtureServer } from '../src/GiveawayScout.Desktop/tests/ui/fixture-server.mjs';

const server = await startFixtureServer();
const profile = await mkdtemp(path.join(tmpdir(), 'giveaway-scout-logging-ui-'));
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

  const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const value = (selector, text) => evaluate(`(() => {
    const input=document.querySelector(${JSON.stringify(selector)});
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(text)});
    input.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  const open = async (section = 'diagnostics') => {
    await click('.header-button');
    await waitFor(`!!document.querySelector('#settings-tab-${section}')`, 'settings navigation');
    await click(`#settings-tab-${section}`);
    await waitFor(`!!document.querySelector('#settings-panel-${section}')?.offsetParent`, section + ' section');
    if (section === 'diagnostics') {
      await click('.settings-technical-details summary');
      await waitFor(`document.querySelector('.settings-technical-details').open`, 'technical details');
    }
  };
  const save = async () => { await click('.dialog-actions .primary-button'); await waitFor(`!document.querySelector('[role="dialog"]')`, 'saved settings'); };
  const load = async () => {
    await command('Page.navigate', {url:server.url});
    await waitFor(`!!document.querySelector('.header-button:not(:disabled)')`, 'app ready');
  };
  const checkCalls = async () => assert.deepEqual(await evaluate('window.__uiTest.unexpected'), []);
  for (const language of ['en','ru']) {
    await load();
    if(language==='ru') { await open('application'); await click('.segmented-control button:first-child'); await save(); }
    await open();
    assert.equal(await evaluate(`document.querySelector('#log-file-size').value`), '2');
    assert.equal(await evaluate(`document.querySelector('#log-file-count').value`), '1');
    assert.equal(await evaluate(`document.querySelector('[data-testid="default-log-directory"]').disabled`), true);
    assert.match(await evaluate(`document.querySelector('[data-testid="log-active-file"]').textContent`), /Fixture logs\/Giveaway Scout\.log/);
    assert.equal(await evaluate(`document.querySelectorAll('.logging-restart-required').length`), 0);
    for (const [selector, invalid] of [['#log-file-size',['','0','101','1.5']],['#log-file-count',['','0','11','2.5']]]) {
      const original=selector==='#log-file-size'?'2':'1';
      for(const entry of invalid) {
        await value(selector,entry);
        await waitFor(`document.querySelector(${JSON.stringify(selector)}).getAttribute('aria-invalid')==='true'`, 'invalid '+selector+' '+entry);
        assert.equal(await evaluate(`document.querySelector('.dialog-actions .primary-button').disabled`), true);
        await click('.settings-validation-link');
        await waitFor(`document.activeElement.id===${JSON.stringify(selector.slice(1))}`, 'validation focuses ' + selector);
      }
      await value(selector,original);
    }
    await waitFor(`document.querySelector('#log-file-size').getAttribute('aria-invalid')==='false' && document.querySelector('#log-file-count').getAttribute('aria-invalid')==='false' && document.querySelector('.dialog-actions .primary-button').disabled`, 'valid unchanged limits');
    // Native dialog cancellation changes neither the draft nor the saved configuration.
    await click('[data-testid="choose-log-directory"]');
    await waitFor(`!document.querySelector('[data-testid="choose-log-directory"]').disabled`, 'folder cancelled');
    assert.equal(await evaluate(`document.querySelector('.settings-draft-status').textContent`), language==='en'?'No changes':'Изменений нет');
    const chosen='C:/A very long folder name/'+'nested folder/'.repeat(12)+'logs';
    await evaluate(`window.__uiTest.folderChoice=${JSON.stringify(chosen)}`);
    await click('[data-testid="choose-log-directory"]');
    await waitFor(`document.querySelector('[data-testid="log-directory-draft"]').textContent===${JSON.stringify(chosen)}`, 'draft folder selected');
    await click('[data-testid="open-log-directory"]');
    await waitFor(`window.__uiTest.openedLogDirectory==='C:/Fixture logs'`, 'opens active folder');
    assert.match(await evaluate(`document.querySelector('[data-testid="log-active-file"]').textContent`), /Fixture logs/);
    for (const [width,height] of [[760,600],[960,600]]) {
      await command('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
      const geometry = await evaluate(`(() => {
        const dialog=document.querySelector('[role="dialog"]'), body=document.querySelector('.settings-content');
        document.querySelector('.logging-settings').scrollIntoView({block:'start'});
        const rect=dialog.getBoundingClientRect(),footer=document.querySelector('.settings-footer').getBoundingClientRect();
        return {top:rect.top,bottom:rect.bottom,right:rect.right,footer:footer.bottom,width:innerWidth,height:innerHeight,
          overflows:body.scrollWidth>body.clientWidth,scrollable:body.scrollHeight>body.clientHeight};
      })()`);
      assert.ok(geometry.top>=0&&geometry.bottom<=height&&geometry.right<=width&&geometry.footer<=height,JSON.stringify(geometry));
      assert.equal(geometry.overflows,false,JSON.stringify(geometry));
      assert.equal(geometry.scrollable,true);
    }
    await click('[data-testid="default-log-directory"]');
    await waitFor(`document.querySelector('.settings-draft-status').textContent===${JSON.stringify(language==='en'?'No changes':'Изменений нет')}`, 'default folder restores draft');
    await click('[data-testid="choose-log-directory"]');
    await waitFor(`!document.querySelector('[data-testid="choose-log-directory"]').disabled`, 'choose folder');
    await click('.dialog-actions .secondary-button');
    await waitFor(`!!document.querySelector('.settings-close-prompt')`, 'cancel protects draft');
    await click('.settings-close-actions button:last-child');
    await waitFor(`!document.querySelector('[role="dialog"]')`, 'discard folder');
    await open();
    assert.equal(await evaluate(`document.querySelector('[data-testid="default-log-directory"]').disabled`), true);
    await value('#log-file-size','8'); await value('#log-file-count','3');
    await click('[data-testid="choose-log-directory"]');
    await waitFor(`!document.querySelector('[data-testid="choose-log-directory"]').disabled`, 'choose folder for save');
    await save();
    await waitFor(`!!document.querySelector('.logging-status')`, 'restart status outside settings');
    const settings = await evaluate(`window.__uiTest.calls.filter(call=>call.command==='update_settings').at(-1).args.settings`);
    assert.equal(settings.logMaxFileSizeMb,8); assert.equal(settings.logFileCount,3); assert.equal(settings.logDirectory,chosen);
    await open();
    await waitFor(`!!document.querySelector('.logging-restart-required')`, 'restart notice in settings');
    assert.equal(await evaluate(`document.querySelector('#log-file-size').value`),'8');
    assert.equal(await evaluate(`document.querySelector('#log-file-count').value`),'3');
    // Startup fallback remains a separate state from a saved configuration change.
    await evaluate(`window.__uiTest.logging.usingFallback=true; window.__uiTest.logging.lastError='Fixture folder unavailable'`);
    await waitFor(`!!document.querySelector('.log-error')`, 'fallback diagnostics');
    assert.match(await evaluate(`document.querySelector('.log-error').textContent`),language==='en'?/default folder/:/стандартную папку/);
    await evaluate(`window.__uiTest.logging.usingFallback=false; window.__uiTest.logging.activeFilePath=null; window.__uiTest.logging.effectiveDirectory=null`);
    await waitFor(`document.querySelector('[data-testid="open-log-directory"]').disabled`, 'no active folder disables Open');
    assert.match(await evaluate(`document.querySelector('.log-error').textContent`),language==='en'?/continues without file logging/:/продолжает работать без файлового журнала/);
    await checkCalls();
  }
  // Detailed logging is saved immediately and does not require a policy restart.
  await load(); await open();
  await click('.logging-settings .check-setting input'); await save();
  assert.equal(await evaluate(`window.__uiTest.calls.filter(call=>call.command==='update_settings').at(-1).args.settings.verboseLogging`),true);
  assert.equal(await evaluate(`document.querySelectorAll('.logging-status').length`),0);
  const labels = await evaluate(`Array.from(document.querySelectorAll('.source-row div > span')).map(node=>node.textContent)`);
  assert.deepEqual(labels,['Steam','Steam · Epic Games · GOG','Steam','Steam · HTTP · one connection per IP']);
  await checkCalls();
  console.log('PASS: logging defaults/validation, folder cancel/select/default/discard, actual folder Open, persistence/restart notice, fallback/disabled file logging, immediate detailed logging and source labels. RU/EN at 760x600 and 960x600. Renderer with fake Tauri only; no native dialog, file writes or production data.');

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
  await server.close();
  // Only the exact mkdtemp-created Chromium profile owned by this test is removed.
  await rm(profile, {recursive:true,force:true});
}
