// Export a repository-native SVG through Chromium; no desktop screenshots or app profile.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repository=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const require=createRequire(path.join(repository,'src/GiveawayScout.Desktop/package.json'));
const {remote}=await import(pathToFileURL(require.resolve('webdriverio')).href);
const svg=fs.readFileSync(path.join(repository,'src/GiveawayScout.Desktop/src-tauri/installer/sidebar.svg'),'utf8');
const output=path.join(repository,'.tools/installer-branding/sidebar.png');
fs.mkdirSync(path.dirname(output),{recursive:true});
let browser;
try {
  browser=await remote({logLevel:'error',connectionRetryCount:0,capabilities:{
    browserName:'chrome','goog:chromeOptions':{args:[
      '--headless=new','--no-first-run','--force-device-scale-factor=1','--window-size=500,600',
    ]},
  }});
  await browser.url('data:text/html;base64,'+Buffer.from('<!doctype html><style>body{margin:0}svg{display:block}</style>'+svg).toString('base64'));
  await browser.execute(async()=>{await document.fonts.ready;});
  await browser.$('body > svg').saveScreenshot(output);
  const bytes=fs.readFileSync(output);
  if(bytes.readUInt32BE(16)!==164 || bytes.readUInt32BE(20)!==314) throw new Error('Unexpected sidebar PNG dimensions');
  console.log('Exported only the owned SVG document to .tools/installer-branding/sidebar.png (164 x 314).');
} finally { if(browser) await browser.deleteSession(); }
