import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { before, after, afterEach, test } from 'node:test';
import { repository } from '../../scripts/windows-ui-toolchain.mjs';
import { createProfile, launchApp, readEvents, setScenario, waitUntil, preserveProfile, verifiedArtifact } from './native-app.mjs';

const report = process.env.GIVEAWAY_SCOUT_UI_REPORT;
if(!report) throw new Error('Run pnpm test:ui:native, not the test file directly');
let root;
let app;
const snapshot = () => app.browser.execute(() => window.__TAURI_INTERNALS__.invoke('get_storage_snapshot'));
const check = async () => {
  await app.browser.$('.source-actions .primary-button').waitForEnabled();
  const before=(await snapshot()).monitor.lastStartedAtUtc;
  await app.browser.$('.source-actions .primary-button').click();
  await waitUntil(async () => {const state=await snapshot();return !state.monitor.isChecking && state.monitor.lastStartedAtUtc!==before;},'new check complete');
  await app.browser.$('.source-actions .primary-button').waitForEnabled();
};
before(async () => {root=await createProfile();app=await launchApp(root);},{timeout:60_000});
afterEach(async context => {
  if(context.passed === false && app) await app.browser.saveScreenshot(path.join(report,'failure.png')).catch(()=>{});
});
after(async () => {
  try { if(app) await app.stop(); }
  finally { if(root) await preserveProfile(root,report); }
});

test('real Rust and SQLite: baseline, new giveaway, no duplicate, settings and history survive restart',async () => {
  await waitUntil(async () => (await snapshot()).monitor.lastSuccessfulCheckUtc !== null,'initial baseline');
  const mainWindow=JSON.parse(await fs.readFile(path.join(repository,'src/GiveawayScout.Desktop/src-tauri/tauri.conf.json'),'utf8')).app.windows.find(window=>window.label==='main');
  const startup=await app.browser.execute(()=>({
    width:innerWidth,height:innerHeight,pageScroll:document.documentElement.scrollHeight>innerHeight,
    actionsBottom:document.querySelector('.source-actions').getBoundingClientRect().bottom,
  }));
  assert.equal(startup.width,mainWindow.width);
  assert.equal(startup.height,mainWindow.height);
  assert.equal(startup.pageScroll,false);
  assert.ok(startup.actionsBottom<=startup.height);
  await app.browser.saveScreenshot(path.join(report,'startup-compact.png'));
  assert.equal((await snapshot()).counts.trackedGiveaways,2);
  assert.equal((await snapshot()).counts.notificationHistory,0);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,0);

  await app.browser.$('.header-button').click();
  await app.browser.$('#check-interval').setValue('60');
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});

  await setScenario(root,'newGiveaway');
  await check();
  await app.browser.$('.history-item.unread').waitForDisplayed();
  await app.browser.saveScreenshot(path.join(report,'main-compact-history.png'));
  assert.equal((await snapshot()).counts.notificationHistory,1);
  const events=await readEvents(root);
  const notification=events.filter(e=>e.kind==='notification');
  assert.equal(notification.length,1);
  assert.match(notification[0].title,/E2E new Steam game/);
  assert.match(notification[0].activation,/^giveaway-scout:\/\/notification\/[a-f0-9-]{36}$/);
  await check();
  assert.equal((await snapshot()).counts.notificationHistory,1);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,1);

  await app.stop();app=null;
  app=await launchApp(root);
  await waitUntil(async () => (await snapshot()).monitor.lastSuccessfulCheckUtc !== null,'restart check');
  assert.equal((await snapshot()).settings.checkIntervalMinutes,60);
  assert.equal((await snapshot()).counts.notificationHistory,1);
  assert.equal((await snapshot()).counts.unreadNotifications,1);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,1);
  await app.browser.$('.history-item.unread').click();
  await app.browser.$('.history-item.unread').waitForExist({reverse:true});
  assert.equal((await snapshot()).counts.unreadNotifications,0);
  const final=await readEvents(root);
  assert.deepEqual(final.filter(e=>e.kind==='openUrl').map(e=>e.url),['https://givee.club/event/900003']);
  assert.equal(final.filter(e=>e.kind==='attention').at(-1).unread,0);
},{timeout:120_000});

test('source failure is visible, preserves data and recovers',async () => {
  await setScenario(root,'sourceFailure');
  await check();
  const failed=await snapshot();
  assert.match(failed.monitor.lastError,/Givee Club: UI_TEST_SOURCE_UNAVAILABLE/);
  assert.equal(failed.monitor.sourceFailures[0].reason.code,'connection');
  await waitUntil(async () => (await app.browser.$('.status-panel').getText()).includes('Could not connect to the source'),'localized source error shown in UI');
  assert.equal((await app.browser.$('.status-panel').getText()).includes('UI_TEST_SOURCE_UNAVAILABLE'),false);
  assert.equal(failed.counts.notificationHistory,1);
  assert.equal(failed.counts.trackedGiveaways,3);
  await setScenario(root,'newGiveaway');
  await check();
  assert.equal((await snapshot()).monitor.lastError,null);
  await waitUntil(async () => !(await app.browser.$('.status-panel').getText()).includes('Could not connect to the source'),'source error cleared in UI');
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,1);
},{timeout:45_000});

test('Zeros opt-in uses real parser and SQLite: silent baseline, single alert, HTTP link and persisted settings',async () => {
  const source=await app.browser.$('.source-list > .source-row:nth-child(3)');
  assert.equal(await source.getAttribute('aria-checked'),'false');
  assert.match(await source.getText(),/HTTP/);
  await source.click();
  await waitUntil(async ()=>(await snapshot()).settings.zerosEnabled,'Zeros setting persisted');
  await check();
  assert.equal((await snapshot()).counts.trackedGiveaways,4);
  assert.equal((await snapshot()).counts.notificationHistory,1);
  await setScenario(root,'zerosGiveaway');
  await check();
  assert.equal((await snapshot()).counts.notificationHistory,2);
  const history=await app.browser.execute(()=>window.__TAURI_INTERNALS__.invoke('get_notification_history'));
  const record=history.find(item=>item.source==='zeros');
  assert.equal(record.title,'E2E Zeros new Steam game');
  assert.equal(record.url,'http://zeros.group/free/');
  await check();
  assert.equal((await snapshot()).counts.notificationHistory,2);
  await waitUntil(async ()=>(await readEvents(root)).filter(e=>e.kind==='notification').length===2,'Zeros notification request');
  await app.browser.$('.history-item.unread').click();
  await waitUntil(async ()=>(await snapshot()).counts.unreadNotifications===0,'Zeros marked read');
  assert.equal((await readEvents(root)).filter(e=>e.kind==='openUrl').at(-1).url,'http://zeros.group/free/');
  await app.stop();app=null;app=await launchApp(root);
  await waitUntil(async ()=>(await snapshot()).monitor.lastSuccessfulCheckUtc!==null,'restart with Zeros');
  assert.equal((await snapshot()).settings.zerosEnabled,true);
  assert.equal((await snapshot()).counts.notificationHistory,2);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,2);
},{timeout:90_000});

test('native Zeros failure has a stable reason and switches between English and Russian',async()=>{
  await setScenario(root,'zerosConnectionLimit');
  await check();
  assert.equal((await snapshot()).monitor.sourceFailures[0].reason.code,'zerosConnectionLimit');
  await waitUntil(async()=>(await app.browser.$('.status-panel').getText()).includes('one connection per public IP'),'English Zeros error');
  assert.equal(await app.browser.$('main h1').isExisting(),false);
  assert.equal(await app.browser.$('.source-row:nth-child(1) div > span').isExisting(),false);
  assert.equal(await app.browser.$('.source-row:nth-child(2) div > span').isExisting(),false);
  await app.browser.saveScreenshot(path.join(report,'compact-english-error.png'));
  await app.browser.$('.header-button').click();
  await app.browser.$('button=Русский').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await waitUntil(async()=>(await app.browser.$('.status-panel').getText()).includes('одно подключение на внешний IP'),'Russian Zeros error');
  assert.equal((await app.browser.$('.status-panel').getText()).includes('Zeros permits'),false);
  await app.browser.saveScreenshot(path.join(report,'compact-russian-error.png'));
  await app.browser.$('.header-button').click();
  await app.browser.$('button=English').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await setScenario(root,'zerosGiveaway');
  await check();
  assert.deepEqual((await snapshot()).monitor.sourceFailures,[]);
  assert.equal((await snapshot()).counts.notificationHistory,2);
},{timeout:60_000});

test('native settings footer stays visible; dirty draft is protected and localized',async()=>{
  const initial=await snapshot();
  await app.browser.$('.header-button').click();
  const geometry=await app.browser.execute(()=>{
    const body=document.querySelector('.settings-content');
    const footer=document.querySelector('.settings-footer');
    const before=footer.getBoundingClientRect();
    body.scrollTop=body.scrollHeight;
    const after=footer.getBoundingClientRect();
    return {overflow:body.scrollHeight>body.clientHeight,top:before.top,
      afterTop:after.top,bottom:after.bottom,viewport:innerHeight};
  });
  assert.equal(geometry.overflow,true);
  assert.ok(Math.abs(geometry.top-geometry.afterTop)<1);
  assert.ok(geometry.bottom<=geometry.viewport);
  await app.browser.saveScreenshot(path.join(report,'settings-fixed-footer-english.png'));
  await app.browser.$('#check-interval').setValue(String(initial.settings.checkIntervalMinutes+1));
  await app.browser.keys('Escape');
  await app.browser.$('.settings-close-prompt').waitForDisplayed();
  await app.browser.saveScreenshot(path.join(report,'settings-unsaved-english.png'));
  assert.equal((await snapshot()).settings.checkIntervalMinutes,initial.settings.checkIntervalMinutes);
  await app.browser.$('button=Discard changes').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.$('.header-button').click();
  await app.browser.$('button=Русский').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.$('.header-button').click();
  await app.browser.$('#check-interval').setValue(String(initial.settings.checkIntervalMinutes+1));
  await app.browser.keys('Escape');
  await app.browser.$('.settings-close-prompt').waitForDisplayed();
  assert.match(await app.browser.$('.settings-draft-status').getText(),/несохранённые/);
  const dirtyBounds=await app.browser.execute(()=>({
    top:document.querySelector('.dialog-header').getBoundingClientRect().top,
    bottom:document.querySelector('.settings-footer').getBoundingClientRect().bottom,
    height:innerHeight,scroll:document.querySelector('.modal-backdrop').scrollTop,
    dialogTop:document.querySelector('.settings-dialog').getBoundingClientRect().top,
    dialogHeight:document.querySelector('.settings-dialog').getBoundingClientRect().height,
    maxHeight:getComputedStyle(document.querySelector('.settings-dialog')).maxHeight,
  }));
  assert.ok(dirtyBounds.top>=0 && dirtyBounds.bottom<=dirtyBounds.height,JSON.stringify(dirtyBounds));
  await app.browser.saveScreenshot(path.join(report,'settings-unsaved-russian.png'));
  await app.browser.$('button=Не сохранять').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.$('.header-button').click();
  await app.browser.$('button=English').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  const final=await snapshot();
  assert.equal(final.settings.checkIntervalMinutes,initial.settings.checkIntervalMinutes);
  assert.deepEqual(final.counts,initial.counts);
},{timeout:60_000});

test('independent startup/close settings persist through Rust, SQLite and restart',async()=>{
  const before=await snapshot();
  // This .e2e.dev build skips OS autostart registration. Enable only the isolated
  // SQLite preference through IPC so the dependent checkbox can be exercised.
  await app.browser.execute(async settings=>window.__TAURI_INTERNALS__.invoke('update_settings',{
    settings:{...settings,startAtLogin:true},
  }),before.settings);
  await app.browser.refresh();
  await app.browser.$('.header-button').waitForEnabled();
  const expectedVersion=JSON.parse(await fs.readFile(path.join(repository,'src/GiveawayScout.Desktop/package.json'),'utf8')).version;
  const appState=await app.browser.execute(()=>window.__TAURI_INTERNALS__.invoke('get_app_state'));
  assert.equal(appState.version,expectedVersion);
  for(const startup of [true,false]) {
    for(const close of [true,false]) {
      await app.browser.$('.header-button').click();
      for(const [id,value] of [['start-in-tray',startup],['keep-running-in-tray',close]]) {
        const input=await app.browser.$(`#${id}`);
        if(await input.isSelected()!==value) await input.click();
      }
      if(startup && close) {
        await app.browser.$('#start-in-tray').scrollIntoView({block:'center'});
        await app.browser.saveScreenshot(path.join(report,'settings-independent-tray-options.png'));
      }
      await app.browser.$('.dialog-actions .primary-button').click();
      await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
      const saved=await snapshot();
      assert.equal(saved.settings.startInTray,startup);
      assert.equal(saved.settings.keepRunningInTray,close);
      assert.deepEqual(saved.counts,before.counts);
    }
  }
  // The final close preference is false, so the owned process exits normally.
  await app.stop(); app=null; app=await launchApp(root);
  await waitUntil(async()=>(await snapshot()).monitor.lastSuccessfulCheckUtc!==null,'restart after preferences');
  const saved=await snapshot();
  assert.equal(saved.settings.startInTray,false);
  assert.equal(saved.settings.keepRunningInTray,false);
  await app.browser.$('.header-button').click();
  assert.equal(await app.browser.$('#start-in-tray').isEnabled(),true);
  await app.browser.keys('Escape');
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.deepEqual(saved.counts,before.counts);
},{timeout:90_000});

test('FlaUI invokes Settings and Close settings in this test process only',async () => {
  const dll=path.join(repository,'tests/GiveawayScout.WindowsUi/bin/Debug/net10.0-windows/GiveawayScout.WindowsUi.dll');
  const child=spawn(path.join(process.env.ProgramFiles,'dotnet/dotnet.exe'),[dll,'--test-pid',String(app.child.pid)],{stdio:['ignore','pipe','pipe'],windowsHide:true,timeout:20_000});
  let output='';child.stdout.on('data',chunk=>output+=chunk);child.stderr.on('data',chunk=>output+=chunk);
  const [code]=await once(child,'exit');
  await fs.writeFile(path.join(report,'flaui.txt'),output);
  assert.equal(code,0,output);
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
},{timeout:30_000});

test('test executable refuses startup without an owned temp profile',async () => {
  const env={...process.env};delete env.GIVEAWAY_SCOUT_UI_ROOT;
  const child=spawn(await verifiedArtifact(),[],{env,stdio:['ignore','ignore','pipe'],windowsHide:true,timeout:5_000});
  let output='';child.stderr.on('data',data=>output+=data);
  const [code]=await once(child,'exit');
  assert.equal(code,2,output);
  assert.match(output,/missing GIVEAWAY_SCOUT_UI_ROOT/);
},{timeout:10_000});
