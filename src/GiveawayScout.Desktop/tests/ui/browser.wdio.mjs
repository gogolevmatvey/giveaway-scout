import assert from 'node:assert/strict';
import { before, beforeEach, after, afterEach, test } from 'node:test';
import { remote } from 'webdriverio';
import { startFixtureServer } from './fixture-server.mjs';
import { historyViewport, scrollHistory, walkHistory, historyAnchor } from './history-ui.mjs';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// Real WebdriverIO/Chrome, but fake Tauri IPC. NOT a native acceptance suite.
// Deliberately not named *.test.*: Vitest must not collect node:test suites.
let server;
let browser;
async function selectSettingsSection(section) {
  const tab=browser.$(`[data-settings-section="${section}"]`);
  await tab.waitForEnabled();
  if(await tab.getAttribute('aria-selected')!=='true') await tab.click();
  await browser.waitUntil(async()=>await tab.getAttribute('aria-selected')==='true');
  await browser.$(`#settings-panel-${section}`).waitForDisplayed();
}
async function saveOrCloseSettings() {
  const save=browser.$('.dialog-actions .primary-button');
  if(await save.isEnabled()) await save.click();
  else {
    assert.equal(await browser.$('.settings-draft-status').getText(),'No changes');
    await browser.$('.dialog-actions .secondary-button').click();
  }
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
}
async function waitForSettingFocus(id) {
  await browser.waitUntil(async()=>await browser.execute(()=>document.activeElement?.id)===id,{timeoutMsg:'setting receives focus: '+id});
}
async function clickCardArea(selector, bottom = false) {
  const point = await browser.execute(({selector, bottom}) => {
    const rect = document.querySelector(selector).getBoundingClientRect();
    return {x: Math.round(rect.left + rect.width / 2), y: Math.round(bottom ? rect.bottom - 4 : rect.top + rect.height / 2)};
  }, {selector, bottom});
  await browser.performActions([{type:'pointer',id:'card-pointer',parameters:{pointerType:'mouse'},actions:[
    {type:'pointerMove',duration:0,origin:'viewport',...point},
    {type:'pointerDown',button:0},{type:'pointerUp',button:0},
  ]}]);
  await browser.releaseActions();
}
before(async () => {
  server = await startFixtureServer();
  browser = await remote({
    logLevel: 'error',
    connectionRetryCount: 0,
    connectionRetryTimeout: 60_000,
    waitforTimeout: 10_000,
    capabilities: {
      browserName: 'chrome',
      // These DOM checks need no BiDi websocket. Classic transport matches
      // the native suite and closes reliably after fixture navigations.
      'wdio:enforceWebDriverClassic': true,
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
  if (browser) {
    assert.deepEqual(await browser.execute(() => window.__uiTest.unexpected), []);
    assert.equal(await browser.execute(()=>window.__uiTest.calls.some(call=>call.command==='get_notification_history')),false,'the renderer never fetches the complete history');
  }
});
after(async () => {
  try { if (browser) await browser.deleteSession(); }
  finally { if (server) await server.close(); }
});

test('autostart select supports keyboard and Escape before closing settings',async()=>{
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  const target=browser.$('#autostart-target');
  await target.scrollIntoView({block:'center'});
  await target.click();
  assert.equal(await target.getAttribute('aria-expanded'),'true');
  assert.equal(await browser.$$('[role="listbox"]').length,1);
  await browser.keys('End');
  await browser.keys('Escape');
  assert.equal(await target.getAttribute('data-value'),'gui');
  await target.click();
  await browser.keys('Home');
  await browser.keys('ArrowDown');
  await browser.keys('Enter');
  assert.equal(await target.getAttribute('data-value'),'cli');
  await target.click();
  await browser.keys('d');
  await browser.keys('Enter');
  assert.equal(await target.getAttribute('data-value'),'gui');
  await target.click();
  await browser.keys('Tab');
  assert.equal(await browser.$('[role="listbox"]').isExisting(),false);
  await target.click();
  assert.equal(await browser.$('[role="listbox"]').isDisplayed(),true);
  await browser.keys('Escape');
  assert.equal(await browser.$('[role="listbox"]').isExisting(),false);
  assert.equal(await browser.$('[role="dialog"]').isExisting(),true);
  await browser.keys('Escape');
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.url(server.url+'?dev');
  await browser.$('.header-button').waitForEnabled();
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  assert.equal(await browser.$('#autostart-target').isEnabled(),false);
});

test('settings controls keep text styles while rounded checkboxes and full-width options expose every state',async()=>{
  await browser.$('.header-button').click();
  const checkbox=browser.$('#source-enabled-freeSteamKeys');
  const checkboxStyle=()=>browser.execute(()=>{
    const input=document.querySelector('#source-enabled-freeSteamKeys'),style=getComputedStyle(input);
    return {appearance:style.appearance,radius:parseFloat(style.borderRadius),width:input.getBoundingClientRect().width,
      background:style.backgroundColor,outline:parseFloat(style.outlineWidth),focus:input.matches(':focus-visible')};
  });
  const checked=await checkboxStyle();
  assert.equal(checked.appearance,'none');assert.ok(checked.radius>=3&&checked.radius<=checked.width/2);
  await browser.$('label[for="source-enabled-freeSteamKeys"] > span').click();
  assert.equal(await checkbox.isSelected(),false,'clicking the label toggles the native input');
  assert.notEqual((await checkboxStyle()).background,checked.background);
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),true);
  assert.equal(await browser.execute(()=>getComputedStyle(document.querySelector('.dialog-actions .primary-button')).color),'rgb(255, 255, 255)');
  await browser.keys('Tab');await browser.keys(['Shift','Tab']);
  assert.equal(await checkbox.isFocused(),true);
  const focused=await checkboxStyle();assert.equal(focused.focus,true);assert.ok(focused.outline>=2);
  await browser.keys(' ');
  assert.equal(await checkbox.isSelected(),true,'Space changes the focused checkbox');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false,'restoring the checkbox restores a clean draft');
  const text=await browser.execute(()=>{
    const read=selector=>{const style=getComputedStyle(document.querySelector(selector));return {size:style.fontSize,color:style.color};};
    return {label:read('label[for="source-enabled-freeSteamKeys"] > span'),hint:read('#source-interval-range'),
      active:read('#settings-tab-sources'),inactive:read('#settings-tab-notifications'),status:read('.settings-draft-status'),
      save:read('.dialog-actions .primary-button'),cancel:read('.dialog-actions .secondary-button')};
  });
  assert.deepEqual(text,{
    label:{size:'12px',color:'rgb(216, 222, 235)'},hint:{size:'12px',color:'rgb(163, 172, 190)'},
    active:{size:'12px',color:'rgb(237, 241, 250)'},inactive:{size:'12px',color:'rgb(178, 187, 205)'},
    status:{size:'12px',color:'rgb(232, 236, 246)'},save:{size:'12px',color:'rgb(150, 162, 186)'},cancel:{size:'12px',color:'rgb(199, 208, 255)'},
  },'the typography proposal was cancelled; labels, help, navigation and button text stay unchanged');
  const debuggerAddress=browser.capabilities['goog:chromeOptions'].debuggerAddress;
  const targets=await(await fetch('http://'+debuggerAddress+'/json/list')).json();
  const currentUrl=await browser.getUrl(),target=targets.find(target=>target.type==='page'&&target.url===currentUrl);
  assert.ok(target,'the test owns the Chrome page used for forced-colors emulation');
  const devtools=new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve,reject)=>{devtools.addEventListener('open',resolve,{once:true});devtools.addEventListener('error',reject,{once:true});});
  let commandId=0;
  const emulate=features=>new Promise((resolve,reject)=>{
    const id=++commandId,receive=event=>{const message=JSON.parse(event.data);if(message.id!==id)return;devtools.removeEventListener('message',receive);if(message.error)reject(new Error(JSON.stringify(message.error)));else resolve();};
    devtools.addEventListener('message',receive);devtools.send(JSON.stringify({id,method:'Emulation.setEmulatedMedia',params:{features}}));
  });
  try {
    await emulate([{name:'forced-colors',value:'active'}]);
    await browser.waitUntil(async()=>await browser.execute(()=>matchMedia('(forced-colors: active)').matches));
    assert.equal((await checkboxStyle()).appearance,'auto','forced colors use the native accessible checkbox');
    await browser.keys(' ');assert.equal(await checkbox.isSelected(),false);
    await browser.keys(' ');assert.equal(await checkbox.isSelected(),true,'native forced-colors fallback still supports Space');
  } finally {
    await emulate([]);devtools.close();
    await browser.waitUntil(async()=>await browser.execute(()=>!matchMedia('(forced-colors: active)').matches));
  }
  await selectSettingsSection('application');
  const disabled=browser.$('#start-in-tray'),before=await disabled.isSelected();
  assert.equal(await disabled.isEnabled(),false);
  await browser.$('label:has(#start-in-tray) strong').click();
  assert.equal(await disabled.isSelected(),before,'a disabled rounded checkbox ignores label activation');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  const trigger=browser.$('#autostart-target');await trigger.scrollIntoView({block:'center'});await trigger.click();
  const other=browser.$('[role="option"][data-value="cli"]');
  const popupStyle=()=>browser.execute(()=>{
    const popup=document.querySelector('[role="listbox"]'),panel=popup.getBoundingClientRect(),style=getComputedStyle(popup);
    const options=[...popup.querySelectorAll('[role="option"]')].map(option=>{
      const rect=option.getBoundingClientRect(),css=getComputedStyle(option);
      return {id:option.id,left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,selected:option.getAttribute('aria-selected')==='true',
        active:option.dataset.active,hovered:option.dataset.hovered,background:css.backgroundColor,shadow:css.boxShadow,color:css.color,size:css.fontSize};
    });
    return {innerLeft:panel.left+parseFloat(style.borderLeftWidth)+parseFloat(style.paddingLeft),
      innerRight:panel.right-parseFloat(style.borderRightWidth)-parseFloat(style.paddingRight),scrolls:popup.scrollHeight>popup.clientHeight,
      font:style.fontSize,color:style.color,options};
  });
  const initial=await popupStyle();assert.equal(initial.scrolls,false);assert.equal(initial.font,'12px');assert.equal(initial.color,'rgb(237, 241, 248)');
  for(const option of initial.options) {
    assert.ok(Math.abs(option.left-initial.innerLeft)<=1&&Math.abs(option.right-initial.innerRight)<=1,JSON.stringify(initial));
    assert.equal(option.size,'12px');
  }
  assert.ok(initial.options[1].top-initial.options[0].bottom>=2,'option highlights have a visible gap');
  const normal=initial.options.find(option=>!option.selected);assert.equal(normal.color,'rgb(237, 241, 248)');
  await other.moveTo();
  await browser.waitUntil(async()=>await other.getAttribute('data-hovered')==='true');
  const hovered=await popupStyle(),selected=hovered.options.find(option=>option.selected),active=hovered.options.find(option=>!option.selected);
  assert.equal(selected.background,'rgb(40, 52, 85)');assert.equal(selected.color,'rgb(214, 222, 255)');
  assert.notEqual(active.background,selected.background);assert.notEqual(active.background,normal.background);assert.notEqual(active.shadow,'none');
  assert.equal(active.color,'rgb(243, 245, 255)');
  await browser.$('.dialog-header h2').moveTo();
  await browser.waitUntil(async()=>await other.getAttribute('data-hovered')==='false');
  const left=(await popupStyle()).options.find(option=>!option.selected);
  assert.equal(left.background,normal.background,'leaving the popup removes pointer highlighting');assert.equal(left.color,normal.color);
  await browser.keys('Home');await browser.keys('ArrowDown');
  assert.equal(await trigger.getAttribute('aria-activedescendant'),await other.getAttribute('id'));
  assert.equal(await other.getAttribute('data-active'),'true');
  await browser.keys('Escape');assert.equal(await browser.$('[role="listbox"]').isExisting(),false);
  assert.equal(await browser.$('[role="dialog"]').isDisplayed(),true);
  if(process.env.GIVEAWAY_SCOUT_BROWSER_REPORT) {
    const directory=process.env.GIVEAWAY_SCOUT_BROWSER_REPORT;await mkdir(directory,{recursive:true});
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight),measurements=[];
    try {
      for(const language of ['en','ru']) {
        if(language==='ru') {
          await browser.$('button=Русский').click();await browser.$('.dialog-actions .primary-button').click();
          await browser.$('[role="dialog"]').waitForExist({reverse:true});await browser.$('.header-button').click();await selectSettingsSection('application');
        }
        for(const [width,height] of [[1100,740],[760,600]]) {
          await browser.setWindowSize(width,height+chromeHeight);
          // Chrome can change its outer/client decoration offset on the first
          // resize. Correct against the actual client bounds before exporting.
          for(let attempt=0;attempt<3;attempt++) {
            const client=await browser.execute(()=>({width:innerWidth,height:innerHeight}));
            if(client.width===width&&client.height===height)break;
            const outer=await browser.getWindowSize();
            await browser.setWindowSize(outer.width+width-client.width,outer.height+height-client.height);
          }
          const client=await browser.execute(()=>({width:innerWidth,height:innerHeight}));
          assert.deepEqual(client,{width,height},'exported popup uses the exact requested client size');
          const popupTrigger=browser.$('#autostart-target'),popupOther=browser.$('[role="option"][data-value="cli"]');
          await popupTrigger.scrollIntoView({block:'center'});await popupTrigger.click();await popupOther.moveTo();
          await browser.waitUntil(async()=>await popupOther.getAttribute('data-hovered')==='true');
          measurements.push({language,width,height,popup:await popupStyle()});
          await browser.saveScreenshot(path.join(directory,language+'-'+width+'x'+height+'-application-popup.png'));
          await browser.keys('Escape');await browser.$('[role="listbox"]').waitForExist({reverse:true});
        }
      }
      await writeFile(path.join(directory,'geometry.json'),JSON.stringify({kind:'real Chrome renderer; fake Tauri release-mode IPC, isDevelopment:false; native e2e build separately verifies its disabled autostart controls',measurements},null,2)+'\n');
    } finally {await browser.setWindowSize(1100,900);}
  }
  await browser.keys('Escape');
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('application select opens above a low anchor, repositions on resize and supports Russian prefix search',async()=>{
  try {
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    await browser.setWindowSize(760,600+chromeHeight);
    await browser.$('.header-button').click();await selectSettingsSection('application');await browser.$('button=Русский').click();
    await browser.$('.dialog-actions .primary-button').click();
    await browser.$('[role="dialog"]').waitForExist({reverse:true});
    await browser.$('.header-button').click();
    await selectSettingsSection('application');
    const field=browser.$('#autostart-target');
    // The remaining select has just two choices. Taller options give the real
    // portal insufficient room below its genuine anchor in the settings pane.
    await browser.execute(()=>{
      const style=document.createElement('style');
      style.textContent='.app-select-popup .app-select-option { min-height: 128px; }';
      document.head.append(style);
      // Extra preceding content models longer labels and makes the real
      // scroll pane place the trigger near its lower edge. No DOM reparenting.
      document.querySelector('#autostart-target').closest('.input-setting').style.marginTop='180px';
    });
    await field.scrollIntoView({block:'end'});
    await field.click();
    await browser.waitUntil(async()=>await browser.$('[role="listbox"]').getAttribute('data-placement')==='above');
    await browser.setWindowSize(820,620+chromeHeight);
    await field.scrollIntoView({block:'end'});
    if(await field.getAttribute('aria-expanded')!=='true') await field.click();
    await browser.waitUntil(async()=>await browser.execute(()=>{
      const panel=document.querySelector('[role="listbox"]')?.getBoundingClientRect();
      return panel&&panel.top>=0&&panel.bottom<=innerHeight&&panel.right<=innerWidth;
    }));
    const geometry=await browser.execute(()=>{
      const listbox=document.querySelector('[role="listbox"]'),panel=listbox.getBoundingClientRect(),anchor=document.querySelector('#autostart-target').getBoundingClientRect();
      return {top:panel.top,bottom:panel.bottom,anchorTop:anchor.top,anchorBottom:anchor.bottom,right:panel.right,width:innerWidth,placement:listbox.dataset.placement};
    });
    assert.ok(geometry.top>=0&&geometry.right<=geometry.width,JSON.stringify(geometry));
    if(geometry.placement==='above') assert.ok(geometry.bottom<=geometry.anchorTop,JSON.stringify(geometry));
    else {assert.equal(geometry.placement,'below');assert.ok(geometry.top>=geometry.anchorBottom,JSON.stringify(geometry));}
    await browser.keys('м');await browser.keys('о');await browser.keys('н');await browser.keys('Enter');
    assert.equal(await field.getAttribute('data-value'),'cli');
    await field.click();
    await selectSettingsSection('sources');
    await browser.$('[role="listbox"]').waitForExist({reverse:true});
  } finally {await browser.setWindowSize(1100,900);}
});

test('source details show one localized error without raw diagnostics and clear after recovery',async()=>{
  const diagnostic='https://freeanywhere.net/: connection reset by peer (os error 10054)\n<script>window.__diagnosticExecuted=true</script>';
  const fail=reason=>browser.execute(({message,reason})=>{
      const failure={source:'gamesForFarm',message,reason};
      const state=window.__uiTest.monitor.sourceStates.find(state=>state.source==='gamesForFarm');
      Object.assign(state,{phase:'error',lastError:failure});
      window.__uiTest.emit('monitor-changed');
    },{message:diagnostic,reason});
  await fail({code:'connection'});
  const details=browser.$('[data-source="gamesForFarm"] .source-details');
  await details.waitForExist();
  await details.$('summary').click();
  const cases=[
    {reason:{code:'connection'},en:'Could not connect to the source.',ru:'Не удалось подключиться к источнику.'},
    {reason:{code:'invalidFeed'},en:'The giveaway list could not be recognized.',ru:'Не удалось распознать список раздач.'},
    {reason:{code:'http',status:503},en:'The website returned HTTP 503.',ru:'Сайт вернул ошибку HTTP 503.'},
    {reason:{code:'futureError'},en:'The source could not be checked.',ru:'Не удалось проверить источник.'},
    {reason:{code:'http',status:700},en:'The source could not be checked.',ru:'Не удалось проверить источник.'},
  ];
  for(const language of ['en','ru']) {
    if(language==='ru') {
      await browser.$('.header-button').click();await selectSettingsSection('application');
      await browser.$('button=Русский').click();await browser.$('.dialog-actions .primary-button').click();
      await browser.$('[role="dialog"]').waitForExist({reverse:true});
    }
    for(const example of cases) {
      await fail(example.reason);
      await browser.waitUntil(async()=>await details.$('.setting-error').getText()===example[language]);
      assert.equal(await details.$$('p').length,1,'one user-facing error paragraph');
      assert.equal((await details.getText()).includes('Games for Farm:'),false,'source name is already in the row heading');
      assert.equal(await details.$('.source-diagnostic').isExisting(),false);
      assert.equal(await browser.execute(()=>document.body.textContent.includes('connection reset by peer')),false);
      assert.equal(await details.$('script').isExisting(),false);
      assert.equal(await browser.execute(()=>window.__diagnosticExecuted===true),false);
      assert.equal(await browser.execute(()=>window.__uiTest.monitor.sourceStates.find(state=>state.source==='gamesForFarm').lastError.message),diagnostic,'raw diagnostic remains unchanged in the API model');
    }
  }
  await browser.execute(()=>{
    Object.assign(window.__uiTest.monitor.sourceStates.find(state=>state.source==='gamesForFarm'),{phase:'ok',lastError:null});
    window.__uiTest.emit('monitor-changed');
  });
  await details.waitForExist({reverse:true});
});

test('polished layout uses matching scrollbars, separates source actions and balances history cards',async()=>{
  try {
    await browser.url(server.url+'?history&merged&update');
    await browser.$('.history-item:first-child').waitForDisplayed();
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const [width,height] of [[1100,740],[760,600]]) {
      await browser.setWindowSize(width,height+chromeHeight);
      // Resize can place the previous pointer over a thumb. Compare scrollbar
      // colors with both scrollbars at rest before testing the row hover below.
      await browser.$('.header-button').moveTo();
      const layout=await browser.execute(()=>{
        const list=document.querySelector('.source-list'), history=document.querySelector('.history-list');
        const actions=document.querySelector('.source-actions');
        const row=document.querySelector('.history-item-layout'),main=row.querySelector('.history-open'),side=row.querySelector('.history-item-actions');
        const rect=element=>{const r=element.getBoundingClientRect();return {top:r.top,bottom:r.bottom,left:r.left,right:r.right};};
        const scroll=element=>({thumb:getComputedStyle(element,'::-webkit-scrollbar-thumb').backgroundColor,width:getComputedStyle(element,'::-webkit-scrollbar').width,arrows:getComputedStyle(element,'::-webkit-scrollbar-button').display});
        const font=selector=>parseFloat(getComputedStyle(document.querySelector(selector)).fontSize);
        const sourceWidth=document.querySelector('.sources-card').getBoundingClientRect().width,historyWidth=document.querySelector('.history-card').getBoundingClientRect().width;
        return {width:innerWidth,height:innerHeight,gap:rect(actions).top-rect(list).bottom,sourceScroll:scroll(list),historyScroll:scroll(history),main:rect(main),side:rect(side),row:rect(row),sourceShare:sourceWidth/(sourceWidth+historyWidth),
          sourceListBorders:['Top','Right','Bottom','Left'].map(side=>parseFloat(getComputedStyle(list)['border'+side+'Width'])),
          fonts:{sourceTitle:font('.source-row strong'),sourceMetadata:font('.source-row div > span'),sourceStatus:font('.source-phase'),historyTitle:font('.history-item strong'),historyMetadata:font('.history-item small'),historyDate:font('.history-item time')},
          mainTestButton:document.querySelector('.sources-card .test-notification-button')!==null,overflow:document.documentElement.scrollWidth>innerWidth};
      });
      assert.ok(layout.gap>=12,JSON.stringify(layout));
      assert.deepEqual(layout.sourceScroll,layout.historyScroll);
      assert.equal(layout.sourceScroll.arrows,'none');
      assert.equal(layout.overflow,false);
      assert.equal(layout.mainTestButton,false);
      assert.ok(layout.sourceListBorders.every(width=>width===0),JSON.stringify(layout));
      for(const [name,font] of Object.entries(layout.fonts)) assert.ok(font>=(name.endsWith('Title')?14:12),`${name}: ${font}px`);
      assert.ok(layout.side.right<=layout.row.right+1,JSON.stringify(layout));
      if(width===1100) {
        assert.ok(layout.sourceShare>=0.28&&layout.sourceShare<=0.35,JSON.stringify(layout));
        assert.ok(layout.side.left>=layout.main.right-1,JSON.stringify(layout));
      }
      const source=browser.$('[data-source="giveeClub"] .source-row');
      await browser.$('.header-button').moveTo();
      const resting=await source.getCSSProperty('background-color');
      await source.moveTo();
      await browser.waitUntil(async()=>(await source.getCSSProperty('background-color')).value!==resting.value);
      const sourceEdge=await browser.execute(()=>{
        const list=document.querySelector('.source-list'),row=document.querySelector('[data-source="giveeClub"] .source-row');
        const bounds=list.getBoundingClientRect(),entry=row.getBoundingClientRect();
        return {scrolling:list.scrollHeight>list.clientHeight,rightGap:bounds.right-parseFloat(getComputedStyle(list).borderRightWidth)-entry.right,
          rightPixelIsRow:row.contains(document.elementFromPoint(entry.right-2,entry.top+entry.height/2))};
      });
      if(!sourceEdge.scrolling) assert.ok(Math.abs(sourceEdge.rightGap)<=1,JSON.stringify(sourceEdge));
      assert.equal(sourceEdge.rightPixelIsRow,true,JSON.stringify(sourceEdge));
    }
  } finally {await browser.setWindowSize(1100,900);}
});

test('compact main screen shows sources in priority order and preserves supported stores',async()=>{
  assert.equal(await browser.$('main h1').isExisting(),false);
  assert.deepEqual(await browser.$$('.source-row strong').map(row=>row.getText()),['FreeSteamKeys','Givee Club','Games for Farm','Zeros']);
  assert.equal(await browser.$('[data-source="freeSteamKeys"] .source-row div > span').getText(),'Steam');
  assert.equal(await browser.$('[data-source="giveeClub"] .source-row div > span').getText(),'Steam · Epic Games · GOG');
  assert.match(await browser.$('[data-source="zeros"]').getText(),/HTTP/);
  assert.equal(await browser.$('.status-panel .header-button').isExisting(),true);
});

test('existing Zeros error is retranslated after switching UI language without exposing diagnostics',async()=>{
  await browser.execute(()=>{window.__uiTest.settings.zerosEnabled=true;Object.assign(window.__uiTest.monitor,{
    lastError:'RAW_CONNECTION_REJECTED',lastFailureCount:1,
    sourceFailures:[{source:'zeros',message:'RAW_CONNECTION_REJECTED',reason:{code:'zerosConnectionLimit'}}],
    sourceStates:window.__uiTest.monitor.sourceStates.map(state=>state.source==='zeros'?{...state,phase:'error',lastError:{source:'zeros',message:'RAW_CONNECTION_REJECTED',reason:{code:'zerosConnectionLimit'}}}:state),
  });window.__uiTest.emit('monitor-changed');});
  await browser.waitUntil(async()=>await browser.$('[data-source="zeros"] .source-details').isExisting());
  await browser.$('[data-source="zeros"] summary').click();
  assert.match(await browser.$('[data-source="zeros"]').getText(),/one connection per public IP/);
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async()=>(await browser.$('[data-source="zeros"]').getText()).includes('одно подключение на внешний IP'));
  assert.equal((await browser.$('.status-panel').getText()).includes('RAW_CONNECTION_REJECTED'),false);
  assert.equal(await browser.$('.connection-dot.error').isExisting(),true);
});

test('source checkbox actually changes checked state and mock settings', async () => {
  const source = await browser.$('[data-source="giveeClub"] .source-row');
  assert.equal(await source.getAttribute('aria-checked'), 'true');
  await source.click();
  await browser.waitUntil(async () => await source.getAttribute('aria-checked') === 'false');
  const writes = await browser.execute(() => window.__uiTest.calls.filter(call => call.command === 'update_settings'));
  assert.equal(writes.length, 1);
  assert.equal(writes[0].args.settings.giveeClubEnabled, false);
});

test('Zeros is opt-in and its connection limits are explained in settings', async () => {
  const source=await browser.$('[data-source="zeros"] .source-row');
  assert.equal(await source.getAttribute('aria-checked'),'false');
  assert.match(await source.getText(),/HTTP/);
  await browser.$('.header-button').click();
  assert.match(await browser.$('[role="dialog"]').getText(),/one connection per public IP/i);
  await browser.$('[role="dialog"]').$('#source-enabled-zeros').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async ()=>(await source.getAttribute('aria-checked'))==='true');
  const writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.equal(writes.at(-1).args.settings.zerosEnabled,true);
});

test('FreeSteamKeys is enabled by default and its main/settings toggles stay in sync after saving',async()=>{
  const source=await browser.$('[data-source="freeSteamKeys"] .source-row');
  assert.equal(await source.getAttribute('aria-checked'),'true');
  await source.click();
  await browser.waitUntil(async()=>await source.getAttribute('aria-checked')==='false');
  await browser.$('.header-button').click();
  let checkbox=await browser.$('[role="dialog"]').$('#source-enabled-freeSteamKeys');
  assert.equal(await checkbox.isSelected(),false);
  await checkbox.click();
  assert.equal(await browser.$('.settings-draft-status').getText(),'You have unsaved changes');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async()=>await source.getAttribute('aria-checked')==='true');
  const writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.deepEqual(writes.map(call=>call.args.settings.freeSteamKeysEnabled),[false,true]);
  assert.equal(writes.at(-1).args.settings.zerosEnabled,false);
  assert.equal(writes.at(-1).args.settings.steamNotificationsEnabled,true);
  await browser.$('.header-button').click();
  checkbox=await browser.$('[role="dialog"]').$('#source-enabled-freeSteamKeys');
  assert.equal(await checkbox.isSelected(),true);
});

test('FreeSteamKeys error translates without repeating its heading or exposing diagnostics',async()=>{
  await browser.execute(()=>Object.assign(window.__uiTest.monitor,{
    lastError:'RAW_FSK_DIAGNOSTIC',lastFailureCount:1,
    sourceFailures:[{source:'freeSteamKeys',message:'RAW_FSK_DIAGNOSTIC',reason:{code:'http',status:429}}],
    sourceStates:window.__uiTest.monitor.sourceStates.map(state=>state.source==='freeSteamKeys'?{...state,phase:'error',lastError:{source:'freeSteamKeys',message:'RAW_FSK_DIAGNOSTIC',reason:{code:'http',status:429}}}:state),
  }));
  await browser.waitUntil(async()=>await browser.$('[data-source="freeSteamKeys"] .source-details').isExisting());
  await browser.$('[data-source="freeSteamKeys"] summary').click();
  assert.equal(await browser.$('[data-source="freeSteamKeys"] .source-details .setting-error').getText(),'The website limited the number of requests (HTTP 429).');
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  const message=await browser.$('[data-source="freeSteamKeys"]').getText();
  assert.equal(await browser.$('[data-source="freeSteamKeys"] .source-details .setting-error').getText(),'Сайт ограничил число запросов (HTTP 429).');
  assert.equal(message.includes('FreeSteamKeys:'),false);
  assert.equal(message.includes('RAW_FSK_DIAGNOSTIC'),false);
  assert.equal(await browser.$$('[data-source="freeSteamKeys"] .source-details p').length,1);
  assert.equal((await browser.$('.status-panel').getText()).includes('RAW_FSK_DIAGNOSTIC'),false);
});

test('Escape asks before discarding changes; keep editing, save and discard all work', async () => {
  await browser.$('.header-button').click();
  await browser.$('[role="dialog"]').waitForDisplayed();
  assert.equal(await browser.$('.settings-draft-status').getText(),'No changes');
  await browser.$('#check-interval-freeSteamKeys').setValue('90');
  assert.equal(await browser.$('.settings-draft-status').getText(),'You have unsaved changes');
  await browser.keys('Escape');
  await browser.$('.settings-close-prompt').waitForDisplayed();
  assert.equal(await browser.$('[role="dialog"]').isDisplayed(),true);
  await browser.$('button=Keep editing').click();
  assert.equal(await browser.$('#check-interval-freeSteamKeys').getValue(),'90');
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('.settings-close-prompt').waitForDisplayed();
  await browser.keys('Escape');
  await browser.$('.settings-close-prompt').waitForExist({reverse:true});
  await browser.$('.dialog-actions .secondary-button').click();
  await browser.$('button=Discard changes').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.execute(() => window.__uiTest.calls.some(call => call.command === 'update_settings')), false);
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#check-interval-freeSteamKeys').getValue(),'60');
  await browser.$('#check-interval-freeSteamKeys').setValue('90');
  await browser.keys('Escape');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.execute(() => window.__uiTest.calls.filter(call => call.command === 'update_settings').at(-1).args.settings.sourceCheckIntervals.freeSteamKeys),90);
});

test('four source intervals default to one hour and persist independently in settings', async () => {
  const intervals = {freeSteamKeys:15,giveeClub:120,gamesForFarm:45,zeros:90};
  await browser.$('.header-button').click();
  assert.deepEqual(await browser.$$('.source-interval-settings input[type="checkbox"]').map(input=>input.getAttribute('id')), ['source-enabled-freeSteamKeys','source-enabled-giveeClub','source-enabled-gamesForFarm','source-enabled-zeros']);
  for (const [source, minutes] of Object.entries(intervals)) {
    const input = await browser.$('#check-interval-'+source);
    assert.equal(await input.getValue(),'60');
    assert.equal(await input.isEnabled(),true);
    await input.setValue(String(minutes));
  }
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  const written = await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings').at(-1).args.settings);
  assert.deepEqual(written.sourceCheckIntervals,intervals);
  assert.equal(written.checkIntervalMinutes,60);
  assert.equal(written.zerosEnabled,false);
  assert.equal(await browser.$('.source-list .source-interval').isExisting(),false);
  await browser.$('.header-button').click();
  assert.equal(await browser.$('.settings-draft-status').getText(),'No changes');
  for (const [source, minutes] of Object.entries(intervals)) assert.equal(await browser.$('#check-interval-'+source).getValue(),String(minutes));
});

test('every source interval enforces whole minutes from 5 to 1440 and validation focuses the affected field', async () => {
  await browser.$('.header-button').click();
  for (const source of ['freeSteamKeys','giveeClub','gamesForFarm','zeros']) {
    const input=await browser.$('#check-interval-'+source);
    for (const value of ['4','1441','5.5','']) {
      await input.setValue(value);
      assert.equal(await input.getAttribute('aria-invalid'),'true');
      assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
    }
    await browser.$('.settings-validation-link').click();
    await waitForSettingFocus('check-interval-'+source);
    assert.equal(await browser.execute(()=>document.activeElement?.id),'check-interval-'+source);
    for (const value of ['5','1440']) {
      await input.setValue(value);
      assert.equal(await input.getAttribute('aria-invalid'),'false');
      assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),true);
    }
    await input.setValue('60');
  }
  assert.equal(await browser.$('.settings-draft-status').getText(),'No changes');
});

test('invalid interval disables Save and focus can leave the input', async () => {
  await browser.$('.header-button').click();
  const input = await browser.$('#check-interval-freeSteamKeys');
  await input.setValue('1');
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(), false);
  await browser.keys('Tab');
  assert.equal(await browser.execute(() => document.activeElement?.id === 'check-interval-freeSteamKeys'), false);
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('button=Discard changes').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('settings navigation and fixed actions stay reachable in both languages and window sizes',async()=>{
  try {
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const language of ['en','ru']) {
      if(language==='ru') {
        await browser.$('.header-button').click();await selectSettingsSection('application');
        await browser.$('button=Русский').click();await saveOrCloseSettings();
      }
      for(const [width,height] of [[1100,740],[760,600]]) {
        await browser.setWindowSize(width,height+chromeHeight);
        await browser.$('.header-button').click();
        assert.equal(await browser.$$('[role="tab"][data-settings-section]').length,6);
        assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
        let scrollingPanels=0;
        for(const section of ['sources','notifications','application','network','diagnostics','updatesData']) {
          await selectSettingsSection(section);
          if(section==='diagnostics') {
            const details=browser.$('.settings-technical-details');
            if(await details.getAttribute('open')===null) await details.$('summary').click();
          }
          const geometry=await browser.execute(()=>{
            const body=document.querySelector('.settings-content'),footer=document.querySelector('.settings-footer'),header=document.querySelector('.dialog-header');
            const nav=document.querySelector('[role="tablist"]'),modal=document.querySelector('.settings-dialog');
            const before={footer:footer.getBoundingClientRect().top,header:header.getBoundingClientRect().top,nav:nav.getBoundingClientRect().top};
            body.scrollTop=body.scrollHeight;
            const bounds=element=>{const r=element.getBoundingClientRect();return {top:r.top,bottom:r.bottom,left:r.left,right:r.right,width:r.width,centerY:r.top+r.height/2};};
            return {before,after:{footer:footer.getBoundingClientRect().top,header:header.getBoundingClientRect().top,nav:nav.getBoundingClientRect().top},body:bounds(body),footer:bounds(footer),header:bounds(header),nav:bounds(nav),modal:bounds(modal),
              footerStatus:bounds(document.querySelector('.settings-footer-status')),footerActions:bounds(document.querySelector('.dialog-actions')),
              overflow:body.scrollHeight>body.clientHeight,visiblePanels:[...document.querySelectorAll('[role="tabpanel"]')].filter(panel=>panel.getClientRects().length>0).length,viewport:{width:innerWidth,height:innerHeight},pageOverflow:document.documentElement.scrollWidth>innerWidth||document.documentElement.scrollHeight>innerHeight};
          });
          if(geometry.overflow) scrollingPanels++;
          assert.equal(geometry.visiblePanels,1,JSON.stringify(geometry));
          assert.equal(geometry.pageOverflow,false,JSON.stringify(geometry));
          for(const key of ['footer','header','nav']) assert.ok(Math.abs(geometry.before[key]-geometry.after[key])<1,JSON.stringify(geometry));
          assert.ok(geometry.header.top>=0&&geometry.footer.bottom<=geometry.viewport.height,JSON.stringify(geometry));
          assert.ok(geometry.nav.right<=geometry.body.left+1&&geometry.nav.bottom<=geometry.footer.top+1,JSON.stringify(geometry));
          assert.ok(Math.abs(geometry.footerStatus.centerY-geometry.footerActions.centerY)<=1,JSON.stringify(geometry));
          assert.ok(geometry.footerStatus.right<=geometry.footerActions.left-8,JSON.stringify(geometry));
          assert.equal(Math.round(geometry.nav.width),width>820?164:148,'the existing sidebar width is preserved');
          assert.ok(geometry.modal.width<=geometry.viewport.width-16,JSON.stringify(geometry));
        }
        if(height<=600)assert.ok(scrollingPanels>=1,'long settings remain independently scrollable in the smaller window');
        await selectSettingsSection('sources');
        for(const source of ['freeSteamKeys','giveeClub','gamesForFarm','zeros']) {
          const input=browser.$('#check-interval-'+source);await input.scrollIntoView({block:'center'});
          assert.equal(await input.isDisplayed(),true);
        }
        await browser.keys('Escape');await browser.$('[role="dialog"]').waitForExist({reverse:true});
      }
    }
  } finally {await browser.setWindowSize(1100,900);}
});

test('save error remains in the fixed footer and the draft can be retried',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#check-interval-freeSteamKeys').setValue('90');
  await browser.execute(()=>window.__uiTest.failSettingsWrites=1);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('.settings-footer .dialog-error').waitForDisplayed();
  assert.equal(await browser.$('#check-interval-freeSteamKeys').getValue(),'90');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('saving locks the draft and all close routes until the write completes',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#check-interval-freeSteamKeys').setValue('90');
  await browser.execute(()=>window.__uiTest.pauseSettingsWrite=true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.waitUntil(async()=>await browser.$('[role="dialog"]').getAttribute('aria-busy')==='true');
  assert.equal(await browser.$('.dialog-header .icon-button').isEnabled(),false);
  assert.equal(await browser.$('.dialog-actions .secondary-button').isEnabled(),false);
  assert.equal(await browser.execute(()=>document.querySelector('.settings-content').inert),true);
  for(const tab of await browser.$$('[role="tab"][data-settings-section]')) assert.equal(await tab.isEnabled(),false);
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
  await selectSettingsSection('network');
  await browser.$('#http-retries').setValue('8');
  await selectSettingsSection('sources');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  await browser.$('.settings-validation-link').click();
  await waitForSettingFocus('http-retries');
  assert.equal(await browser.$('[data-settings-section="network"]').getAttribute('aria-selected'),'true');
  assert.equal(await browser.execute(()=>document.activeElement?.id),'http-retries');
  await browser.$('#http-retries').setValue('2');
  await browser.$('.dialog-header .icon-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
});

test('section keyboard navigation preserves one unsaved draft and commits all sections together',async()=>{
  await browser.$('.header-button').click();
  const sourceTab=browser.$('[data-settings-section="sources"]');
  await sourceTab.click();
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  await browser.keys('End');
  assert.equal(await browser.$('[data-settings-section="updatesData"]').getAttribute('aria-selected'),'true');
  await browser.keys('Home');
  assert.equal(await sourceTab.getAttribute('aria-selected'),'true');
  await browser.keys('ArrowDown');
  assert.equal(await browser.$('[data-settings-section="notifications"]').getAttribute('aria-selected'),'true');
  await browser.keys('ArrowUp');
  assert.equal(await sourceTab.getAttribute('aria-selected'),'true');
  assert.equal(await browser.$$('[role="tab"][tabindex="0"]').length,1);
  await browser.$('#check-interval-freeSteamKeys').setValue('17');
  await selectSettingsSection('notifications');
  await browser.$('#settings-panel-notifications').$('label*=GOG').$('input').click();
  await selectSettingsSection('application');
  await browser.$('#keep-running-in-tray').click();
  await selectSettingsSection('network');
  await browser.$('#http-timeout').setValue('55');
  await selectSettingsSection('diagnostics');
  await browser.$('#log-file-size').setValue('3');
  await selectSettingsSection('updatesData');
  assert.equal(await browser.execute(()=>window.__uiTest.calls.some(call=>call.command==='update_settings')),false);
  await selectSettingsSection('sources');
  assert.equal(await browser.$('#check-interval-freeSteamKeys').getValue(),'17');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  const writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.equal(writes.length,1);
  const saved=writes[0].args.settings;
  assert.equal(saved.sourceCheckIntervals.freeSteamKeys,17);
  assert.equal(saved.gogNotificationsEnabled,false);
  assert.equal(saved.keepRunningInTray,true);
  assert.equal(saved.httpTimeoutSeconds,55);
  assert.equal(saved.logMaxFileSizeMb,3);
  await browser.$('.header-button').click();
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  await browser.$('#check-interval-freeSteamKeys').setValue('18');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),true);
  await browser.$('#check-interval-freeSteamKeys').setValue('17');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
});

test('validation finds invalid fields in other sections without losing the draft',async()=>{
  await browser.$('.header-button').click();
  await browser.$('#check-interval-freeSteamKeys').setValue('4');
  await selectSettingsSection('diagnostics');
  await browser.$('#log-file-size').setValue('0');
  await selectSettingsSection('updatesData');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false);
  await browser.$('.settings-validation-link').click();
  await waitForSettingFocus('check-interval-freeSteamKeys');
  assert.equal(await browser.$('[data-settings-section="sources"]').getAttribute('aria-selected'),'true');
  assert.equal(await browser.execute(()=>document.activeElement?.id),'check-interval-freeSteamKeys');
  await browser.$('#check-interval-freeSteamKeys').setValue('60');
  await selectSettingsSection('notifications');
  await browser.$('.settings-validation-link').click();
  await waitForSettingFocus('log-file-size');
  assert.equal(await browser.$('[data-settings-section="diagnostics"]').getAttribute('aria-selected'),'true');
  assert.equal(await browser.execute(()=>document.activeElement?.id),'log-file-size');
  assert.equal(await browser.$('#log-file-size').getValue(),'0');
  await browser.$('#log-file-size').setValue('2');
  assert.equal(await browser.$('.dialog-actions .primary-button').isEnabled(),false,'restoring all values clears the dirty state');
  assert.equal(await browser.execute(()=>window.__uiTest.calls.some(call=>call.command==='update_settings')),false);
});

test('startup-in-tray and close-to-tray checkboxes save independently',async()=>{
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  assert.equal(await browser.$('#start-in-tray').isSelected(),false);
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  for(const startup of [false,true]) {
    for(const close of [false,true]) {
      await browser.$('.header-button').click();
      await selectSettingsSection('application');
      for(const [id,value] of [['start-in-tray',startup],['keep-running-in-tray',close]]) {
        const input=await browser.$(`#${id}`);
        if(await input.isSelected()!==value) await input.click();
      }
      await saveOrCloseSettings();
      const saved=await browser.execute(()=>window.__uiTest.calls.filter(c=>c.command==='update_settings').at(-1).args.settings);
      assert.equal(saved.startInTray,startup);
      assert.equal(saved.keepRunningInTray,close);
      await browser.$('.header-button').click();
      await selectSettingsSection('application');
      assert.equal(await browser.$('#start-in-tray').isSelected(),startup);
      assert.equal(await browser.$('#keep-running-in-tray').isSelected(),close);
      await browser.keys('Escape');
      await browser.$('[role="dialog"]').waitForExist({reverse:true});
    }
  }
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  assert.equal(await browser.$('#keep-running-in-tray').isEnabled(),true);
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  assert.equal(await browser.$('#start-in-tray').isEnabled(),false);
  await browser.$('#start-at-login').click();
  assert.equal(await browser.$('#start-in-tray').isEnabled(),true);
  assert.equal(await browser.$('#start-in-tray').isSelected(),true);
});

test('history fills the window while sources stay compact and long checkbox labels do not shrink inputs',async()=>{
  try {
    await browser.url(server.url+'?history');
    await browser.$('.history-item:first-child').waitForDisplayed();
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const [width,height] of [[960,720],[1100,900],[960,600]]) {
      await browser.setWindowSize(width,height+chromeHeight);
      const geometry=await browser.execute(()=>{
        const card=document.querySelector('.history-card').getBoundingClientRect();
        const sources=document.querySelector('.sources-card').getBoundingClientRect();
        const actions=document.querySelector('.source-actions').getBoundingClientRect();
        const list=document.querySelector('.history-list');
        const sourcesList=document.querySelector('.source-list');
        return {bottom:card.bottom,sourcesBottom:sources.bottom,viewport:innerHeight,
          actionsBottom:actions.bottom,
          sourceEmptySpace:sourcesList.getBoundingClientRect().bottom-sourcesList.lastElementChild.getBoundingClientRect().bottom,
          padding:parseFloat(getComputedStyle(document.querySelector('.app-shell')).paddingBottom),
          pageScroll:document.documentElement.scrollHeight>innerHeight,
          listScroll:list.scrollHeight>list.clientHeight};
      });
      assert.ok(Math.abs(geometry.viewport-geometry.bottom-geometry.padding)<2,JSON.stringify(geometry));
      assert.ok(geometry.sourcesBottom<=geometry.bottom+1,JSON.stringify(geometry));
      assert.ok(geometry.sourceEmptySpace<=1,JSON.stringify(geometry));
      assert.ok(geometry.actionsBottom<geometry.sourcesBottom,JSON.stringify(geometry));
      assert.equal(geometry.pageScroll,false);
      assert.equal(geometry.listScroll,true);
    }
    await browser.$('.header-button').click();
    await selectSettingsSection('application');
  await browser.$('button=Русский').click();
    const widths=await browser.execute(()=>Array.from(document.querySelectorAll('.check-setting-details input')).filter(input=>input.getClientRects().length>0).map(input=>input.getBoundingClientRect().width));
    assert.ok(widths.length>=3);
    assert.ok(widths.every(width=>width===17),JSON.stringify(widths));
  } finally { await browser.setWindowSize(1100,900); }
});

test('valid interval saves into isolated in-memory settings', async () => {
  await browser.$('.header-button').click();
  await browser.$('#check-interval-freeSteamKeys').setValue('90');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.$('.header-button').click();
  assert.equal(await browser.$('#check-interval-freeSteamKeys').getValue(), '90');
});

test('main check has hover feedback and notification testing works only in settings with pending and retry feedback', async () => {
  const primary = await browser.$('.source-actions .primary-button');
  assert.equal(await browser.$$('.source-actions button').length,1);
  assert.equal(await browser.$('.sources-card .test-notification-button').isExisting(),false);
  await browser.$('.header-button').moveTo();
  const layout = await browser.execute(() => {
    const first = document.querySelector('.source-actions .primary-button');
    return {
      firstCursor: getComputedStyle(first).cursor,
      firstColor: getComputedStyle(first).backgroundColor,
    };
  });
  assert.equal(layout.firstCursor, 'pointer');
  await primary.moveTo();
  await browser.waitUntil(async () => await browser.execute(() => getComputedStyle(document.querySelector('.source-actions .primary-button')).backgroundColor) !== layout.firstColor);
  await browser.$('.header-button').click();
  await selectSettingsSection('notifications');
  const secondary=browser.$('.notification-test .test-notification-button');
  await secondary.scrollIntoView({block:'center'});
  await browser.$('.dialog-header').moveTo();
  const settingsButton=await browser.execute(()=>{
    const button=document.querySelector('.notification-test .test-notification-button');
    return {cursor:getComputedStyle(button).cursor,color:getComputedStyle(button).backgroundColor};
  });
  assert.equal(settingsButton.cursor,'pointer');
  await secondary.moveTo();
  await browser.waitUntil(async()=>await browser.execute(()=>getComputedStyle(document.querySelector('.notification-test .test-notification-button')).backgroundColor)!==settingsButton.color);
  await browser.execute(()=>{window.__uiTest.pauseTestNotification=true;});
  await secondary.click();
  await browser.waitUntil(async()=>!(await secondary.isEnabled()));
  assert.equal(await secondary.getText(),'Sending…');
  assert.equal(await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='send_test_notification').length),1);
  await browser.execute(()=>{window.__uiTest.pauseTestNotification=false;window.__uiTest.finishTestNotification();});
  await secondary.waitForEnabled();
  await browser.$('.notification-test [role="status"]').waitForDisplayed();
  assert.equal(await browser.$('.notification-test [role="status"]').getText(),'The test system notification was sent.');
  await browser.execute(()=>{window.__uiTest.failTestNotifications=1;});
  await secondary.click();
  await browser.$('.notification-test .error[role="alert"]').waitForDisplayed();
  assert.equal(await browser.$('.notification-test .error[role="alert"]').getText(),'The system notification could not be shown.');
  await secondary.click();
  await browser.$('.notification-test [role="status"]').waitForDisplayed();
  assert.equal(await browser.$('.notification-test [role="alert"]').isExisting(),false);
  const calls=await browser.execute(()=>window.__uiTest.calls);
  assert.equal(calls.filter(call=>call.command==='send_test_notification').length,3);
  assert.equal(calls.some(call=>call.command==='update_settings'),false,'testing notifications does not save a settings draft');
  await browser.keys('Escape');
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.$('.sources-card .monitor-message').isExisting(),false);
  await browser.$('.header-button').click();
  await selectSettingsSection('notifications');
  assert.equal(await browser.$('.notification-test .monitor-message').isExisting(),false,'feedback resets when settings reopen');
});


test('GOG filter can be saved, reopened and re-enabled in both languages',async()=>{
  await browser.$('.header-button').click();
  await selectSettingsSection('notifications');
  const gog=()=>browser.$('[role="dialog"]').$('label*=GOG').$('input');
  assert.equal(await gog().isSelected(),true);
  await gog().click();
  assert.equal(await browser.$('.settings-draft-status').getText(),'You have unsaved changes');
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  let writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.equal(writes.at(-1).args.settings.gogNotificationsEnabled,false);
  assert.equal(writes.at(-1).args.settings.steamNotificationsEnabled,true);
  assert.equal(writes.at(-1).args.settings.epicGamesNotificationsEnabled,true);
  await browser.$('.header-button').click();
  await selectSettingsSection('notifications');
  assert.equal(await gog().isSelected(),false);
  await selectSettingsSection('application');
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.$('.header-button').click();
  await selectSettingsSection('notifications');
  const russianGog=await browser.$('[role="dialog"]').$('label*=GOG').$('input');
  assert.equal(await russianGog.isSelected(),false);
  await russianGog.click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  writes=await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='update_settings'));
  assert.equal(writes.at(-1).args.settings.gogNotificationsEnabled,true);
});

test('notification history shows Steam, Epic Games, GOG and an explicit unknown platform',async()=>{
  await browser.url(server.url+'?history');
  await browser.$('.history-item:first-child').waitForDisplayed();
  const rows=await browser.$$('.history-item small');
  for(const [index,name] of ['Steam','Epic Games','GOG','Unknown platform'].entries()) {
    assert.equal(await rows[index].getText(),name+' · Givee Club · Giveaway');
  }
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.match(await browser.$('.history-item:nth-child(3) small').getText(),/^GOG ·/);
  assert.match(await browser.$('.history-item:nth-child(4) small').getText(),/^Платформа не определена ·/);
});

test('FreeSteamKeys notifications show the source name and Steam store in history',async()=>{
  await browser.url(server.url+'?freeSteamKeys');
  await browser.$('.history-item:first-child').waitForDisplayed();
  assert.equal(await browser.$('.history-item strong').getText(),'One Gun Guy');
  assert.equal(await browser.$('.history-item small').getText(),'Steam · FreeSteamKeys · Giveaway');
  await browser.$('.header-button').click();
  await selectSettingsSection('application');
  await browser.$('button=Русский').click();
  await browser.$('.dialog-actions .primary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  assert.equal(await browser.$('.history-item small').getText(),'Steam · FreeSteamKeys · Раздача');
});

test('history keeps every entry and provenance without filters, with unread markers beside titles',async()=>{
  await browser.url(server.url+'?merged');
  await browser.$('[data-notification-id="merged-1"]').waitForDisplayed();
  await browser.$('.header-button').moveTo();
  assert.equal(await browser.$('[data-notification-id="merged-1"] small').getText(),'Steam · Givee Club · found via FreeSteamKeys, Givee Club · Giveaway');
  assert.equal(await browser.$('input[type="search"]').isExisting(),false);
  assert.equal(await browser.$('.history-filters, .history-unread-filter, .history-filter-summary, .filter-reset').isExisting(),false);
  assert.equal(await browser.$('.history-card [role="combobox"], .history-card input').isExisting(),false);
  assert.equal(await browser.$$('.history-item').length,2);
  assert.equal(await browser.$('[data-notification-id="merged-1"] .history-item-title .unread-dot').isExisting(),true);
  assert.equal(await browser.$('[data-notification-id="merged-2"] .unread-dot').isExisting(),false);
  assert.equal(await browser.$('.history-item-actions .unread-dot').isExisting(),false);
  const marker=await browser.execute(()=>{
    const title=document.querySelector('[data-notification-id="merged-1"] .history-item-title');
    const dot=title.querySelector('.unread-dot').getBoundingClientRect(),text=title.querySelector('strong').getBoundingClientRect();
    const unread=getComputedStyle(document.querySelector('[data-notification-id="merged-1"]')),read=getComputedStyle(document.querySelector('[data-notification-id="merged-2"]'));
    return {dotRight:dot.right,titleLeft:text.left,gap:text.left-dot.right,dotTop:dot.top,dotBottom:dot.bottom,titleTop:text.top,titleBottom:text.bottom,
      unreadBackground:unread.backgroundColor,readBackground:read.backgroundColor,borders:['Top','Right','Bottom','Left'].map(side=>unread['border'+side+'Width'])};
  });
  assert.ok(marker.gap>=4&&marker.gap<=12,JSON.stringify(marker));
  assert.ok(marker.dotTop>=marker.titleTop&&marker.dotBottom<=marker.titleBottom,JSON.stringify(marker));
  assert.notEqual(marker.unreadBackground,marker.readBackground);
  assert.equal(new Set(marker.borders).size,1,'unread state uses no thicker left stripe');
  await browser.execute(()=>window.__uiTest.emit('history-changed'));
  await browser.waitUntil(async()=>await browser.$$('.history-item').length===2);
  await browser.refresh();
  await browser.$('[data-notification-id="merged-1"]').waitForDisplayed();
  assert.deepEqual(await browser.$$('.history-item strong').map(row=>row.getText()),['Приключение ONE','Different ONE']);
});

test('read controls remain independent while clicking dates and card padding opens exactly once',async()=>{
  await browser.url(server.url+'?merged&history');
  await browser.$('[data-notification-id="merged-1"]').waitForExist();
  await browser.$('[data-notification-id="merged-1"]').scrollIntoView({block:'center'});
  await browser.$('[data-notification-id="merged-1"] .mark-read-button').click();
  await browser.$('[data-notification-id="merged-1"] .unread-dot').waitForExist({reverse:true});
  assert.ok(await browser.$$('.history-item').length<=30,'only a visible history window is mounted');
  assert.equal(await browser.execute(()=>window.__uiTest.history.length),42);
  const calls=await browser.execute(()=>window.__uiTest.calls);
  assert.equal(calls.filter(call=>call.command==='mark_notification_read').length,1);
  assert.equal(calls.some(call=>call.command==='open_notification'),false);
  assert.equal(await browser.$('button button').isExisting(),false);
  await clickCardArea('[data-notification-id="merged-1"] time');
  await browser.waitUntil(async()=>await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='open_notification').length===1));
  await browser.$('[data-notification-id="merged-2"]').scrollIntoView({block:'center'});
  await clickCardArea('[data-notification-id="merged-2"]',true);
  await browser.waitUntil(async()=>await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='open_notification').length===2));
  assert.deepEqual(await browser.execute(()=>window.__uiTest.calls.filter(call=>call.command==='open_notification').map(call=>call.args.notificationId)),['merged-1','merged-2']);
  await browser.$('.history-toolbar button').click();
  await browser.waitUntil(async()=>await browser.execute(()=>window.__uiTest.history.every(item=>item.readAtUtc)));
  assert.equal(await browser.$('.history-toolbar > span').getText(),'Unread: 0');
  assert.ok(await browser.$$('.history-item').length<=30,'mark all keeps history virtualized');
  assert.equal(await browser.execute(()=>window.__uiTest.history.length),42);
  assert.equal(await browser.$('.unread-dot, .mark-read-button').isExisting(),false);
});

test('per-source status events update immediately and manual checks acknowledge already running sources',async()=>{
  await browser.execute(()=>{
    const state=window.__uiTest.monitor.sourceStates.find(state=>state.source==='freeSteamKeys');
    Object.assign(state,{phase:'checking',lastSuccessfulCheckUtc:'2026-11-07T08:00:00Z',nextCheckAtUtc:null});
    window.__uiTest.monitor.isChecking=true;window.__uiTest.emit('monitor-changed');
  });
  await browser.waitUntil(async()=>await browser.$('[data-source="freeSteamKeys"] .source-phase').getText()==='Checking');
  assert.equal(await browser.$('[data-source="freeSteamKeys"] time').isExisting(),false);
  assert.equal(await browser.$('.source-actions .primary-button').isEnabled(),true);
  await browser.$('.source-actions .primary-button').click();
  await browser.waitUntil(async()=>(await browser.$('.sources-card').getText()).includes('Sources started: 2. Already checking: 1.'));
  assert.equal(await browser.$('[data-source="giveeClub"] .source-phase').getText(),'Checking');
  assert.equal(await browser.$('[data-source="zeros"] .source-phase').getText(),'Disabled');
});

test('minimum and original window sizes keep all source/history controls reachable with an update in both languages',async()=>{
  try {
    await browser.url(server.url+'?history&update');
    await browser.$('.history-item:first-child').waitForDisplayed();
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const language of ['en','ru']) {
      if(language==='ru') {
        await browser.$('.header-button').click();await selectSettingsSection('application');await browser.$('button=Русский').click();
        await browser.$('.dialog-actions .primary-button').click();await browser.$('[role="dialog"]').waitForExist({reverse:true});
      }
      for(const width of [960,760]) {
        await browser.setWindowSize(width,600+chromeHeight);
        const geometry=await browser.execute(()=>{
          const selectors=['.status-panel','.update-banner','.source-actions','.history-toolbar'];
          const rects=selectors.map(selector=>{const rect=document.querySelector(selector).getBoundingClientRect();return{selector,top:rect.top,bottom:rect.bottom,left:rect.left,right:rect.right};});
          const sourceList=document.querySelector('.source-list');sourceList.scrollTop=sourceList.scrollHeight;
          return{width:innerWidth,height:innerHeight,scroll:document.documentElement.scrollHeight>innerHeight,rects,zeros:document.querySelector('[data-source="zeros"] .source-row').getBoundingClientRect().bottom,sourceBottom:sourceList.getBoundingClientRect().bottom,historyHeight:document.querySelector('.history-list').clientHeight};
        });
        assert.equal(geometry.scroll,false,JSON.stringify(geometry));
        assert.ok(geometry.rects.every(rect=>rect.top>=0&&rect.bottom<=geometry.height&&rect.left>=0&&rect.right<=geometry.width),JSON.stringify(geometry));
        assert.ok(geometry.zeros<=geometry.sourceBottom+1,JSON.stringify(geometry));
        assert.ok(geometry.historyHeight>=80,JSON.stringify(geometry));
        assert.equal(await browser.$('.source-actions .primary-button').isDisplayed(),true);
        assert.equal(await browser.$('.sources-card .test-notification-button').isExisting(),false);
      }
    }
  } finally {await browser.setWindowSize(1100,900);}
});


test('history cleanup remains available while source HTTP requests are in flight',async()=>{
  await browser.url(server.url+'?history');
  await browser.$('.history-item:first-child').waitForDisplayed();
  await browser.execute(()=>{
    const monitor=window.__uiTest.monitor;monitor.isChecking=true;
    monitor.sourceStates.find(source=>source.source==='giveeClub').phase='checking';
    window.confirm=()=>true;window.__uiTest.emit('monitor-changed');
  });
  await browser.$('.header-button').click();
  await selectSettingsSection('updatesData');
  await browser.$('.danger-button').scrollIntoView({block:'center'});
  assert.equal(await browser.$('.danger-button').isEnabled(),true);
  assert.equal(await browser.$('#clear-data-busy-hint').isExisting(),false);
  await browser.$('.danger-button').click();
  await browser.waitUntil(async()=>await browser.execute(()=>window.__uiTest.calls.some(call=>call.command==='clear_giveaway_data')));
  assert.equal(await browser.execute(()=>window.__uiTest.history.length),0);
});

test('large history loads fixed pages only on demand and virtual scrolling visits every variable-height record',async()=>{
  await browser.url(server.url+'?largeHistory');
  await browser.$('[data-notification-id="0"]').waitForDisplayed();
  await browser.waitUntil(async()=>await browser.$('.history-list').getAttribute('aria-busy')==='false');
  assert.equal(await browser.$('.history-toolbar > span').getText(),'Unread: 180','the counter includes unloaded pages');
  const initial=await browser.execute(()=>window.__uiTest.historyPageResponses);
  assert.deepEqual(initial.map(page=>({cursor:page.cursor,count:page.count})),[{cursor:null,count:50}],'startup reads only one first page after subscribing');
  const idleRequests=await browser.execute(()=>window.__uiTest.historyPageRequests.length);
  await browser.execute(()=>window.__uiTest.emit('monitor-changed'));
  await browser.pause(3500);
  assert.equal(await browser.execute(()=>window.__uiTest.historyPageRequests.length),idleRequests,'neither timer polling nor monitor events reload history');
  const walked=await walkHistory(browser,'239');
  assert.deepEqual(walked.seen,Array.from({length:240},(_,index)=>String(index)),'scrolling has no skipped or repeated records');
  assert.ok(walked.heights.length>=2,'long titles create genuinely different measured heights');
  const pages=await browser.execute(()=>[...new Map(window.__uiTest.historyPageResponses.map(page=>[page.cursor,page])).values()]);
  assert.deepEqual(pages.map(page=>page.count),[50,50,50,50,40]);
  assert.deepEqual(pages.flatMap(page=>page.ids),Array.from({length:240},(_,index)=>String(index)));
  assert.equal(new Set(pages.slice(1).map(page=>page.cursor)).size,4,'each continuation uses a distinct opaque cursor');
  await scrollHistory(browser,-walked.last.scrollHeight);
  await browser.$('[data-notification-id="0"]').waitForDisplayed();
},{timeout:120_000});

test('new notifications preserve the old history anchor and read controls affect unloaded records',async()=>{
  // An event arriving after SQLite's first snapshot but before its IPC reply
  // must schedule a fresh page instead of committing only the stale snapshot.
  await browser.url(server.url+'?largeHistory&pauseHistory');
  await browser.waitUntil(async()=>await browser.execute(()=>typeof window.__uiTest.finishHistoryPage==='function'));
  await browser.execute(()=>{
    window.__uiTest.history.unshift({id:'initial-race-arrival',source:'giveeClub',store:'steam',kind:'giveaway',title:'Arrived while the first history reply was pending',url:'https://givee.club/event/initial-race-arrival',receivedAtUtc:'2026-11-15T11:00:00Z',readAtUtc:null});
    window.__uiTest.emit('history-changed');
  });
  await browser.execute(()=>window.__uiTest.finishHistoryPage());
  await browser.$('[data-notification-id="initial-race-arrival"]').waitForDisplayed();
  await browser.waitUntil(async()=>await browser.execute(()=>document.querySelector('.history-list')?.getAttribute('aria-busy')==='false'));
  assert.equal(await browser.$('.history-list').getAttribute('data-history-first-id'),'initial-race-arrival');
  assert.ok(await browser.execute(()=>window.__uiTest.historyPageRequests.length)>=2,'the pending initial snapshot is resynchronized after its event');
  await browser.url(server.url+'?largeHistory');
  await browser.$('[data-notification-id="0"]').waitForDisplayed();
  for(let step=0;step<120;step++) {
    const view=await historyViewport(browser);
    if(view.visible.some(row=>Number(row.id)>=110))break;
    await scrollHistory(browser,view.height*0.7);
  }
  const before=await historyViewport(browser),anchor=historyAnchor(before);
  assert.ok(Number(anchor.id)>=100,'the reader is on an older page');
  await browser.execute(()=>{
    window.__uiTest.history.unshift({id:'new-arrival',source:'giveeClub',store:'steam',kind:'giveaway',title:'New arrival while reading older history',url:'https://givee.club/event/new-arrival',receivedAtUtc:'2026-11-15T12:00:00Z',readAtUtc:null});
    window.__uiTest.emit('history-changed');
  });
  await browser.waitUntil(async()=>await browser.execute(()=>{
    const list=document.querySelector('.history-list');
    return list?.getAttribute('aria-busy')==='false'&&list.dataset.historyFirstId==='new-arrival';
  }),{timeoutMsg:'all loaded history pages are committed after an arrival'});
  await browser.waitUntil(async()=>{
    const view=await historyViewport(browser),row=view.visible.find(row=>row.id===anchor.id);
    return row&&Math.abs(row.top-anchor.top)<=2;
  },{timeoutMsg:'a new first page preserves the visible record and its position'});
  assert.equal(await browser.$('.history-toolbar > span').getText(),'Unread: 181');
  const unread=await browser.execute(()=>{
    const list=document.querySelector('.history-list').getBoundingClientRect();
    return [...document.querySelectorAll('.history-item.unread')].find(row=>{const rect=row.getBoundingClientRect();return rect.top>=list.top&&rect.bottom<=list.bottom;})?.dataset.notificationId;
  });
  assert.ok(unread,'an unread older record is visible');
  await browser.$(`[data-notification-id="${unread}"] .mark-read-button`).click();
  await browser.$(`[data-notification-id="${unread}"] .mark-read-button`).waitForExist({reverse:true});
  await browser.waitUntil(async()=>await browser.$('.history-toolbar > span').getText()==='Unread: 180');
  assert.equal(await browser.execute(()=>window.__uiTest.calls.some(call=>call.command==='open_notification')),false);
  await browser.$('.history-toolbar button').click();
  await browser.waitUntil(async()=>await browser.$('.history-toolbar > span').getText()==='Unread: 0');
  assert.equal(await browser.execute(()=>window.__uiTest.history.filter(item=>!item.readAtUtc).length),0,'mark all also updates never-mounted records');
  await scrollHistory(browser,-(await historyViewport(browser)).scrollHeight);
  await browser.$('[data-notification-id="new-arrival"]').waitForDisplayed();
  assert.equal(await browser.$('[data-notification-id="new-arrival"] .unread-dot').isExisting(),false);
  assert.equal(await browser.execute(()=>window.__uiTest.history.length),241);
},{timeout:120_000});

test('history page failures offer retry, retain loaded rows and clearing discards all pages',async()=>{
  await browser.url(server.url+'?largeHistory&historyError');
  await browser.$('.history-retry').waitForDisplayed();
  assert.equal(await browser.$('.history-item').isExisting(),false);
  await browser.execute(()=>{window.__uiTest.failHistoryPages=0;});
  await browser.$('.history-retry').click();
  await browser.$('[data-notification-id="0"]').waitForDisplayed();
  await browser.execute(()=>{window.__uiTest.failHistoryPages=1;window.__uiTest.historyFailureCursor='next';});
  for(let step=0;step<100;step++) {
    if(await browser.$('.history-load-more-retry').isDisplayed())break;
    const view=await historyViewport(browser);await scrollHistory(browser,view.height*0.75);
  }
  await browser.$('.history-load-more-retry').waitForDisplayed();
  assert.ok((await historyViewport(browser)).mounted.length>0,'a continuation failure keeps the loaded page');
  const failedCursor=await browser.execute(()=>window.__uiTest.historyPageRequests.at(-1));
  const requestCount=await browser.execute(()=>window.__uiTest.historyPageRequests.length);
  assert.ok(failedCursor,'a next-page failure retains its cursor');
  await browser.$('.history-load-more-retry').click();
  await browser.waitUntil(async()=>await browser.execute(cursor=>window.__uiTest.historyPageResponses.some(page=>page.cursor===cursor),failedCursor));
  assert.equal(await browser.execute(index=>window.__uiTest.historyPageRequests[index],requestCount),failedCursor,'retry first repeats the failed page before any further prefetch');
  assert.equal(await browser.$('.history-load-more-retry').isExisting(),false);
  await browser.execute(()=>{window.confirm=()=>true;});
  await browser.$('.header-button').click();await selectSettingsSection('updatesData');
  await browser.$('.danger-button').scrollIntoView({block:'center'});await browser.$('.danger-button').click();
  await browser.waitUntil(async()=>await browser.execute(()=>window.__uiTest.history.length)===0);
  await browser.$('.dialog-actions .secondary-button').click();
  await browser.$('[role="dialog"]').waitForExist({reverse:true});
  await browser.waitUntil(async()=>await browser.$$('.history-item').length===0);
  assert.equal(await browser.$('.history-toolbar > span').getText(),'Unread: 0');
  assert.equal(await browser.$('.history-load-more-retry').isExisting(),false);
},{timeout:120_000});

test('keyboard focus crosses virtual rows and long history remains readable after Russian resize',async()=>{
  try {
    await browser.url(server.url+'?largeHistory');
    await browser.$('[data-notification-id="0"]').waitForDisplayed();
    await browser.execute(()=>document.querySelector('[data-notification-id="0"] .history-open').focus());
    let previous=0;
    for(let step=0;step<100;step++) {
      const previousAction=await browser.execute(()=>document.activeElement?.closest('.history-item')?.dataset.notificationId+'|'+document.activeElement?.className);
      await browser.keys('Tab');
      await browser.waitUntil(async()=>await browser.execute(()=>document.activeElement?.closest('.history-item')?.dataset.notificationId+'|'+document.activeElement?.className)!==previousAction,{timeoutMsg:'Tab advances after virtual row mounting'});
      const focused=await browser.execute(()=>{
        const element=document.activeElement,row=element?.closest('.history-item');
        return row?{id:Number(row.dataset.notificationId),type:element.className}:null;
      });
      assert.ok(focused,'Tab stays in logical history order at an unmounted boundary');
      assert.ok(focused.id>=previous&&focused.id<=previous+1,JSON.stringify({previous,focused}));
      previous=focused.id;
      assert.ok((await browser.$$('.history-item')).length<=31,'focused rows do not grow the virtual DOM');
    }
    assert.ok(previous>=50,'keyboard navigation crossed a page boundary');
    await browser.keys(['Shift','Tab']);
    assert.ok(await browser.execute(()=>!!document.activeElement.closest('.history-item')),'reverse Tab keeps a valid virtual row focus');
    await browser.$('.header-button').click();await selectSettingsSection('application');await browser.$('button=Русский').click();
    await browser.$('.dialog-actions .primary-button').click();await browser.$('[role="dialog"]').waitForExist({reverse:true});
    const chromeHeight=await browser.execute(()=>outerHeight-innerHeight);
    for(const [width,height] of [[760,600],[1100,740]]) {
      await browser.setWindowSize(width,height+chromeHeight);
      await browser.waitUntil(async()=>await browser.execute(()=>document.documentElement.scrollWidth<=innerWidth));
      const layout=await browser.execute(()=>{
        const list=document.querySelector('.history-list').getBoundingClientRect();
        return {overflow:document.documentElement.scrollWidth>innerWidth,rows:[...document.querySelectorAll('.history-item')].map(row=>{const rect=row.getBoundingClientRect();return {left:rect.left,right:rect.right};}),left:list.left,right:list.right};
      });
      assert.equal(layout.overflow,false,JSON.stringify(layout));
      assert.ok(layout.rows.every(row=>row.left>=layout.left-1&&row.right<=layout.right+1),JSON.stringify(layout));
      assert.match(await browser.$('.history-toolbar > span').getText(),/Непрочитанных:/);
    }
  } finally {await browser.setWindowSize(1100,900);}
},{timeout:120_000});
