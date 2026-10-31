// Renderer-only fixture. Nothing here talks to Rust, SQLite, notifications or a real feed.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('../../dist/', import.meta.url));
const bridge = `
const settings = {checkIntervalMinutes:60,httpTimeoutSeconds:30,httpRetryCount:2,
giveeClubEnabled:true,gamesForFarmEnabled:true,zerosEnabled:false,steamNotificationsEnabled:true,
epicGamesNotificationsEnabled:true,notificationSoundEnabled:true,startAtLogin:false,
startInTray:false,keepRunningInTray:false,languageCode:'en',verboseLogging:false,initialSyncMode:'silent'};
const monitor={isChecking:false,lastStartedAtUtc:null,lastCompletedAtUtc:null,lastSuccessfulCheckUtc:null,lastError:null,lastNewItems:0,lastFailureCount:0,sourceFailures:[]};
const history = new URLSearchParams(location.search).has('history') ? Array.from({length:40},(_,i)=>({id:String(i),source:'giveeClub',kind:'giveaway',title:'Fixture game '+i,url:'https://givee.club/event/'+i,receivedAtUtc:'2026-10-25T08:00:00Z',readAtUtc:null})) : [];
window.__uiTest = {calls:[],unexpected:[],monitor,failSettingsWrites:0};
window.__TAURI_INTERNALS__ = {transformCallback:()=>1,unregisterCallback:()=>{},invoke:async(command,args)=>{
  window.__uiTest.calls.push({command,args});
  switch(command) {
    case 'get_app_state': return {appName:'Giveaway Scout',version:'1.0.0',platform:'windows',architecture:'x86_64',isDevelopment:new URLSearchParams(location.search).has('dev')};
    case 'get_update_state': return {mode:'development',phase:'idle',currentVersion:'0.1.0',candidate:null,downloadedBytes:0,totalBytes:null,lastCheckedAtUtc:null,lastError:null};
    case 'get_notification_history': return history;
    case 'get_storage_snapshot': return {settings:structuredClone(settings),counts:{trackedGiveaways:2,notificationHistory:0,unreadNotifications:0},dataSizeBytes:8192,databaseRecovery:null,initialSyncChoiceRequired:false,monitor:structuredClone(monitor)};
    case 'update_settings':
      if (window.__uiTest.pauseSettingsWrite) {
        await new Promise(resolve => { window.__uiTest.finishSettingsWrite = resolve; });
      }
      if (window.__uiTest.failSettingsWrites > 0) {
        window.__uiTest.failSettingsWrites--; throw new Error('Fixture save failure');
      }
      Object.assign(settings,args.settings); return;
    case 'plugin:event|listen': return 1;
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
