import assert from 'node:assert/strict';
import { before, beforeEach, after, afterEach, test } from 'node:test';
import { remote } from 'webdriverio';
import { startFixtureServer } from './fixture-server.mjs';

// Real WebdriverIO/Chrome, but fake Tauri IPC. NOT a native acceptance suite.
// Deliberately not named *.test.*: Vitest must not collect node:test suites.
let server;
let browser;
before(async () => {
  server = await startFixtureServer();
  browser = await remote({
    logLevel: 'error',
    connectionRetryCount: 0,
    connectionRetryTimeout: 60_000,
    waitforTimeout: 10_000,
    capabilities: {
      browserName: 'chrome',
      'goog:chromeOptions': {
        ...(process.env.WDIO_CHROME_BINARY ? {binary: process.env.WDIO_CHROME_BINARY} : {}),
        args: ['--headless=new', '--no-first-run', '--window-size=1100,900'],
      },
    },
  });
}, {timeout: 120_000});
beforeEach(async () => {
  await browser.url(server.url);
  await browser.$('.header-button').waitForEnabled();
});
afterEach(async () => {
  if (browser) assert.deepEqual(await browser.execute(() => window.__uiTest.unexpected), []);
});
after(async () => {
  try { if (browser) await browser.deleteSession(); }
  finally { if (server) await server.close(); }
});

test('compact main screen has no repeated branding or generic source descriptions',async()=>{
  assert.equal(await browser.$('main h1').isExisting(),false);
  assert.equal(await browser.$('.brand').isExisting(),false);
  assert.equal(await browser.$('.source-row:nth-child(1) div > span').isExisting(),false);
  assert.equal(await browser.$('.source-row:nth-child(2) div > span').isExisting(),false);
  assert.match(await browser.$('.source-row:nth-child(3)').getText(),/HTTP/);
  assert.equal(await browser.$('.status-panel .header-button').isExisting(),true);
});

test('existing Zeros error is retranslated after switching UI language without exposing diagnostics',async()=>{
  await browser.execute(()=>Object.assign(window.__uiTest.monitor,{
    lastError:'RAW_CONNECTION_REJECTED',lastFailureCount:1,
    sourceFailures:[{source:'zeros',message:'RAW_CONNECTION_REJECTED',reason:{code:'zerosConnectionLimit'}}],
  }));
  await browser.waitUntil(async()=>(await browser.$('.status-panel').getText()).includes('one connection per public IP'));
  await browser.$('.header-button').click();
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async()=>(await browser.$('.status-panel').getText()).includes('одно подключение на внешний IP'));
  assert.equal((await browser.$('.status-panel').getText()).includes('RAW_CONNECTION_REJECTED'),false);
  assert.equal(await browser.$('.connection-dot.error').isExisting(),true);
});

test('source checkbox actually changes checked state and mock settings', async () => {
  const source = await browser.$('.source-list > .source-row:first-child');
  assert.equal(await source.getAttribute('aria-checked'), 'true');
  await source.click();
  await browser.waitUntil(async () => await source.getAttribute('aria-checked') === 'false');
  const writes = await browser.execute(() => window.__uiTest.calls.filter(call => call.command === 'update_settings'));
  assert.equal(writes.length, 1);
  assert.equal(writes[0].args.settings.giveeClubEnabled, false);
});

test('Zeros is opt-in and its connection limits are explained in settings', async () => {
  const source=await browser.$('.source-list > .source-row:nth-child(3)');
  assert.equal(await source.getAttribute('aria-checked'),'false');
  assert.match(await source.getText(),/HTTP/);
  await browser.$('.header-button').click();
  assert.match(await browser.$('[role="dialog"]').getText(),/one connection per public IP/);
  await browser.$('[role="dialog"]').$('label*=Zeros').$('input').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async ()=>(await source.getAttribute('aria-checked'))==='true');
  const writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.equal(writes.at(-1).args.settings.zerosEnabled,true);
});

test('Escape asks before discarding changes; keep editing, save and discard all work', async () => {
  await browser.$('.header-button').click();
  await browser.$('[role="dialog"]').waitForDisplayed();
  assert.equal(await browser.$('.settings-draft-status').getText(),'No changes');
  await browser.$('#check-interval').setValue('90');
  assert.equal(await browser.$('.settings-draft-status').getText(),'You have unsaved changes');
  await browser.keys('Escape');
  await browser.$('.settings-close-prompt').waitForDisplayed();
  assert.equal(await browser.$('[role="dialog"]').isDisplayed(),true);
  await browser.$('button=Keep editing').click();
  assert.equal(await browser.$('#check-interval').getValue(),'90');
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('.settings-close-prompt').waitForDisplayed();
  await browser.keys('Escape');
  await browser.$('.settings-close-prompt').waitForExist({reverse:true});
  await browser.$('.dialog-actions .secondary-button').click();
  await browser.$('button=Discard changes').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.execute(() => window.__uiTest.calls.some(call => call.command === 'update_settings')), false);
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#check-interval').getValue(),'60');
  await browser.$('#check-interval').setValue('90');
  await browser.keys('Escape');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.execute(() => window.__uiTest.calls.filter(call => call.command === 'update_settings').at(-1).args.settings.checkIntervalMinutes),90);
});

test('invalid interval disables Save and focus can leave the input', async () => {
  await browser.$('.header-button').click();
  const input = await browser.$('#check-interval');
  await input.setValue('1');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(), false);
  await browser.keys('Tab');
  assert.equal(await browser.execute(() => document.activeElement?.id === 'check-interval'), false);
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('button=Discard changes').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('footer and header remain visible at both ends of the settings list, including a small window',async()=>{
  try {
    for (const [width,height] of [[960,720],[800,640]]) {
      await browser.setWindowSize(width,height);
      await browser.$('.header-button').click();
      const geometry=await browser.execute(()=>{
        const body=document.querySelector('.settings-content');
        const footer=document.querySelector('.settings-footer');
        const header=document.querySelector('.dialog-header');
        const before=footer.getBoundingClientRect();
        const top=header.getBoundingClientRect().top;
        const overflow=body.scrollHeight>body.clientHeight;
        body.scrollTop=body.scrollHeight;
        const after=footer.getBoundingClientRect();
        return {overflow,top,headerAfter:header.getBoundingClientRect().top,
          beforeTop:before.top,afterTop:after.top,bottom:after.bottom,viewport:innerHeight};
      });
      assert.equal(geometry.overflow,true);
      assert.ok(Math.abs(geometry.beforeTop-geometry.afterTop)<1);
      assert.equal(geometry.top,geometry.headerAfter);
      assert.ok(geometry.top>=0 && geometry.bottom<=geometry.viewport);
      assert.equal(await browser.$('.dialog-actions .primary-button').isDisplayed(),true);
      await browser.keys('Escape');
      await browser.$('[role="dialog"]').waitForExist({reverse:true});
    }
  } finally { await browser.setWindowSize(1100,900); }
});

test('save error remains in the fixed footer and the draft can be retried',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#check-interval').setValue('90');
  await browser.execute(()=>window.__uiTest.failSettingsWrites=1);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('.settings-footer .dialog-error').waitForDisplayed();
  assert.equal(await browser.$('#check-interval').getValue(),'90');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('saving locks the draft and all close routes until the write completes',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#check-interval').setValue('90');
  await browser.execute(()=>window.__uiTest.pauseSettingsWrite=true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.waitUntil(async()=>await browser.$('[role="dialog"]').getAttribute('aria-busy')==='true');
  assert.equal(await browser.$('.dialog-header .icon-button').isEnabled(),false);
  assert.equal(await browser.$('.dialog-actions .secondary-button').isEnabled(),false);
  assert.equal(await browser.execute(()=>document.querySelector('.settings-content').inert),true);
  await browser.keys('Escape');
  await browser.execute(()=>document.querySelector('.modal-backdrop').dispatchEvent(new MouseEvent('mousedown',{bubbles:true})));
  assert.equal(await browser.$('[role="dialog"]').isDisplayed(),true);
  assert.equal(await browser.$('.settings-close-prompt').isExisting(),false);
  await browser.execute(()=>window.__uiTest.finishSettingsWrite());
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.execute(()=>window.__uiTest.calls.filter(c=>c.command==='update_settings').length),1);
});

test('footer validation link focuses the invalid field without hiding the footer',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#http-retries').setValue('8');
  await browser.execute(()=>{document.querySelector('.settings-content').scrollTop=0;});
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  await browser.$('.settings-validation-link').click();
  assert.equal(await browser.execute(()=>document.activeElement?.id),'http-retries');
  await browser.$('#http-retries').setValue('2');
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('startup-in-tray and close-to-tray checkboxes save independently',async()=>{
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#start-in-tray').isSelected(),false);
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  for(const startup of [false,true]) {
    for(const close of [false,true]) {
      await browser.$('.header-button').click();
      for(const [id,value] of [['start-in-tray',startup],['keep-running-in-tray',close]]) {
        const input=await browser.$(`#${id}`);
        if(await input.isSelected()!==value) await input.click();
      }
      await browser.$('.dialog-actions .primary-button').click();
      await browser.$('[role="dialog"]').waitForExist({reverse:true});
      const saved=await browser.execute(()=>window.__uiTest.calls.filter(c=>c.command==='update_settings').at(-1).args.settings);
      assert.equal(saved.startInTray,startup);
      assert.equal(saved.keepRunningInTray,close);
      await browser.$('.header-button').click();
      assert.equal(await browser.$('#start-in-tray').isSelected(),startup);
      assert.equal(await browser.$('#keep-running-in-tray').isSelected(),close);
      await browser.keys('Escape');
      await browser.$('[role="dialog"]').waitForExist({reverse:true});
    }
  }
  await browser.$('.header-button').click();
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  assert.equal(await browser.$('#keep-running-in-tray').isEnabled(),true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),true);
  assert.equal(await browser.$('#start-in-tray').isSelected(),true);
});

test('panels fill the window, history scrolls and long checkbox labels do not shrink inputs',async()=>{
  try {
    await browser.url(server.url+'?history');
    await browser.$('.history-item:first-child').waitForDisplayed();
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const [width,height] of [[960,720],[1100,900],[960,600]]) {
      await browser.setWindowSize(width,height+chromeHeight);
      const geometry=await browser.execute(()=>{
        const card=document.querySelector('.history-card').getBoundingClientRect();
        const sources=document.querySelector('.sources-card').getBoundingClientRect();
        const list=document.querySelector('.history-list');
        return {bottom:card.bottom,sourcesBottom:sources.bottom,viewport:innerHeight,
          padding:parseFloat(getComputedStyle(document.querySelector('.app-shell')).paddingBottom),
          pageScroll:document.documentElement.scrollHeight>innerHeight,
          listScroll:list.scrollHeight>list.clientHeight};
      });
      assert.ok(Math.abs(geometry.viewport-geometry.bottom-geometry.padding)<2,JSON.stringify(geometry));
      assert.equal(geometry.bottom,geometry.sourcesBottom);
      assert.equal(geometry.pageScroll,false);
      assert.equal(geometry.listScroll,true);
    }
    await browser.$('.header-button').click();
    await browser.$('button=Русский').click();
    const widths=await browser.execute(()=>Array.from(document.querySelectorAll('.check-setting-details input')).map(input=>input.getBoundingClientRect().width));
    assert.ok(widths.length>=3);
    assert.ok(widths.every(width=>width===17),JSON.stringify(widths));
  } finally { await browser.setWindowSize(1100,900); }
});

test('valid interval saves into isolated in-memory settings', async () => {
  await browser.$('.header-button').click();
  await browser.$('#check-interval').setValue('90');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#check-interval').getValue(), '90');
});

test('action buttons have spacing, pointer cursors and visible hover feedback', async () => {
  const primary = await browser.$('.source-actions .primary-button');
  const secondary = await browser.$('.source-actions .secondary-button');
  await browser.$('.header-button').moveTo();
  const layout = await browser.execute(() => {
    const first = document.querySelector('.source-actions .primary-button');
    const second = document.querySelector('.source-actions .secondary-button');
    return {
      gap: second.getBoundingClientRect().top - first.getBoundingClientRect().bottom,
      firstCursor: getComputedStyle(first).cursor,
      secondCursor: getComputedStyle(second).cursor,
      firstColor: getComputedStyle(first).backgroundColor,
      secondColor: getComputedStyle(second).backgroundColor,
    };
  });
  assert.ok(layout.gap >= 8, `Buttons must not touch; gap was ${layout.gap}px`);
  assert.equal(layout.firstCursor, 'pointer');
  assert.equal(layout.secondCursor, 'pointer');
  await primary.moveTo();
  await browser.waitUntil(async () => await browser.execute(() => getComputedStyle(document.querySelector('.source-actions .primary-button')).backgroundColor) !== layout.firstColor);
  await secondary.moveTo();
  await browser.waitUntil(async () => await browser.execute(() => getComputedStyle(document.querySelector('.source-actions .secondary-button')).backgroundColor) !== layout.secondColor);
});
