// Renderer-only fixture. Nothing here talks to Rust, SQLite, notifications or a real feed.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('../../dist/', import.meta.url));
const bridge = `
const settings = {checkIntervalMinutes:60,sourceCheckIntervals:{freeSteamKeys:60,giveeClub:60,gamesForFarm:60,zeros:60},httpTimeoutSeconds:30,httpRetryCount:2,
giveeClubEnabled:true,gamesForFarmEnabled:true,zerosEnabled:false,freeSteamKeysEnabled:true,steamNotificationsEnabled:true,
epicGamesNotificationsEnabled:true, gogNotificationsEnabled: true,notificationSoundEnabled:true,startAtLogin:false,autostartTarget:'gui',
startInTray:false,keepRunningInTray:false,languageCode:'en',verboseLogging:false,initialSyncMode:'silent',logMaxFileSizeMb:2,logFileCount:1,logDirectory:null};
const logging = {requestedAtStartup:{maxFileSizeMb:2,fileCount:1,directory:null},effectiveDirectory:'C:/Fixture logs',activeFilePath:'C:/Fixture logs/Giveaway Scout.log',maxFileSizeMb:2,fileCount:1,restartRequired:false,lastError:null,usingFallback:false};
const monitor={isChecking:false,lastStartedAtUtc:null,lastCompletedAtUtc:null,lastSuccessfulCheckUtc:null,lastError:null,lastNewItems:0,lastFailureCount:0,sourceFailures:[],sourceStates:['freeSteamKeys','giveeClub','gamesForFarm','zeros'].map(source=>({source,phase:source==='zeros'?'disabled':'waiting',lastStartedAtUtc:null,lastCompletedAtUtc:null,lastSuccessfulCheckUtc:null,nextCheckAtUtc:null,lastError:null,lastNewItems:0,unresolvedLinks:0}))};
const params = new URLSearchParams(location.search);
const historyCount = params.has('largeHistory') ? 240 : params.has('history') ? 40 : 0;
const history = Array.from({length:historyCount},(_,i)=>({id:String(i),source:'giveeClub',store:['steam','epicGames','gog','unknown'][i%4],kind:params.has('largeHistory')&&i%3===0?'raffle':'giveaway',
 title:'Fixture game '+i+(params.has('largeHistory')&&i%5===0?' — '+('A long title with spaces для проверки переноса '.repeat(7)):''),
 url:'https://givee.club/event/'+i,receivedAtUtc:'2026-10-25T08:00:00Z',readAtUtc:params.has('largeHistory')&&i%4===0?'2026-10-25T09:00:00Z':null}));
if (new URLSearchParams(location.search).has('freeSteamKeys')) history.push({id:'fsk-1',source:'FreeSteamKeys',store:'steam',kind:'giveaway',title:'One Gun Guy',url:'https://gleam.io/4cb3E/free-steam-keys-of-one-gun-guy',receivedAtUtc:'2026-11-07T08:00:00Z',readAtUtc:null});
if (new URLSearchParams(location.search).has('merged')) history.push(
 {id:'merged-1',campaignId:'campaign-1',source:'freeSteamKeys',originSource:'giveeClub',originName:'Givee Club',detectedSources:['freeSteamKeys','giveeClub'],store:'steam',kind:'giveaway',title:'Приключение ONE',url:'https://givee.club/ru/event/123',receivedAtUtc:'2026-11-07T08:00:00Z',readAtUtc:null},
 {id:'merged-2',source:'gamesForFarm',originSource:'gamesForFarm',detectedSources:['gamesForFarm'],store:'unknown',kind:'raffle',title:'Different ONE',url:'https://gamesforfarm.com/giveaway/456',receivedAtUtc:'2026-11-07T07:00:00Z',readAtUtc:'2026-11-07T08:00:00Z'});
const update={mode:'development',phase:'idle',currentVersion:'0.1.0',candidate:null,downloadedBytes:0,totalBytes:null,lastCheckedAtUtc:null,lastError:null};
if(new URLSearchParams(location.search).has('update')) Object.assign(update,{mode:'enabled',phase:'available',candidate:{token:'fixture-update',version:'2.0.0',notes:'Fixture update'}});
const callbacks=new Map(),listeners=new Map();let callbackId=0;
const emit=event=>{for(const listener of listeners.values())if(listener.event===event)callbacks.get(listener.handler)?.({event,id:1,payload:null});};
// The cursor stores its own immutable sort tuple, so deleting or prepending rows
// cannot turn it into a different offset. It is deliberately opaque to the UI.
const historyOrder = new Map(history.map((item,index)=>[item.id,history.length-index]));
let historySequence = history.length;
const historyKey = item => {if(!historyOrder.has(item.id))historyOrder.set(item.id,++historySequence);return [item.receivedAtUtc,historyOrder.get(item.id),item.id];};
const compareHistoryKeys = (left,right) => right[0].localeCompare(left[0]) || right[1]-left[1] || right[2].localeCompare(left[2]);
window.__uiTest = {calls:[],unexpected:[],monitor,logging,history,settings,update,emit,historyPageRequests:[],historyPageResponses:[],failHistoryPages:params.has('historyError')?100:0,historyFailureCursor:null,pauseHistoryPages:params.has('pauseHistory')?1:0,failSettingsWrites:0,failTestNotifications:0,pauseTestNotification:false,folderChoice:null};
window.__TAURI_INTERNALS__ = {transformCallback:callback=>{callbacks.set(++callbackId,callback);return callbackId;},unregisterCallback:id=>callbacks.delete(id),invoke:async(command,args)=>{
  window.__uiTest.calls.push({command,args});
  switch(command) {
    case 'get_app_state': return {appName:'Giveaway Scout',version:'1.0.0',platform:'windows',architecture:'x86_64',isDevelopment:new URLSearchParams(location.search).has('dev')};
    case 'get_update_state': return structuredClone(update);
    case 'get_notification_history': return history;
    case 'get_notification_history_page': {
      const cursor=args?.cursor??null;
      window.__uiTest.historyPageRequests.push(cursor);
      const failure=window.__uiTest.historyFailureCursor;
      if(window.__uiTest.failHistoryPages>0 && (failure===null || (failure==='initial' ? cursor===null : cursor!==null))) {
        window.__uiTest.failHistoryPages--;throw new Error('Fixture history request failed');
      }
      const key=cursor===null?null:JSON.parse(decodeURIComponent(cursor));
      const ordered=history.map(item=>({item,key:historyKey(item)})).sort((left,right)=>compareHistoryKeys(left.key,right.key));
      const remaining=key===null?ordered:ordered.filter(entry=>compareHistoryKeys(entry.key,key)>0);
      const page=remaining.slice(0,50);
      const nextCursor=remaining.length>50?encodeURIComponent(JSON.stringify(page.at(-1).key)):null;
      const result={items:page.map(entry=>structuredClone(entry.item)),nextCursor};
      if(window.__uiTest.pauseHistoryPages>0) {
        window.__uiTest.pauseHistoryPages--;
        await new Promise(resolve=>{window.__uiTest.finishHistoryPage=resolve;});
      }
      window.__uiTest.historyPageResponses.push({cursor,count:result.items.length,ids:result.items.map(item=>item.id),nextCursor});
      return result;
    }
    case 'get_logging_state': return {...structuredClone(logging),restartRequired:settings.logMaxFileSizeMb!==logging.requestedAtStartup.maxFileSizeMb||settings.logFileCount!==logging.requestedAtStartup.fileCount||settings.logDirectory!==logging.requestedAtStartup.directory};
    case 'choose_log_directory': return window.__uiTest.folderChoice;
    case 'open_log_directory': window.__uiTest.openedLogDirectory=logging.effectiveDirectory; return;
    case 'send_test_notification':
      if(window.__uiTest.pauseTestNotification) await new Promise(resolve=>{window.__uiTest.finishTestNotification=resolve;});
      if(window.__uiTest.failTestNotifications>0) {window.__uiTest.failTestNotifications--;throw new Error('Fixture notification failure');}
      return;
    case 'get_storage_snapshot': return {settings:structuredClone(settings),counts:{trackedGiveaways:history.length,notificationHistory:history.length,unreadNotifications:history.filter(item=>!item.readAtUtc).length},dataSizeBytes:8192,databaseRecovery:null,initialSyncChoiceRequired:false,monitor:structuredClone(monitor)};
    case 'update_settings':
      if (window.__uiTest.pauseSettingsWrite) {
        await new Promise(resolve => { window.__uiTest.finishSettingsWrite = resolve; });
      }
      if (window.__uiTest.failSettingsWrites > 0) {
        window.__uiTest.failSettingsWrites--; throw new Error('Fixture save failure');
      }
      Object.assign(settings,args.settings);for(const state of monitor.sourceStates){if(!settings[state.source+'Enabled'])state.phase='disabled';else if(state.phase==='disabled')state.phase='waiting';} emit('monitor-changed');return;
    case 'check_now': {
      const enabled=monitor.sourceStates.filter(state=>settings[state.source+'Enabled']);
      const alreadyRunningSources=enabled.filter(state=>state.phase==='checking').map(state=>state.source);
      const startedSources=enabled.filter(state=>state.phase!=='checking').map(state=>state.source);
      for(const state of enabled)state.phase='checking';monitor.isChecking=enabled.length>0;emit('monitor-changed');
      return {startedSources,alreadyRunningSources};
    }
    case 'open_notification':
    case 'mark_notification_read': {const item=history.find(item=>item.id===args.notificationId);if(item)item.readAtUtc='2026-11-07T09:00:00Z';emit('history-changed');return;}
    case 'clear_giveaway_data': history.splice(0);emit('history-changed');return;
    case 'mark_all_notifications_read': {const unread=history.filter(item=>!item.readAtUtc);for(const item of unread)item.readAtUtc='2026-11-07T09:00:00Z';emit('history-changed');return unread.length;}
    case 'plugin:event|listen': {const id=listeners.size+1;listeners.set(id,args);return id;}
    case 'plugin:event|unlisten': return;
    default: window.__uiTest.unexpected.push(command); throw new Error('Unimplemented test IPC: '+command);
  }
}};
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {unregisterListener:()=>{}};
`;

export async function startFixtureServer() {
  // Fail before starting a browser when the production frontend has not been built.
  await readFile(path.join(root, 'index.html'));
  const server = createServer(async (request, response) => {
    try {
      const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
      response.setHeader('Cache-Control', 'no-store');
      if (pathname === '/fixture-bridge.js') {
        response.setHeader('Content-Type', 'text/javascript');
        response.end(bridge);
        return;
      }
      const filename = path.resolve(root, '.' + (pathname === '/' ? '/index.html' : pathname));
      const relative = path.relative(root, filename);
      if (relative.startsWith('..') || path.isAbsolute(relative)) {
        response.writeHead(403).end();
        return;
      }
      let data = await readFile(filename);
      const extension = path.extname(filename);
      response.setHeader('Content-Type', ({'.html':'text/html','.js':'text/javascript','.css':'text/css','.svg':'image/svg+xml'})[extension] ?? 'application/octet-stream');
      if (extension === '.html') data = data.toString().replace('<head>', '<head><script src="/fixture-bridge.js"></script>');
      response.end(data);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  return {
    url: `http://127.0.0.1:${server.address().port}/`,
    close: () => new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())),
  };
}
