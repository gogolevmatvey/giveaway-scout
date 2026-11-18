import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { before, after, afterEach, test } from 'node:test';
import { repository } from '../../scripts/windows-ui-toolchain.mjs';
import { createProfile, launchApp, readEvents, setScenario, waitUntil, preserveProfile, verifiedArtifact, armAttentionRace } from './native-app.mjs';
import { historyViewport, settledHistoryViewport, scrollHistory, walkHistory, historyAnchor } from './history-ui.mjs';

const report = process.env.GIVEAWAY_SCOUT_UI_REPORT;
if(!report) throw new Error('Run pnpm test:ui:native, not the test file directly');
let root;
let app;
const snapshot = () => app.browser.execute(() => window.__TAURI_INTERNALS__.invoke('get_storage_snapshot'));
const loggingSnapshot = () => app.browser.execute(() => window.__TAURI_INTERNALS__.invoke('get_logging_state'));
const normalPath = value => path.normalize(value.replace(/^\\\\\?\\/,'')).replace(/[\\/]+$/,'').toLowerCase();
const samePath = (actual, expected) => assert.equal(normalPath(actual),normalPath(expected));
const logNames = async directory => (await fs.readdir(directory)).filter(name => /^Giveaway Scout(?:\.\d+)?\.log$/.test(name)).sort();
async function flaui(mode, action, trigger) {
  const dll=path.join(repository,'tests/GiveawayScout.WindowsUi/bin/Debug/net10.0-windows/GiveawayScout.WindowsUi.dll');
  const child=spawn(path.join(process.env.ProgramFiles,'dotnet/dotnet.exe'),[dll,mode,String(app.child.pid),action,root],{stdio:['ignore','pipe','pipe'],windowsHide:true,timeout:25_000});
  let output='';child.stdout.on('data',chunk=>output+=chunk);child.stderr.on('data',chunk=>output+=chunk);
  const exited=once(child,'exit');
  // Register rejection handling immediately while waiting for the ready signal.
  exited.catch(()=>{});
  try {
    if(trigger) {
      await waitUntil(()=>{
        if(child.exitCode!==null) throw new Error(output);
        return output.includes('READY:');
      },'FlaUI observes Explorer before opening the owned folder',15_000);
      await trigger();
    }
    const [code]=await exited;
    assert.equal(code,0,output);
  } finally {
    await fs.appendFile(path.join(report,'flaui-logging.txt'),output);
    if(child.exitCode===null && child.signalCode===null) {child.kill();await exited.catch(()=>{});}
  }
}
async function openDiagnostics() {
  await app.browser.$('.header-button').click();
  await selectSettingsSection(app.browser,'diagnostics');
  await app.browser.$('.logging-settings').scrollIntoView({block:'center'});
  await openTechnicalDetails(app.browser);
}
async function selectSettingsSection(browser, section) {
  const tab=browser.$(`[data-settings-section="${section}"]`);
  await tab.waitForEnabled();
  if(await tab.getAttribute('aria-selected')!=='true') await tab.click();
  await waitUntil(async()=>await tab.getAttribute('aria-selected')==='true','settings section '+section);
  await browser.$(`#settings-panel-${section}`).waitForDisplayed();
}
async function openTechnicalDetails(browser) {
  const details=browser.$('.settings-technical-details');
  if(await details.getAttribute('open')===null) {
    await details.$('summary').scrollIntoView({block:'center'});
    await details.$('summary').click();
  }
}
async function saveSettings() {
  const save=app.browser.$('.dialog-actions .primary-button');
  if(await save.isEnabled()) await save.click();
  else await app.browser.$('.dialog-actions .secondary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
}
async function clickCardArea(browser, selector, bottom = false) {
  const point=await browser.execute(({selector,bottom})=>{
    const rect=document.querySelector(selector).getBoundingClientRect();
    return {x:Math.round(rect.left+rect.width/2),y:Math.round(bottom?rect.bottom-4:rect.top+rect.height/2)};
  },{selector,bottom});
  await browser.performActions([{type:'pointer',id:'card-pointer',parameters:{pointerType:'mouse'},actions:[
    {type:'pointerMove',duration:0,origin:'viewport',...point},
    {type:'pointerDown',button:0},{type:'pointerUp',button:0},
  ]}]);
  await browser.releaseActions();
}
const check = async () => {
  await app.browser.$('.source-actions .primary-button').waitForEnabled();
  const before=(await snapshot()).monitor.lastStartedAtUtc;
  await app.browser.$('.source-actions .primary-button').click();
  await waitUntil(async () => {const state=await snapshot();return !state.monitor.isChecking && state.monitor.lastStartedAtUtc!==before;},'new check complete');
  await app.browser.$('.source-actions .primary-button').waitForEnabled();
};
before(async () => {root=await createProfile();app=await launchApp(root);},{timeout:60_000});
afterEach(async context => {
  if(context.passed === false && app) {
    const slug=context.name.toLowerCase().replace(/[^a-z0-9]+/g,'-').slice(0,100);
    await app.browser.saveScreenshot(path.join(report,`failure-${slug}.png`)).catch(()=>{});
    const diagnostics=await app.browser.execute(async()=>({
      alerts:[...document.querySelectorAll('[role="alert"]')].map(element=>element.textContent),
      selectedFolder:document.querySelector('[data-testid="log-directory-draft"]')?.textContent,
      activeFile:document.querySelector('[data-testid="log-active-file"]')?.textContent,
      pickerDisabled:document.querySelector('[data-testid="choose-log-directory"]')?.disabled,
      logging:await window.__TAURI_INTERNALS__.invoke('get_logging_state'),
    })).catch(error=>({error:error.message}));
    await fs.writeFile(path.join(report,`failure-${slug}.json`),JSON.stringify(diagnostics,null,2)+'\n');
  }
});
after(async () => {
  try { if(app) await app.stop(); }
  finally { if(root) await preserveProfile(root,report); }
});

test('real Rust and SQLite: baseline, new giveaway, no duplicate, settings and history survive restart',async () => {
  await waitUntil(async () => {
    const {monitor}=await snapshot();
    return !monitor.isChecking && monitor.lastSuccessfulCheckUtc !== null;
  },'initial baseline');
  const mainWindow=JSON.parse(await fs.readFile(path.join(repository,'src/GiveawayScout.Desktop/src-tauri/tauri.ui-tests.conf.json'),'utf8')).app.windows.find(window=>window.label==='main');
  const startup=await app.browser.execute(()=>({
    width:innerWidth,height:innerHeight,pageScroll:document.documentElement.scrollHeight>innerHeight,
    actionsBottom:document.querySelector('.source-actions').getBoundingClientRect().bottom,
  }));
  assert.equal(startup.width,mainWindow.width);
  assert.equal(startup.height,mainWindow.height);
  assert.equal(startup.pageScroll,false);
  assert.ok(startup.actionsBottom<=startup.height);
  // This native driver's moveTo dispatches a synthetic MouseEvent, which does
  // not activate CSS :hover. Chrome covers the actual hover color; WebView2
  // verifies that the row reaches and receives hits at its right content edge.
  const sourceEdge=await app.browser.execute(()=>{
    const list=document.querySelector('.source-list'),row=document.querySelector('[data-source="giveeClub"] .source-row');
    const bounds=list.getBoundingClientRect(),entry=row.getBoundingClientRect();
    return {scrolling:list.scrollHeight>list.clientHeight,rightGap:bounds.right-parseFloat(getComputedStyle(list).borderRightWidth)-entry.right,
      rightPixelIsRow:row.contains(document.elementFromPoint(entry.right-2,entry.top+entry.height/2))};
  });
  assert.equal(sourceEdge.scrolling,false,JSON.stringify(sourceEdge));
  assert.ok(Math.abs(sourceEdge.rightGap)<=1,JSON.stringify(sourceEdge));
  assert.equal(sourceEdge.rightPixelIsRow,true,JSON.stringify(sourceEdge));
  await app.browser.saveScreenshot(path.join(report,'startup-compact.png'));
  assert.deepEqual((await snapshot()).settings.sourceCheckIntervals,{freeSteamKeys:60,giveeClub:60,gamesForFarm:60,zeros:60});
  assert.equal((await snapshot()).counts.trackedGiveaways,2);
  assert.equal((await snapshot()).counts.notificationHistory,0);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,0);

  await app.browser.$('.header-button').click();
  const expectedIntervals={freeSteamKeys:15,giveeClub:120,gamesForFarm:45,zeros:90};
  for(const [source, minutes] of Object.entries(expectedIntervals)) {
    await app.browser.$('#check-interval-'+source).setValue(String(minutes));
  }
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
  assert.deepEqual((await snapshot()).settings.sourceCheckIntervals,expectedIntervals);
  await app.browser.$('.header-button').click();
  for(const [source, minutes] of Object.entries(expectedIntervals)) {
    assert.equal(await app.browser.$('#check-interval-'+source).getValue(),String(minutes));
  }
  await app.browser.keys('Escape');
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal((await snapshot()).counts.notificationHistory,1);
  assert.equal((await snapshot()).counts.unreadNotifications,1);
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,1);
  await app.browser.$('.history-item.unread .history-open').click();
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
  await waitUntil(async () => (await app.browser.$('[data-source="giveeClub"] .source-phase').getText()).includes('Error'),'localized source error shown in UI');
  const details=app.browser.$('[data-source="giveeClub"] .source-details');
  await details.$('summary').click();
  assert.equal(await details.$('.setting-error').getText(),'Could not connect to the source.');
  assert.equal(await details.$$('p').length,1);
  assert.equal(await app.browser.execute(()=>document.body.textContent.includes('UI_TEST_SOURCE_UNAVAILABLE')),false);
  assert.equal((await app.browser.$('.status-panel').getText()).includes('UI_TEST_SOURCE_UNAVAILABLE'),false);
  assert.equal(failed.counts.notificationHistory,1);
  assert.equal(failed.counts.trackedGiveaways,3);
  await setScenario(root,'newGiveaway');
  await check();
  assert.equal((await snapshot()).monitor.lastError,null);
  await waitUntil(async () => !(await app.browser.$('[data-source="giveeClub"] .source-phase').getText()).includes('Error'),'source error cleared in UI');
  await details.waitForExist({reverse:true});
  assert.equal((await readEvents(root)).filter(e=>e.kind==='notification').length,1);
},{timeout:45_000});

test('Zeros opt-in uses real parser and SQLite: silent baseline, single alert, HTTP link and persisted settings',async () => {
  const source=await app.browser.$('[data-source="zeros"] .source-row');
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
  await app.browser.$('.history-item.unread .history-open').click();
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
  await app.browser.$('[data-source="zeros"] .source-details summary').click();
  await waitUntil(async()=>(await app.browser.$('[data-source="zeros"]').getText()).includes('one connection per public IP'),'English Zeros error');
  assert.equal(await app.browser.$('main h1').isExisting(),false);
  assert.equal(await app.browser.$('[data-source="giveeClub"] .source-row div > span').getText(),'Steam · Epic Games · GOG');
  assert.equal(await app.browser.$('[data-source="gamesForFarm"] .source-row div > span').getText(),'Steam');
  await app.browser.saveScreenshot(path.join(report,'compact-english-error.png'));
  await app.browser.$('.header-button').click();
  await selectSettingsSection(app.browser,'application');
  await app.browser.$('button=Русский').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await waitUntil(async()=>(await app.browser.$('[data-source="zeros"]').getText()).includes('одно подключение на внешний IP'),'Russian Zeros error');
  assert.equal(await app.browser.$('[data-source="zeros"] .source-diagnostic').isExisting(),false);
  assert.equal(await app.browser.$$('[data-source="zeros"] .source-details p').length,1);
  assert.equal(await app.browser.execute(()=>document.body.textContent.includes('Zeros permits')),false);
  assert.match((await snapshot()).monitor.sourceFailures[0].message,/Zeros permits/);
  await app.browser.saveScreenshot(path.join(report,'compact-russian-error.png'));
  await app.browser.$('.header-button').click();
  await selectSettingsSection(app.browser,'application');
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
  assert.equal(await app.browser.$('.dialog-actions .primary-button').isEnabled(),false);
  assert.equal(await app.browser.$$('[role="tab"][data-settings-section]').length,6);
  const sections=[];
  for(const section of ['sources','notifications','application','network','diagnostics','updatesData']) {
    await selectSettingsSection(app.browser,section);
    if(section==='diagnostics') await openTechnicalDetails(app.browser);
    const geometry=await app.browser.execute(()=>{
      const body=document.querySelector('.settings-content'),footer=document.querySelector('.settings-footer');
      const header=document.querySelector('.dialog-header'),nav=document.querySelector('[role="tablist"]');
      const before={footer:footer.getBoundingClientRect().top,header:header.getBoundingClientRect().top,nav:nav.getBoundingClientRect().top};
      body.scrollTop=body.scrollHeight;
      const rect=element=>{const r=element.getBoundingClientRect();return {top:r.top,bottom:r.bottom,left:r.left,right:r.right,centerY:r.top+r.height/2};};
      return {overflow:body.scrollHeight>body.clientHeight,before,after:{footer:footer.getBoundingClientRect().top,header:header.getBoundingClientRect().top,nav:nav.getBoundingClientRect().top},
        body:rect(body),footer:rect(footer),header:rect(header),nav:rect(nav),viewport:innerHeight,
        status:rect(document.querySelector('.settings-footer-status')),actions:rect(document.querySelector('.dialog-actions')),
        visiblePanels:[...document.querySelectorAll('[role="tabpanel"]')].filter(panel=>panel.getClientRects().length>0).length};
    });
    sections.push({section,...geometry});
    assert.equal(geometry.visiblePanels,1,JSON.stringify(geometry));
    for(const key of ['footer','header','nav'])assert.ok(Math.abs(geometry.before[key]-geometry.after[key])<1,JSON.stringify(geometry));
    assert.ok(geometry.header.top>=0&&geometry.footer.bottom<=geometry.viewport,JSON.stringify(geometry));
    assert.ok(geometry.nav.right<=geometry.body.left+1,JSON.stringify(geometry));
    assert.ok(Math.abs(geometry.status.centerY-geometry.actions.centerY)<=1,JSON.stringify(geometry));
    assert.ok(geometry.status.right<=geometry.actions.left-8,JSON.stringify(geometry));
  }
  if(sections[0].viewport<=600)assert.ok(sections.some(section=>section.overflow),'long settings remain scrollable in the smaller window');
  await fs.writeFile(path.join(report,'settings-navigation-geometry.json'),JSON.stringify(sections,null,2)+'\n');
  await selectSettingsSection(app.browser,'sources');
  await app.browser.saveScreenshot(path.join(report,'settings-fixed-footer-english.png'));
  const controlsReport=path.join(report,'settings-controls');await fs.mkdir(controlsReport);
  const originalRect=await app.browser.getWindowRect(),originalInner=await app.browser.execute(()=>({width:innerWidth,height:innerHeight}));
  const chromeWidth=originalRect.width-originalInner.width,chromeHeight=originalRect.height-originalInner.height;
  const controlMeasurements=[];
  for(const language of ['en','ru']) {
    if(language==='ru') {
      await selectSettingsSection(app.browser,'application');await app.browser.$('button=Русский').click();
      await app.browser.$('.dialog-actions .primary-button').click();await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
      await app.browser.$('.header-button').click();
    }
    for(const [width,height] of [[1100,740],[760,600]]) {
      await app.browser.setWindowRect(originalRect.x,originalRect.y,width+chromeWidth,height+chromeHeight);
      await waitUntil(async()=>await app.browser.execute(size=>innerWidth===size.width&&innerHeight===size.height,{width,height}),'exact settings control client size '+width+'x'+height);
      await selectSettingsSection(app.browser,'sources');
      await app.browser.saveScreenshot(path.join(controlsReport,language+'-'+width+'x'+height+'-sources-clean.png'));
      const zeros=app.browser.$('#source-enabled-zeros');
      if(await zeros.isSelected())await app.browser.$('label[for="source-enabled-zeros"] > span').click();
      await app.browser.$('#check-interval-freeSteamKeys').setValue('1');
      await app.browser.execute(()=>document.querySelector('#source-enabled-freeSteamKeys').focus());
      assert.equal(await app.browser.$('.dialog-actions .primary-button').isEnabled(),false);
      assert.equal(await app.browser.$('.settings-validation-link').isDisplayed(),true);
      const validation=await app.browser.execute(()=>{
        const rect=element=>{const r=element.getBoundingClientRect();return {left:r.left,right:r.right,centerY:r.top+r.height/2,bottom:r.bottom};};
        return {status:rect(document.querySelector('.settings-footer-status')),actions:rect(document.querySelector('.dialog-actions')),footer:rect(document.querySelector('.settings-footer')),
          pageOverflow:document.documentElement.scrollWidth>innerWidth||document.documentElement.scrollHeight>innerHeight,
          checked:document.querySelector('#source-enabled-freeSteamKeys').checked,unchecked:!document.querySelector('#source-enabled-zeros').checked,
          focusVisible:document.querySelector('#source-enabled-freeSteamKeys').matches(':focus-visible')};
      });
      assert.ok(Math.abs(validation.status.centerY-validation.actions.centerY)<=1,JSON.stringify(validation));
      assert.equal(validation.pageOverflow,false,JSON.stringify(validation));assert.equal(validation.checked,true);assert.equal(validation.unchecked,true);
      await app.browser.saveScreenshot(path.join(controlsReport,language+'-'+width+'x'+height+'-sources-dirty-validation.png'));
      await app.browser.$('#check-interval-freeSteamKeys').setValue(String(initial.settings.sourceCheckIntervals.freeSteamKeys));
      if(initial.settings.zerosEnabled)await app.browser.$('label[for="source-enabled-zeros"] > span').click();
      assert.equal(await app.browser.$('.dialog-actions .primary-button').isEnabled(),false);
      await selectSettingsSection(app.browser,'application');
      const trigger=app.browser.$('#autostart-target');await trigger.scrollIntoView({block:'center'});
      assert.equal(await trigger.isEnabled(),false,'the native e2e development build preserves its real autostart restriction');
      assert.equal(await app.browser.$('#start-at-login').isEnabled(),false);
      assert.equal(await app.browser.$('[role="listbox"]').isExisting(),false);
      await app.browser.saveScreenshot(path.join(controlsReport,language+'-'+width+'x'+height+'-application-disabled.png'));
      controlMeasurements.push({language,width,height,validation,application:{isDevelopment:true,autostartDisabled:true,popup:'not opened; selected/hover/keyboard states covered by real Chrome with fake release-mode IPC'}});
    }
  }
  await fs.writeFile(path.join(controlsReport,'geometry.json'),JSON.stringify(controlMeasurements,null,2)+'\n');
  await app.browser.$('button=English').click();await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.setWindowRect(originalRect.x,originalRect.y,originalRect.width,originalRect.height);
  await app.browser.$('.header-button').click();
  await app.browser.$('#check-interval-freeSteamKeys').setValue(String(initial.settings.sourceCheckIntervals.freeSteamKeys+1));
  await selectSettingsSection(app.browser,'network');
  assert.equal((await snapshot()).settings.sourceCheckIntervals.freeSteamKeys,initial.settings.sourceCheckIntervals.freeSteamKeys,'section navigation does not autosave');
  await selectSettingsSection(app.browser,'sources');
  assert.equal(await app.browser.$('#check-interval-freeSteamKeys').getValue(),String(initial.settings.sourceCheckIntervals.freeSteamKeys+1));
  await app.browser.keys('Escape');
  await app.browser.$('.settings-close-prompt').waitForDisplayed();
  await app.browser.saveScreenshot(path.join(report,'settings-unsaved-english.png'));
  assert.equal((await snapshot()).settings.sourceCheckIntervals.freeSteamKeys,initial.settings.sourceCheckIntervals.freeSteamKeys);
  await app.browser.$('button=Discard changes').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.$('.header-button').click();
  await selectSettingsSection(app.browser,'application');
  await app.browser.$('button=Русский').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  await app.browser.$('.header-button').click();
  await app.browser.$('#check-interval-freeSteamKeys').setValue(String(initial.settings.sourceCheckIntervals.freeSteamKeys+1));
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
  await selectSettingsSection(app.browser,'application');
  await app.browser.$('button=English').click();
  await app.browser.$('.dialog-actions .primary-button').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  const final=await snapshot();
  assert.equal(final.settings.sourceCheckIntervals.freeSteamKeys,initial.settings.sourceCheckIntervals.freeSteamKeys);
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
      await selectSettingsSection(app.browser,'application');
      for(const [id,value] of [['start-in-tray',startup],['keep-running-in-tray',close]]) {
        const input=await app.browser.$(`#${id}`);
        if(await input.isSelected()!==value) await input.click();
      }
      if(startup && close) {
        await app.browser.$('#start-in-tray').scrollIntoView({block:'center'});
        await app.browser.saveScreenshot(path.join(report,'settings-independent-tray-options.png'));
      }
      await saveSettings();
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
  await selectSettingsSection(app.browser,'application');
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

test('native log folder dialog cancels without edits, selects a draft and applies saved policy after restart',async()=>{
  await waitUntil(async()=>(await snapshot()).monitor.lastSuccessfulCheckUtc!==null,'initial baseline before log settings');
  const defaultDirectory=path.join(root,'logs');
  const defaultFile=path.join(defaultDirectory,'Giveaway Scout.log');
  const customDirectory=path.join(root,'custom-logs');
  await fs.mkdir(customDirectory);
  await fs.writeFile(path.join(customDirectory,'notes.txt'),'Foreign file must survive log cleanup.');
  const before=await snapshot();
  const initial=await loggingSnapshot();
  samePath(initial.activeFilePath,defaultFile);
  assert.equal(initial.maxFileSizeMb,2);
  assert.equal(initial.fileCount,1);
  assert.equal(initial.restartRequired,false);

  await openDiagnostics();
  const draft=()=>app.browser.$('[data-testid="log-directory-draft"]');
  const originalDraft=await draft().getText();
  const choose=app.browser.$('[data-testid="choose-log-directory"]');
  await choose.scrollIntoView({block:'center'});
  await Promise.all([flaui('--folder-dialog','cancel'),choose.click()]);
  await choose.waitForEnabled();
  assert.equal(await draft().getText(),originalDraft);
  assert.equal((await snapshot()).settings.logDirectory,null);
  await Promise.all([flaui('--folder-dialog','select'),choose.click()]);
  await choose.waitForEnabled();
  samePath(await draft().getText(),customDirectory);
  assert.match(await app.browser.$('.settings-draft-status').getText(),/unsaved/i);
  assert.equal((await snapshot()).settings.logDirectory,null);
  samePath((await loggingSnapshot()).activeFilePath,defaultFile);
  await app.browser.$('#log-file-size').setValue('1');
  await app.browser.$('#log-file-count').setValue('3');
  const open=app.browser.$('[data-testid="open-log-directory"]');
  await open.scrollIntoView({block:'center'});
  await flaui('--watch-log-folder','default',()=>open.click());
  await saveSettings();
  const saved=await snapshot();
  samePath(saved.settings.logDirectory,customDirectory);
  assert.equal(saved.settings.logMaxFileSizeMb,1);
  assert.equal(saved.settings.logFileCount,3);
  assert.deepEqual(saved.counts,before.counts);
  const pending=await loggingSnapshot();
  assert.equal(pending.restartRequired,true);
  assert.equal(pending.maxFileSizeMb,2);
  assert.equal(pending.fileCount,1);
  samePath(pending.activeFilePath,defaultFile);
  await app.browser.$('.logging-status').waitForDisplayed();
  await app.stop();app=null;
  const oldLog=await fs.readFile(defaultFile);
  app=await launchApp(root);
  await waitUntil(async()=>(await snapshot()).monitor.lastSuccessfulCheckUtc!==null,'custom log directory restart');
  const applied=await loggingSnapshot();
  samePath(applied.activeFilePath,path.join(customDirectory,'Giveaway Scout.log'));
  assert.equal(applied.maxFileSizeMb,1);
  assert.equal(applied.fileCount,3);
  assert.equal(applied.restartRequired,false);
  assert.equal(applied.lastError,null);
  assert.deepEqual(await fs.readFile(defaultFile),oldLog);
  await fs.access(applied.activeFilePath);
  await openDiagnostics();
  await app.browser.$('[data-testid="default-log-directory"]').scrollIntoView({block:'center'});
  await app.browser.$('[data-testid="default-log-directory"]').click();
  assert.equal(await draft().getText(),originalDraft);
  // A different unsaved folder must not affect what Open folder opens.
  await flaui('--watch-log-folder','custom',()=>app.browser.$('[data-testid="open-log-directory"]').click());
  await app.browser.keys('Escape');
  await app.browser.$('.settings-close-prompt').waitForDisplayed();
  await app.browser.$('button=Discard changes').click();
  await app.browser.$('[role="dialog"]').waitForExist({reverse:true});
  samePath((await snapshot()).settings.logDirectory,customDirectory);
  await fs.writeFile(path.join(report,'native-logging-state.json'),JSON.stringify({initial,pending,applied},null,2)+'\n');
},{timeout:120_000});

test('native Windows log rotation respects total file count and preserves other files and previous folders',async()=>{
  const directory=path.join(root,'custom-logs');
  const active=path.join(directory,'Giveaway Scout.log');
  const foreign=path.join(directory,'Giveaway Scout.backup.log');
  await fs.writeFile(foreign,'Foreign similarly-named file must remain.');
  const prefixed=marker=>{
    const data=Buffer.alloc(999_999,'.');data.write(marker);return data;
  };
  for(let cycle=1;cycle<=3;cycle++) {
    await app.stop();app=null;
    await fs.writeFile(active,prefixed(`NATIVE_ROTATION_${cycle}\n`));
    app=await launchApp(root);
    await waitUntil(async()=>{
      try {return (await fs.readFile(path.join(directory,'Giveaway Scout.1.log'),'utf8')).startsWith(`NATIVE_ROTATION_${cycle}\n`);}
      catch(error) {if(error.code==='ENOENT') return false;throw error;}
    },'startup message rotates a native file at the configured decimal MB limit');
    const state=await loggingSnapshot();
    assert.equal(state.fileCount,3);
    assert.equal(state.lastError,null);
    assert.ok((await fs.stat(active)).size<1_000_000);
  }
  assert.deepEqual(await logNames(directory),['Giveaway Scout.1.log','Giveaway Scout.2.log','Giveaway Scout.log']);
  assert.ok((await fs.readFile(path.join(directory,'Giveaway Scout.1.log'),'utf8')).startsWith('NATIVE_ROTATION_3\n'));
  assert.ok((await fs.readFile(path.join(directory,'Giveaway Scout.2.log'),'utf8')).startsWith('NATIVE_ROTATION_2\n'));
  await openDiagnostics();
  await app.browser.$('#log-file-count').setValue('1');
  await saveSettings();
  assert.equal((await loggingSnapshot()).restartRequired,true);
  await app.stop();app=null;
  await fs.writeFile(active,prefixed('NATIVE_SINGLE_FILE\n'));
  app=await launchApp(root);
  await waitUntil(async()=>!(await fs.readFile(active,'utf8')).startsWith('NATIVE_SINGLE_FILE\n'),'single-file rotation without an archive');
  assert.deepEqual(await logNames(directory),['Giveaway Scout.log']);
  assert.equal((await loggingSnapshot()).fileCount,1);
  assert.equal((await loggingSnapshot()).restartRequired,false);
  assert.equal(await fs.readFile(foreign,'utf8'),'Foreign similarly-named file must remain.');
  assert.equal(await fs.readFile(path.join(directory,'notes.txt'),'utf8'),'Foreign file must survive log cleanup.');

  await openDiagnostics();
  await app.browser.$('#log-file-size').setValue('2');
  await app.browser.$('[data-testid="default-log-directory"]').scrollIntoView({block:'center'});
  await app.browser.$('[data-testid="default-log-directory"]').click();
  await saveSettings();
  await app.stop();app=null;
  const previousFolderLog=await fs.readFile(active);
  app=await launchApp(root);
  await waitUntil(async()=>(await snapshot()).monitor.lastSuccessfulCheckUtc!==null,'standard log folder restored');
  const restored=await loggingSnapshot();
  samePath(restored.activeFilePath,path.join(root,'logs','Giveaway Scout.log'));
  assert.equal(restored.maxFileSizeMb,2);
  assert.equal(restored.fileCount,1);
  assert.equal(restored.restartRequired,false);
  assert.equal((await snapshot()).settings.logDirectory,null);
  assert.deepEqual(await fs.readFile(active),previousFolderLog);
  await fs.writeFile(path.join(report,'native-log-rotation.json'),JSON.stringify({files:await logNames(directory),restored},null,2)+'\n');
},{timeout:180_000});

test('independent source batches preserve page order, reverse notification order and older history across restart',async()=>{
  await app.stop();app=null;
  const orderingRoot=await createProfile();
  const orderingReport=path.join(report,'ordering');
  await fs.mkdir(orderingReport);
  let orderingApp;
  const invoke=command=>orderingApp.browser.execute(name=>window.__TAURI_INTERNALS__.invoke(name),command);
  const state=()=>invoke('get_storage_snapshot');
  const history=()=>invoke('get_notification_history');
  const requests=async()=> (await readEvents(orderingRoot)).filter(event=>event.kind==='notification');
  const titles=records=>records.map(record=>record.title);
  const requestTitles=async()=> (await requests()).map(event=>event.title.match(/Order (?:Givee|Farm) [A-D]$/)?.[0]);
  async function assertOrder(expectedHistory,expectedRequests) {
    const records=await history();
    assert.deepEqual(titles(records),expectedHistory);
    await waitUntil(async()=>JSON.stringify(await orderingApp.browser.execute(()=>
      [...document.querySelectorAll('.history-item strong')].map(item=>item.textContent)))===JSON.stringify(expectedHistory),'UI history matches persisted order');
    assert.deepEqual(await requestTitles(),expectedRequests);
    const events=await requests();
    assert.equal(new Set(events.map(event=>event.activation)).size,events.length);
    for(const event of events) {
      const id=event.activation?.match(/^giveaway-scout:\/\/notification\/([a-f0-9-]{36})$/)?.[1];
      assert.ok(records.some(record=>record.id===id),'each request belongs to a persisted notification');
    }
    return records;
  }
  async function checkOrdering(scenario) {
    await setScenario(orderingRoot,scenario);
    const before=(await state()).monitor.lastStartedAtUtc;
    await orderingApp.browser.$('.source-actions .primary-button').click();
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking && current.monitor.lastStartedAtUtc!==before;},'ordering check complete');
  }
  async function startOrdering() {
    orderingApp=await launchApp(orderingRoot);
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking && current.monitor.lastSuccessfulCheckUtc!==null;},'ordering baseline complete');
  }
  try {
    await startOrdering();
    assert.equal((await state()).counts.notificationHistory,0);
    assert.equal((await state()).counts.trackedGiveaways,2);
    await checkOrdering('orderFirst');
    const first=await history();
    const firstTitles=titles(first);
    const firstRequests=await requestTitles();
    assert.equal(first.length,6);
    for(const [source,prefix] of [['giveeClub','Givee'],['gamesForFarm','Farm']]) {
      assert.deepEqual(titles(first.filter(record=>record.source===source)),['A','B','C'].map(letter=>`Order ${prefix} ${letter}`));
      assert.deepEqual(firstRequests.filter(title=>title?.includes(prefix)),['C','B','A'].map(letter=>`Order ${prefix} ${letter}`));
    }
    await assertOrder(firstTitles,firstRequests);
    assert.equal((await state()).counts.trackedGiveaways,8);
    await orderingApp.stop();orderingApp=null;
    await startOrdering();
    assert.deepEqual(await assertOrder(firstTitles,firstRequests),first);
    await checkOrdering('orderSecond');
    const second=await history();
    assert.deepEqual(new Set(titles(second.slice(0,2))),new Set(['Order Givee D','Order Farm D']));
    assert.deepEqual(second.slice(2),first,'each newly completed batch is above old history');
    const secondRequests=await requestTitles();
    assert.deepEqual(secondRequests.slice(0,6),firstRequests);
    assert.deepEqual(new Set(secondRequests.slice(6)),new Set(['Order Givee D','Order Farm D']));
    await assertOrder(titles(second),secondRequests);
    assert.equal((await state()).counts.trackedGiveaways,10);
    await checkOrdering('orderSecond');
    assert.deepEqual(await assertOrder(titles(second),secondRequests),second);
    await orderingApp.stop();orderingApp=null;
    await startOrdering();
    assert.deepEqual(await assertOrder(titles(second),secondRequests),second);
    await fs.writeFile(path.join(orderingReport,'notification-order.json'),JSON.stringify({
      firstHistory:first,secondHistory:second,requests:await requests(),
      evidence:'Per-source application request order from the isolated notification stub; independent completion order across sources is not publication chronology.',
    },null,2)+'\n');
  } finally {
    try {if(orderingApp) await orderingApp.stop();}
    finally {await preserveProfile(orderingRoot,orderingReport);}
  }
},{timeout:180_000});

test('FreeSteamKeys uses the API parser, persists its toggle, reports failures and opens each new Steam giveaway once',async()=>{
  if(app) {await app.stop();app=null;}
  const sourceRoot=await createProfile();
  const sourceReport=path.join(report,'free-steam-keys');
  await fs.mkdir(sourceReport);
  let sourceApp;
  const invoke=command=>sourceApp.browser.execute(name=>window.__TAURI_INTERNALS__.invoke(name),command);
  const state=()=>invoke('get_storage_snapshot');
  const history=()=>invoke('get_notification_history');
  const sourceRow=()=>sourceApp.browser.$('[data-source="freeSteamKeys"] .source-row');
  async function start() {
    sourceApp=await launchApp(sourceRoot);
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.monitor.lastSuccessfulCheckUtc!==null;},'FreeSteamKeys profile baseline');
  }
  async function sourceCheck() {
    const before=(await state()).monitor.lastStartedAtUtc;
    const button=sourceApp.browser.$('.source-actions .primary-button');
    await button.waitForEnabled();await button.click();
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.monitor.lastStartedAtUtc!==before;},'FreeSteamKeys check complete');
    await button.waitForEnabled();
  }
  try {
    await start();
    assert.equal((await state()).settings.freeSteamKeysEnabled,true);
    assert.match(await sourceRow().getText(),/FreeSteamKeys/);
    assert.equal(await sourceRow().getAttribute('aria-checked'),'true');
    await sourceRow().click();
    await waitUntil(async()=>!(await state()).settings.freeSteamKeysEnabled,'FreeSteamKeys disabled');
    await sourceApp.stop();sourceApp=null;
    await start();
    assert.equal((await state()).settings.freeSteamKeysEnabled,false);
    await setScenario(sourceRoot,'freeSteamKeysFailure');
    await sourceCheck();
    assert.deepEqual((await state()).monitor.sourceFailures,[],'disabled FreeSteamKeys is not fetched');
    await sourceRow().click();
    await waitUntil(async()=>(await state()).settings.freeSteamKeysEnabled,'FreeSteamKeys enabled');
    await sourceCheck();
    const failed=(await state()).monitor.sourceFailures;
    assert.equal(failed[0].source,'freeSteamKeys');
    assert.deepEqual(failed[0].reason,{code:'http',status:503});
    await waitUntil(async()=>(await sourceApp.browser.$('[data-source="freeSteamKeys"] .source-phase').getText())==='Error','FreeSteamKeys failure shown');
    await setScenario(sourceRoot,'freeSteamKeysGiveaway');
    await sourceCheck();
    const records=await history();
    assert.deepEqual(records.map(record=>record.title),['Sorcerer Adventure','One Gun Guy']);
    assert.ok(records.every(record=>record.source==='freeSteamKeys'&&record.store==='steam'));
    assert.deepEqual((await state()).monitor.sourceFailures,[]);
    await waitUntil(async()=>(await readEvents(sourceRoot)).filter(event=>event.kind==='notification').length===2,'FreeSteamKeys notifications');
    await sourceCheck();
    assert.equal((await history()).length,2);
    assert.equal((await readEvents(sourceRoot)).filter(event=>event.kind==='notification').length,2);
    await sourceApp.browser.$('.history-item:first-child .history-open').click();
    await waitUntil(async()=>(await readEvents(sourceRoot)).some(event=>event.kind==='openUrl'),'FreeSteamKeys link opened');
    assert.equal((await readEvents(sourceRoot)).filter(event=>event.kind==='openUrl').at(-1).url,'https://www.freesteamkeys.com/open/sorcerer-adventure');
    await sourceApp.browser.saveScreenshot(path.join(sourceReport,'history.png'));
    await sourceApp.stop();sourceApp=null;
    await start();
    assert.equal((await state()).settings.freeSteamKeysEnabled,true);
    assert.equal((await history()).length,2);
    assert.equal((await readEvents(sourceRoot)).filter(event=>event.kind==='notification').length,2,'restart does not repeat notifications');
    await fs.writeFile(path.join(sourceReport,'result.json'),JSON.stringify({records,events:await readEvents(sourceRoot)},null,2)+'\n');
  } catch(error) {
    if(sourceApp) await sourceApp.browser.saveScreenshot(path.join(sourceReport,'failure.png')).catch(()=>{});
    throw error;
  } finally {
    try {if(sourceApp) await sourceApp.stop();}
    finally {await preserveProfile(sourceRoot,sourceReport);}
  }
},{timeout:150_000});

test('cross-source campaigns stay visible after reading and open from the date or card padding',async()=>{
  const campaignRoot=await createProfile();
  const campaignReport=path.join(report,'cross-source-history');
  await fs.mkdir(campaignReport);
  let campaignApp;
  const invoke=(command,args={})=>campaignApp.browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const state=()=>invoke('get_storage_snapshot');
  const history=()=>invoke('get_notification_history');
  const checkCampaign=async()=>{
    await invoke('check_now');
    await waitUntil(async()=>!(await state()).monitor.isChecking,'campaign source checks complete');
  };
  try {
    campaignApp=await launchApp(campaignRoot);
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.monitor.lastSuccessfulCheckUtc;},'campaign baseline');
    const initial=await state();
    await invoke('update_settings',{settings:{...initial.settings,giveeClubEnabled:false}});
    await setScenario(campaignRoot,'crossSourceGiveaway');
    await checkCampaign();
    await campaignApp.browser.$('.history-item').waitForDisplayed();
    const first=(await history())[0];
    assert.equal((await history()).length,1);
    assert.equal(first.originSource,'giveeClub');
    assert.deepEqual(first.detectedSources,['freeSteamKeys']);
    assert.equal(new URL(first.url).hostname,'givee.club');
    assert.ok(new URL(first.url).pathname.endsWith('/event/900003'));
    assert.equal((await state()).settings.giveeClubEnabled,false);
    assert.match(await campaignApp.browser.$('.history-item small').getText(),/Givee Club.*FreeSteamKeys/);
    assert.equal(await campaignApp.browser.$('input[type="search"]').isExisting(),false);
    assert.equal(await campaignApp.browser.$('.history-filters, .history-unread-filter, .history-filter-summary, .filter-reset').isExisting(),false);
    assert.equal(await campaignApp.browser.$('.history-card [role="combobox"], .history-card input').isExisting(),false);
    assert.equal(await campaignApp.browser.$('.history-item-title .unread-dot').isExisting(),true);
    assert.equal(await campaignApp.browser.$('.history-item-actions .unread-dot').isExisting(),false);
    const unreadBackground=await campaignApp.browser.execute(()=>getComputedStyle(document.querySelector('.history-item')).backgroundColor);
    const beforeOpen=(await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').length;
    await campaignApp.browser.$('.history-item .mark-read-button').click();
    await waitUntil(async()=>(await state()).counts.unreadNotifications===0,'one campaign read');
    await campaignApp.browser.$('.history-item .unread-dot').waitForExist({reverse:true});
    assert.equal((await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').length,beforeOpen);
    await campaignApp.browser.$('.history-item').waitForDisplayed();
    assert.equal((await campaignApp.browser.$$('.history-item')).length,1);
    assert.equal((await campaignApp.browser.$('.history-item').getAttribute('class')).split(/\s+/).includes('unread'),false);
    assert.notEqual(await campaignApp.browser.execute(()=>getComputedStyle(document.querySelector('.history-item')).backgroundColor),unreadBackground);
    const readAt=(await history())[0].readAtUtc;
    assert.ok(readAt);
    const settings=(await state()).settings;
    await invoke('update_settings',{settings:{...settings,giveeClubEnabled:true}});
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.monitor.sourceStates.find(source=>source.source==='giveeClub').phase==='ok';},'Givee immediate check');
    const combined=await history();
    assert.equal(combined.length,1);
    assert.equal(combined[0].title,'E2E new Steam game');
    assert.deepEqual(new Set(combined[0].detectedSources),new Set(['freeSteamKeys','giveeClub']));
    assert.equal(combined[0].readAtUtc,readAt);
    assert.equal((await readEvents(campaignRoot)).filter(event=>event.kind==='notification').length,1);
    await clickCardArea(campaignApp.browser,'.history-item time');
    await waitUntil(async()=>(await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').length===beforeOpen+1,'canonical origin opened');
    const opened=(await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').at(-1);
    assert.equal(opened.url,combined[0].url);
    await clickCardArea(campaignApp.browser,'.history-item',true);
    await waitUntil(async()=>(await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').length===beforeOpen+2,'card lower padding opens once');
    assert.equal((await readEvents(campaignRoot)).filter(event=>event.kind==='openUrl').at(-1).url,combined[0].url);
    assert.equal((await history())[0].readAtUtc,readAt,'opening an already read card preserves its original read timestamp');
    await campaignApp.browser.saveScreenshot(path.join(campaignReport,'history.png'));
    await fs.writeFile(path.join(campaignReport,'result.json'),JSON.stringify({first,combined,events:await readEvents(campaignRoot)},null,2)+'\n');
  } finally {
    try {if(campaignApp) await campaignApp.stop();}
    finally {await preserveProfile(campaignRoot,campaignReport);}
  }
},{timeout:90_000});

test('a slow source does not delay fast history, manual checks coalesce and cleanup cancels in-flight results',async()=>{
  const monitorRoot=await createProfile();
  const monitorReport=path.join(report,'independent-monitor');
  await fs.mkdir(monitorReport);
  let monitorApp;
  const invoke=(command,args={})=>monitorApp.browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const state=()=>invoke('get_storage_snapshot');
  try {
    monitorApp=await launchApp(monitorRoot);
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.monitor.lastSuccessfulCheckUtc;},'independent baseline');
    await setScenario(monitorRoot,'slowGivee');
    const started=await invoke('check_now');
    assert.deepEqual(started.startedSources,['freeSteamKeys','giveeClub','gamesForFarm']);
    const second=await invoke('check_now');
    assert.ok(second.alreadyRunningSources.includes('giveeClub'));
    await waitUntil(async()=>(await state()).counts.notificationHistory===2,'fast FreeSteamKeys history while Givee waits',3_000);
    const during=await state();
    const slow=during.monitor.sourceStates.find(source=>source.source==='giveeClub');
    const fast=during.monitor.sourceStates.find(source=>source.source==='freeSteamKeys');
    assert.equal(slow.phase,'checking');
    assert.equal(slow.nextCheckAtUtc,null,'running source is scheduled after its completion');
    assert.equal(fast.phase,'ok');
    const intervalMs=during.settings.sourceCheckIntervals.freeSteamKeys*60_000;
    assert.ok(Math.abs(Date.parse(fast.nextCheckAtUtc)-Date.parse(fast.lastCompletedAtUtc)-intervalMs)<1_000,'completed source keeps its independent schedule in the backend');
    assert.ok(fast.lastSuccessfulCheckUtc,'source success remains recorded after its UI timestamps are removed');
    await setScenario(monitorRoot,'baseline');
    await invoke('clear_giveaway_data');
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.counts.trackedGiveaways===2;},'silent baseline after cleanup');
    await monitorApp.browser.pause(4_500);
    const cleared=await state();
    assert.equal(cleared.counts.notificationHistory,0);
    assert.equal(cleared.counts.unreadNotifications,0);
    assert.equal(cleared.counts.trackedGiveaways,2,'stale delayed giveaway was not restored');
    assert.equal((await readEvents(monitorRoot)).filter(event=>event.kind==='notification').length,2,'cleanup and delayed result add no delivery');
    await fs.writeFile(path.join(monitorReport,'result.json'),JSON.stringify({started,second,during,cleared},null,2)+'\n');
  } finally {
    try {if(monitorApp) await monitorApp.stop();}
    finally {await preserveProfile(monitorRoot,monitorReport);}
  }
},{timeout:90_000});

test('native WebView2 layout keeps source and history controls reachable at original and minimum sizes with an update banner',async()=>{
  const layoutRoot=await createProfile();
  const layoutReport=path.join(report,'native-layout');
  await fs.mkdir(layoutReport);
  const measurements=[];
  let layoutApp;
  const invoke=(command,args={})=>layoutApp.browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const state=()=>invoke('get_storage_snapshot');
  try {
    layoutApp=await launchApp(layoutRoot);
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.monitor.lastSuccessfulCheckUtc;},'layout baseline');
    const initial=await state();
    await invoke('update_settings',{settings:{...initial.settings,zerosEnabled:true}});
    await waitUntil(async()=>!(await state()).monitor.isChecking,'all four layout sources settled');
    await setScenario(layoutRoot,'orderSecond');
    await invoke('check_now');
    await waitUntil(async()=>{const snapshot=await state();return !snapshot.monitor.isChecking&&snapshot.counts.notificationHistory>0;},'layout history from real source fixtures');
    await layoutApp.browser.$('.history-item:first-child').waitForDisplayed();
    // Display-only Rust fixture through the real IPC in this validated profile.
    // The actual update service stays in development mode with no pending package.
    await fs.writeFile(path.join(layoutRoot,'update-banner-fixture'),'native layout only\n');
    const update=await invoke('get_update_state');
    assert.equal(update.mode,'enabled');
    assert.equal(update.candidate.token,'layout-only-no-install');
    await layoutApp.browser.$('.update-banner').waitForDisplayed();
    const originalRect=await layoutApp.browser.getWindowRect();
    const originalInner=await layoutApp.browser.execute(()=>({width:innerWidth,height:innerHeight}));
    const chromeWidth=originalRect.width-originalInner.width;
    const chromeHeight=originalRect.height-originalInner.height;
    for(const language of ['en','ru']) {
      const settings=(await state()).settings;
      await invoke('update_settings',{settings:{...settings,languageCode:language}});
      await waitUntil(async()=>await layoutApp.browser.execute(()=>document.documentElement.lang)===language,'native layout language '+language);
      for(const [width,height] of [[1100,740],[960,600],[760,600]]) {
        await layoutApp.browser.setWindowRect(originalRect.x,originalRect.y,width+chromeWidth,height+chromeHeight);
        await waitUntil(async()=>await layoutApp.browser.execute(({width,height})=>innerWidth===width&&innerHeight===height,{width,height}),'exact native client size '+width+'x'+height);
        const geometry=await layoutApp.browser.execute(()=>{
          const bounds=element=>{const rect=element.getBoundingClientRect();return{top:rect.top,bottom:rect.bottom,left:rect.left,right:rect.right,width:rect.width,height:rect.height};};
          const controls=['.update-banner','.status-panel','.source-actions','.history-toolbar','.update-banner-action','.update-banner-dismiss'].map(selector=>({selector,...bounds(document.querySelector(selector))}));
          const sourceList=document.querySelector('.source-list');sourceList.scrollTop=sourceList.scrollHeight;
          const lastSource=bounds(document.querySelector('[data-source="zeros"] .source-row'));
          const sourceBounds=bounds(sourceList);
          const historyList=document.querySelector('.history-list');historyList.scrollTop=historyList.scrollHeight;
          const lastHistory=bounds(historyList.lastElementChild);
          const historyBounds=bounds(historyList);
          const measurement={language:document.documentElement.lang,width:innerWidth,height:innerHeight,devicePixelRatio,
            pageScroll:document.documentElement.scrollHeight>innerHeight,horizontalScroll:document.documentElement.scrollWidth>innerWidth,
            sourceNames:[...document.querySelectorAll('.source-row strong')].map(element=>element.textContent),
            controls,lastSource,sourceBounds,lastHistory,historyBounds,historyHeight:historyList.clientHeight};
          sourceList.scrollTop=0;historyList.scrollTop=0;
          return measurement;
        });
        measurements.push(geometry);
        await fs.writeFile(path.join(layoutReport,'bounds.json'),JSON.stringify({banner:'display-only Rust fixture through real IPC; real WebView2 and OS window resizing',measurements},null,2)+'\n');
        await layoutApp.browser.saveScreenshot(path.join(layoutReport,language+'-'+width+'x'+height+'.png'));
        assert.equal(geometry.width,width);
        assert.equal(geometry.height,height);
        assert.equal(geometry.pageScroll,false,JSON.stringify(geometry));
        assert.equal(geometry.horizontalScroll,false,JSON.stringify(geometry));
        assert.deepEqual(geometry.sourceNames,['FreeSteamKeys','Givee Club','Games for Farm','Zeros']);
        assert.ok(geometry.controls.every(rect=>rect.top>=0&&rect.bottom<=geometry.height&&rect.left>=0&&rect.right<=geometry.width),JSON.stringify(geometry));
        assert.ok(geometry.controls[0].bottom<=geometry.controls[1].top,JSON.stringify(geometry));
        assert.ok(geometry.controls[4].right<=geometry.controls[5].left,JSON.stringify(geometry));
        assert.ok(geometry.controls[5].width<=40,JSON.stringify(geometry));
        const dismiss=layoutApp.browser.$('.update-banner-dismiss');
        assert.equal(await dismiss.getAttribute('aria-label'),language==='ru'?'Скрыть уведомление':'Dismiss update notification');
        assert.equal(await dismiss.getAttribute('title'),language==='ru'?'Скрыть уведомление':'Dismiss update notification');
        assert.ok(geometry.lastSource.top>=geometry.sourceBounds.top&&geometry.lastSource.bottom<=geometry.sourceBounds.bottom+1,JSON.stringify(geometry));
        assert.ok(geometry.lastHistory.bottom<=geometry.historyBounds.bottom+1,JSON.stringify(geometry));
        assert.ok(geometry.historyHeight>=80,JSON.stringify(geometry));
        assert.equal(await layoutApp.browser.$('.source-actions .primary-button').isEnabled(),true);
        assert.equal(await layoutApp.browser.$('.sources-card .test-notification-button').isExisting(),false);
        await layoutApp.browser.$('.header-button').click();await selectSettingsSection(layoutApp.browser,'updatesData');
        await layoutApp.browser.saveScreenshot(path.join(layoutReport,language+'-'+width+'x'+height+'-updates-settings.png'));
        await layoutApp.browser.$('.dialog-actions .secondary-button').click();
        await layoutApp.browser.$('[role="dialog"]').waitForExist({reverse:true});
      }
    }
    await layoutApp.browser.$('.header-button').click();await selectSettingsSection(layoutApp.browser,'updatesData');
    const automaticChecks=layoutApp.browser.$('#automatic-update-checks');
    assert.equal(await automaticChecks.isSelected(),true);
    await automaticChecks.click();
    assert.equal((await state()).settings.automaticUpdateChecks,true,'native settings draft is not saved immediately');
    await layoutApp.browser.$('.dialog-actions .primary-button').click();
    await layoutApp.browser.$('[role="dialog"]').waitForExist({reverse:true});
    assert.equal((await state()).settings.automaticUpdateChecks,false);
    await layoutApp.browser.$('.update-banner-dismiss').click();
    await layoutApp.browser.$('.update-banner').waitForExist({reverse:true});
    assert.equal((await invoke('get_update_state')).bannerDismissed,true);
    await layoutApp.browser.$('.header-button').click();await selectSettingsSection(layoutApp.browser,'updatesData');
    assert.equal(await layoutApp.browser.$('.update-notes').getText(),update.candidate.notes);
    assert.equal(await layoutApp.browser.$('.update-candidate .primary-button').isEnabled(),true);
    await layoutApp.browser.saveScreenshot(path.join(layoutReport,'ru-dismissed-updates-settings.png'));
    await layoutApp.stop();layoutApp=null;
    layoutApp=await launchApp(layoutRoot);
    await layoutApp.browser.$('.header-button').waitForEnabled();
    assert.equal((await state()).settings.automaticUpdateChecks,false,'automatic checks preference survives a native restart');
    assert.equal((await invoke('get_update_state')).bannerDismissed,true,'version dismissal survives a native restart');
    assert.equal(await layoutApp.browser.$('.update-banner').isExisting(),false);
    await layoutApp.browser.$('.header-button').click();await selectSettingsSection(layoutApp.browser,'updatesData');
    assert.equal(await layoutApp.browser.$('#automatic-update-checks').isSelected(),false);
    assert.equal(await layoutApp.browser.$('.update-candidate').isDisplayed(),true);
    await layoutApp.browser.saveScreenshot(path.join(layoutReport,'ru-restarted-updates-settings.png'));
    assert.equal((await readEvents(layoutRoot)).some(event=>event.kind==='openUrl'),false,'layout test opens no external URLs');
  } catch(error) {
    if(layoutApp) await layoutApp.browser.saveScreenshot(path.join(layoutReport,'failure.png')).catch(()=>{});
    throw error;
  } finally {
    try {if(layoutApp) await layoutApp.stop();}
    finally {await preserveProfile(layoutRoot,layoutReport);}
  }
},{timeout:120_000});

test('polished native layout and one localized Games for Farm error work in both languages',async()=>{
  const polishRoot=await createProfile(),polishReport=path.join(report,'ui-polish');
  await fs.mkdir(polishReport);
  let polish;
  const invoke=(command,args={})=>polish.browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const state=()=>invoke('get_storage_snapshot');
  const measurements=[];
  try {
    polish=await launchApp(polishRoot);
    await waitUntil(async()=>{const s=await state();return !s.monitor.isChecking&&s.monitor.lastSuccessfulCheckUtc;},'polish baseline');
    await setScenario(polishRoot,'orderSecond');
    await invoke('check_now');
    await waitUntil(async()=>{const s=await state();return !s.monitor.isChecking&&s.counts.notificationHistory>0;},'polish history');
    await setScenario(polishRoot,'gamesForFarmFailure');
    await invoke('check_now');
    await waitUntil(async()=>{const s=await state();return !s.monitor.isChecking&&s.monitor.sourceStates.some(source=>source.source==='gamesForFarm'&&source.lastError);},'GFF failure');
    await fs.writeFile(path.join(polishRoot,'update-banner-fixture'),'native polish only\n');
    await invoke('get_update_state');
    await polish.browser.$('.update-banner').waitForDisplayed();
    const original=await polish.browser.getWindowRect();
    const inner=await polish.browser.execute(()=>({width:innerWidth,height:innerHeight}));
    for(const language of ['en','ru']) {
      const current=await state();
      await invoke('update_settings',{settings:{...current.settings,languageCode:language}});
      await waitUntil(async()=>await polish.browser.execute(()=>document.documentElement.lang)===language,'polish language');
      for(const [width,height] of [[1100,740],[760,600]]) {
        await polish.browser.setWindowRect(original.x,original.y,width+original.width-inner.width,height+original.height-inner.height);
        const details=polish.browser.$('[data-source="gamesForFarm"] .source-details');
        await details.scrollIntoView({block:'center'});
        if(await details.getAttribute('open')===null) await details.$('summary').click();
        const text=await details.$('.setting-error').getText();
        assert.equal(text,language==='ru'?'Не удалось подключиться к источнику.':'Could not connect to the source.');
        assert.equal(await details.$$('p').length,1);
        assert.equal(await details.$('.source-diagnostic').isExisting(),false);
        assert.equal(await polish.browser.execute(()=>document.body.textContent.includes('connection reset by peer')),false);
        const failure=(await state()).monitor.sourceStates.find(source=>source.source==='gamesForFarm').lastError;
        assert.match(failure.message,/freeanywhere\.net/);
        assert.match(failure.message,/connection reset by peer \(os error 10054\)/);
        // Window resizing can leave the pointer over only one scrollbar thumb.
        // Sample both in their neutral state instead of comparing mixed states.
        await polish.browser.$('.status-panel').moveTo();
        await polish.browser.pause(120);
        const geometry=await polish.browser.execute(()=>{
          const list=document.querySelector('.source-list'),actions=document.querySelector('.source-actions'),history=document.querySelector('.history-list');
          const rect=element=>{const r=element.getBoundingClientRect();return {left:r.left,top:r.top,right:r.right,bottom:r.bottom};};
          const scrollbar=element=>({thumb:getComputedStyle(element,'::-webkit-scrollbar-thumb').backgroundColor,width:getComputedStyle(element,'::-webkit-scrollbar').width,arrows:getComputedStyle(element,'::-webkit-scrollbar-button').display});
          const first=document.querySelector('.history-item-layout');
          const markers=[...document.querySelectorAll('.history-item')].map(item=>{
            const dot=item.querySelector('.unread-dot'),title=item.querySelector('.history-item-title strong');
            return {unread:item.classList.contains('unread'),dotInTitle:Boolean(item.querySelector('.history-item-title .unread-dot')),
              marker:dot?rect(dot):null,title:rect(title),borders:['Top','Right','Bottom','Left'].map(side=>getComputedStyle(item)['border'+side+'Width'])};
          });
          const sourceWidth=document.querySelector('.sources-card').getBoundingClientRect().width,historyWidth=document.querySelector('.history-card').getBoundingClientRect().width;
          const font=selector=>parseFloat(getComputedStyle(document.querySelector(selector)).fontSize);
          return {width:innerWidth,height:innerHeight,language:document.documentElement.lang,gap:rect(actions).top-rect(list).bottom,sourceScroll:scrollbar(list),historyScroll:scrollbar(history),main:rect(first.querySelector('.history-open')),side:rect(first.querySelector('.history-item-actions')),sourceShare:sourceWidth/(sourceWidth+historyWidth),sourceListBorders:['Top','Right','Bottom','Left'].map(side=>parseFloat(getComputedStyle(list)['border'+side+'Width'])),fonts:{sourceTitle:font('.source-row strong'),sourceMetadata:font('.source-row div > span'),sourceStatus:font('.source-phase'),historyTitle:font('.history-item strong'),historyMetadata:font('.history-item small'),historyDate:font('.history-item time')},mainTestButton:document.querySelector('.sources-card .test-notification-button')!==null,sourceTimes:document.querySelectorAll('.source-list time').length,searchFields:document.querySelectorAll('input[type="search"]').length,historyFilterControls:document.querySelectorAll('.history-filters, .history-unread-filter, .history-filter-summary, .filter-reset, .history-card [role="combobox"], .history-card input').length,historyCount:document.querySelectorAll('.history-item').length,markers,intervals:[...document.querySelectorAll('.source-list .source-interval')].map(element=>element.textContent),phases:[...document.querySelectorAll('.source-list .source-phase')].map(element=>element.textContent),overflow:document.documentElement.scrollWidth>innerWidth||document.documentElement.scrollHeight>innerHeight};
        });
        measurements.push(geometry);
        assert.ok(geometry.gap>=12,JSON.stringify(geometry));
        assert.deepEqual(geometry.sourceScroll,geometry.historyScroll);
        assert.equal(geometry.sourceScroll.arrows,'none');
        assert.equal(geometry.overflow,false,JSON.stringify(geometry));
        assert.equal(geometry.mainTestButton,false);
        assert.ok(geometry.sourceListBorders.every(width=>width===0),JSON.stringify(geometry));
        for(const [name,font] of Object.entries(geometry.fonts)) assert.ok(font>=(name.endsWith('Title')?14:12),`${name}: ${font}px`);
        assert.equal(geometry.sourceTimes,0);
        assert.equal(geometry.searchFields,0);
        assert.equal(geometry.historyFilterControls,0);
        assert.equal(geometry.historyCount,(await state()).counts.notificationHistory);
        assert.ok(geometry.markers.every(item=>item.unread===item.dotInTitle),JSON.stringify(geometry.markers));
        for(const item of geometry.markers.filter(item=>item.unread)) {
          assert.ok(item.marker.right<=item.title.left-4&&item.title.left-item.marker.right<=12,JSON.stringify(item));
          assert.ok(item.marker.top>=item.title.top&&item.marker.bottom<=item.title.bottom,JSON.stringify(item));
          assert.equal(new Set(item.borders).size,1,'native unread state uses no thicker left stripe');
        }
        assert.equal(geometry.intervals.length,0);
        assert.equal(geometry.phases.length,4);
        assert.ok(geometry.phases.includes(language==='ru'?'Ошибка':'Error'),JSON.stringify(geometry));
        if(width===1100) {
          assert.ok(geometry.sourceShare>=0.28&&geometry.sourceShare<=0.35,JSON.stringify(geometry));
          assert.ok(geometry.side.left>=geometry.main.right-1,JSON.stringify(geometry));
        }
        await polish.browser.saveScreenshot(path.join(polishReport,`${language}-${width}x${height}.png`));
      }
    }
    await setScenario(polishRoot,'baseline');
    await invoke('check_now');
    await waitUntil(async()=>{const s=await state();return !s.monitor.isChecking&&s.monitor.sourceStates.find(source=>source.source==='gamesForFarm').lastError===null;},'GFF recovery');
    await polish.browser.$('[data-source="gamesForFarm"] .source-details').waitForExist({reverse:true});
    const beforeTest=(await readEvents(polishRoot)).filter(event=>event.kind==='notification').length;
    const beforeHistory=(await state()).counts.notificationHistory;
    await polish.browser.$('.header-button').click();
    await selectSettingsSection(polish.browser,'notifications');
    const testButton=polish.browser.$('.notification-test .test-notification-button');
    await testButton.scrollIntoView({block:'center'});
    await testButton.click();
    await polish.browser.$('.notification-test [role="status"]').waitForDisplayed();
    await waitUntil(async()=>(await readEvents(polishRoot)).filter(event=>event.kind==='notification').length===beforeTest+1,'test notification from native settings');
    assert.equal((await readEvents(polishRoot)).filter(event=>event.kind==='notification').at(-1).title,'Тестовое уведомление Giveaway Scout');
    assert.equal((await state()).counts.notificationHistory,beforeHistory,'testing notifications does not add a giveaway to history');
    await polish.browser.keys('Escape');
    await polish.browser.$('[role="dialog"]').waitForExist({reverse:true});
    await fs.writeFile(path.join(polishReport,'geometry.json'),JSON.stringify(measurements,null,2)+'\n');
    assert.equal((await readEvents(polishRoot)).some(event=>event.kind==='openUrl'),false);
  } catch(error) {
    if(polish) await polish.browser.saveScreenshot(path.join(polishReport,'failure.png')).catch(()=>{});
    throw error;
  } finally {
    try {if(polish) await polish.stop();} finally {await preserveProfile(polishRoot,polishReport);}
  }
},{timeout:120_000});

test('real window focus and main-thread database polling cannot deadlock on source completion',async()=>{
  const raceRoot=await createProfile(),raceReport=path.join(report,'attention-storage-regression');
  await fs.mkdir(raceReport);
  let raceApp;
  const invoke=(command,args={})=>raceApp.browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const state=()=>invoke('get_storage_snapshot');
  const rounds=[];
  try {
    raceApp=await launchApp(raceRoot);
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.monitor.lastSuccessfulCheckUtc;},'attention regression silent baseline');
    assert.equal((await state()).counts.notificationHistory,0);
    await setScenario(raceRoot,'newGiveaway');
    for(let round=0;round<3;round++) {
      const token=await armAttentionRace(raceRoot);
      const started=await invoke('check_now');
      assert.deepEqual(started.startedSources,['freeSteamKeys','giveeClub','gamesForFarm']);
      // Wait through the profile file, not WebDriver: the historic failure
      // blocks the UI thread. The in-process 20s watchdog exits only this owned
      // test child, leaving no stuck WebView or background process on failure.
      await waitUntil(async()=>{
        if(raceApp.child.exitCode!==null||raceApp.child.signalCode!==null) throw new Error(`Attention regression process exited ${raceApp.child.exitCode}; inspect owned profile events`);
        const events=await readEvents(raceRoot);
        const failure=events.find(event=>event.kind==='attentionRaceFailed'&&event.token===token);
        if(failure) throw new Error(failure.message);
        return events.some(event=>event.kind==='attentionRaceCompleted'&&event.token===token);
      },'real focus response after main-thread storage poll',25_000);
      const events=(await readEvents(raceRoot)).filter(event=>event.token===token);
      assert.deepEqual(events.map(event=>event.kind),[
        'attentionRaceStarted','attentionRaceMainThreadEntered','attentionRaceMainThreadPolled','attentionRaceCompleted',
      ]);
      assert.equal(events[1].backgroundWorker,true,'actual source worker differs from the polling window thread');
      assert.equal(typeof events.at(-1).focused,'boolean');
      await waitUntil(async()=>!(await state()).monitor.isChecking,'all source completions after attention race');
      const current=await state();
      for(const source of started.startedSources) assert.equal(current.monitor.sourceStates.find(status=>status.source===source)?.phase,'ok',source+' completed');
      assert.equal(current.counts.notificationHistory,1);
      assert.equal(current.counts.unreadNotifications,1);
      assert.equal((await invoke('get_notification_history')).length,1);
      assert.equal((await invoke('get_logging_state')).lastError,null);
      rounds.push({token,started,events,snapshot:current});
    }
    assert.equal((await readEvents(raceRoot)).filter(event=>event.kind==='notification').length,1,'source races do not duplicate delivery');
    const queriesBeforeRead=(await readEvents(raceRoot)).filter(event=>event.kind==='attentionWindowQuery').length;
    await raceApp.browser.$('.history-item.unread .mark-read-button').click();
    await raceApp.browser.$('.history-item.unread').waitForExist({reverse:true});
    assert.equal((await state()).counts.unreadNotifications,0);
    const history=await invoke('get_notification_history');
    assert.ok(history[0].readAtUtc,'successful mark-read persists its timestamp');
    const events=await readEvents(raceRoot);
    assert.equal(events.filter(event=>event.kind==='attention').at(-1).unread,0);
    assert.ok(events.filter(event=>event.kind==='attentionWindowQuery').length>queriesBeforeRead,'mark-read also uses the real window focus path');
    assert.equal(events.some(event=>event.kind==='openUrl'),false,'mark-read does not launch a browser');
    await raceApp.browser.$('.header-button').click();
    await raceApp.browser.$('[role="dialog"]').waitForDisplayed();
    await raceApp.browser.keys('Escape');
    await raceApp.browser.$('[role="dialog"]').waitForExist({reverse:true});
    await raceApp.browser.saveScreenshot(path.join(raceReport,'responsive-after-completions.png'));
    await fs.writeFile(path.join(raceReport,'result.json'),JSON.stringify({rounds,history,events},null,2)+'\n');
  } finally {
    try {if(raceApp) await raceApp.stop();}
    finally {await preserveProfile(raceRoot,raceReport);}
  }
},{timeout:120_000});

test('native paged history virtualizes 240 parsed campaigns and preserves an older viewport on arrival',async()=>{
  const historyRoot=await createProfile(),historyReport=path.join(report,'large-paged-history');
  await fs.mkdir(historyReport);
  let historyApp;
  try {
    historyApp=await launchApp(historyRoot);
    const browser=historyApp.browser;
    const invoke=(command,args)=>browser.execute(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
    const state=()=>invoke('get_storage_snapshot');
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.monitor.lastSuccessfulCheckUtc!==null;},'isolated history baseline');
    await setScenario(historyRoot,'largeHistory');await invoke('check_now');
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.counts.notificationHistory===240;},'240 source records stored in SQLite',45_000);
    const expected=await invoke('get_notification_history');
    assert.equal(expected.length,240);
    const pages=[],paged=[];
    let cursor=null;
    do {
      const page=await invoke('get_notification_history_page',{cursor});
      pages.push(page.items.length);paged.push(...page.items);cursor=page.nextCursor;
    } while(cursor!==null);
    assert.deepEqual(pages,[50,50,50,50,40]);
    assert.deepEqual(paged.map(item=>item.id),expected.map(item=>item.id),'native IPC pages retain canonical SQL order');
    assert.equal(new Set(paged.map(item=>item.id)).size,240);
    await browser.$(`[data-notification-id="${expected[0].id}"]`).waitForDisplayed();
    assert.equal(await browser.$('.history-toolbar > span').getText(),'Unread: 240');
    await invoke('mark_notification_read',{notificationId:expected[1].id});
    await waitUntil(async()=>(await state()).counts.unreadNotifications===239,'one read card for layout comparison');
    const originalRect=await browser.getWindowRect();
    const originalInner=await browser.execute(()=>({width:innerWidth,height:innerHeight}));
    const chromeWidth=originalRect.width-originalInner.width,chromeHeight=originalRect.height-originalInner.height;
    const measurements=[];
    for(const language of ['en','ru']) {
      await invoke('update_settings',{settings:{...(await state()).settings,languageCode:language}});
      await waitUntil(async()=>await browser.execute(()=>document.documentElement.lang)===language,'large history screenshot language '+language);
      for(const [width,height] of [[1100,740],[760,600]]) {
        await browser.setWindowRect(originalRect.x,originalRect.y,width+chromeWidth,height+chromeHeight);
        await waitUntil(async()=>await browser.execute(size=>innerWidth===size.width&&innerHeight===size.height,{width,height}),'exact large history client size '+width+'x'+height);
        await browser.$(`[data-notification-id="${expected[0].id}"]`).waitForDisplayed();
        await waitUntil(async()=>await browser.execute(id=>{const row=document.querySelector(`[data-notification-id="${id}"]`);return row&&!row.classList.contains('unread');},expected[1].id),'read/unread layout settled');
        await browser.saveScreenshot(path.join(historyReport,language+'-'+width+'x'+height+'.png'));
        measurements.push({...await historyViewport(browser),language,width,height});
      }
    }
    await invoke('update_settings',{settings:{...(await state()).settings,languageCode:'en'}});
    await waitUntil(async()=>await browser.execute(()=>document.documentElement.lang)==='en','restore English for history actions');
    await fs.writeFile(path.join(historyReport,'walk-expected.json'),JSON.stringify(expected.map((item,index)=>({id:item.id,position:index+1,title:item.title})),null,2)+'\n');
    const initialWalk=await settledHistoryViewport(browser);
    await fs.writeFile(path.join(historyReport,'walk-start.json'),JSON.stringify({expected:expected.map((item,index)=>({id:item.id,position:index+1,title:item.title})),viewport:initialWalk},null,2)+'\n');
    const walked=await walkHistory(browser,expected.at(-1).id,{wheel:false,onStep:entry=>fs.appendFile(path.join(historyReport,'walk-trace.jsonl'),JSON.stringify(entry)+'\n')});
    await fs.writeFile(path.join(historyReport,'walk-result.json'),JSON.stringify({seen:walked.seen,expected:expected.map(item=>item.id),heights:walked.heights,largestDom:walked.largestDom,last:walked.last},null,2)+'\n');
    assert.deepEqual(walked.seen,expected.map(item=>item.id),'WebView2 scrolling visits every stored campaign');
    assert.ok(walked.heights.length>=2,'production parsed long titles create different card heights');
    await browser.saveScreenshot(path.join(historyReport,'oldest-page.png'));
    const anchor=historyAnchor(walked.last);
    await setScenario(historyRoot,'largeHistoryNew');await invoke('check_now');
    await waitUntil(async()=>{const current=await state();return !current.monitor.isChecking&&current.counts.notificationHistory===241;},'one new campaign while reading an older page');
    const newFirst=(await invoke('get_notification_history'))[0].id;
    await waitUntil(async()=>await browser.execute(first=>{
      const list=document.querySelector('.history-list');
      return list?.getAttribute('aria-busy')==='false'&&list.dataset.historyFirstId===first;
    },newFirst),'new native history committed after cursor-chain refresh');
    await waitUntil(async()=>{
      const view=await historyViewport(browser),row=view.visible.find(row=>row.id===anchor.id);
      return row&&Math.abs(row.top-anchor.top)<=2;
    },'native arrival preserves the visible campaign and its offset');
    const unread=walked.last.visible.find(row=>row.top>=0&&row.bottom<=walked.last.height)??walked.last.visible[0];
    await browser.$(`[data-notification-id="${unread.id}"] .mark-read-button`).click();
    await waitUntil(async()=>(await state()).counts.unreadNotifications===239,'read one old visible campaign');
    await browser.$('.history-toolbar button').click();
    await waitUntil(async()=>(await state()).counts.unreadNotifications===0,'mark all includes all unloaded native pages');
    await scrollHistory(browser,-(await historyViewport(browser)).scrollHeight,{wheel:false});
    await waitUntil(async()=>await browser.$(`[data-notification-id="${newFirst}"] strong`).getText()==='New arrival while reading older history','return to newly arrived first campaign');
    assert.equal(await browser.$(`[data-notification-id="${newFirst}"] .unread-dot`).isExisting(),false);
    await browser.$('.header-button').click();await selectSettingsSection(browser,'application');await browser.$('button=Русский').click();
    await browser.$('.dialog-actions .primary-button').click();await browser.$('[role="dialog"]').waitForExist({reverse:true});
    await browser.setWindowRect(originalRect.x,originalRect.y,760+chromeWidth,600+chromeHeight);
    await waitUntil(async()=>await browser.execute(()=>innerWidth===760&&innerHeight===600),'exact final compact history size');
    const compact=await browser.execute(()=>({width:innerWidth,overflow:document.documentElement.scrollWidth>innerWidth,mounted:document.querySelectorAll('.history-item').length}));
    assert.equal(compact.overflow,false,JSON.stringify(compact));assert.ok(compact.mounted<=30,JSON.stringify(compact));
    await browser.saveScreenshot(path.join(historyReport,'compact-russian.png'));
    await browser.$('.header-button').click();await selectSettingsSection(browser,'updatesData');
    await browser.execute(()=>{window.confirm=()=>true;});await browser.$('.danger-button').scrollIntoView({block:'center'});await browser.$('.danger-button').click();
    await waitUntil(async()=>(await state()).counts.notificationHistory===0,'clear all paged native history');
    await browser.$('.dialog-actions .secondary-button').click();await browser.$('[role="dialog"]').waitForExist({reverse:true});
    await waitUntil(async()=>await browser.$$('.history-item').length===0,'cleared virtual rows disappear');
    assert.equal(await browser.$('.history-toolbar > span').getText(),'Непрочитанных: 0');
    await fs.writeFile(path.join(historyReport,'result.json'),JSON.stringify({pages,records:expected.map(item=>({id:item.id,title:item.title})),largestDom:walked.largestDom,heights:walked.heights,anchor,compact,measurements},null,2)+'\n');
  } catch(error) {
    if(historyApp)await historyApp.browser.saveScreenshot(path.join(historyReport,'failure.png')).catch(()=>{});
    if(error.historySettling)await fs.writeFile(path.join(historyReport,'walk-settling-failure.json'),JSON.stringify(error.historySettling,null,2)+'\n');
    throw error;
  } finally {
    try {if(historyApp)await historyApp.stop();}
    finally {await preserveProfile(historyRoot,historyReport);}
  }
},{timeout:180_000});

test('test executable refuses startup without an owned temp profile',async () => {
  const env={...process.env};delete env.GIVEAWAY_SCOUT_UI_ROOT;
  const child=spawn(await verifiedArtifact(),[],{env,stdio:['ignore','ignore','pipe'],windowsHide:true,timeout:5_000});
  let output='';child.stderr.on('data',data=>output+=data);
  const [code]=await once(child,'exit');
  assert.equal(code,2,output);
  assert.match(output,/missing GIVEAWAY_SCOUT_UI_ROOT/);
},{timeout:10_000});
