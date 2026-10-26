import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { verifyInstallerScript } from './check-installer-bundle.mjs';

const repository=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const tauri=path.join(repository,'src/GiveawayScout.Desktop/src-tauri');
const config=JSON.parse(fs.readFileSync(path.join(tauri,'tauri.conf.json'),'utf8'));
const nsis=config.bundle.windows.nsis;

test('one NSIS setup uses Windows language, RU/EN and English fallback without changing identity',()=>{
  assert.deepEqual(nsis.languages,['English','Russian']);
  assert.equal(nsis.displayLanguageSelector,false);
  assert.equal(config.identifier,'com.giveawayscout.desktop');
  assert.equal(nsis.installMode ?? 'currentUser','currentUser');
  assert.equal(nsis.template,undefined,'Keep the upstream NSIS workflow');
});

test('setup and uninstaller use the existing multi-resolution app ICO',()=>{
  assert.equal(nsis.installerIcon,'icons/icon.ico');
  assert.equal(nsis.uninstallerIcon,nsis.installerIcon);
  const bytes=fs.readFileSync(path.join(tauri,nsis.installerIcon));
  assert.equal(bytes.readUInt16LE(0),0);
  assert.equal(bytes.readUInt16LE(2),1);
  const sizes=Array.from({length:bytes.readUInt16LE(4)},(_,i)=>bytes[6+16*i] || 256);
  for(const size of [16,32,256]) assert(sizes.includes(size),`Missing ${size}px icon`);
});

test('sidebar is an opaque uncompressed 24-bit 164 x 314 BMP3 asset',()=>{
  assert.equal(nsis.sidebarImage,'installer/sidebar.bmp');
  const bytes=fs.readFileSync(path.join(tauri,nsis.sidebarImage));
  assert.equal(bytes.subarray(0,2).toString(),'BM');
  assert.equal(bytes.readUInt32LE(2),bytes.length);
  assert.equal(bytes.readUInt32LE(14),40);
  assert.equal(bytes.readInt32LE(18),164);
  assert.equal(bytes.readInt32LE(22),314);
  assert.equal(bytes.readUInt16LE(28),24);
  assert.equal(bytes.readUInt32LE(30),0);
});

test('sidebar reuses the original SVG logo rather than introducing another app icon',()=>{
  const source=fs.readFileSync(path.join(tauri,'icon-source.svg'),'utf8');
  const sidebar=fs.readFileSync(path.join(tauri,'installer/sidebar.svg'),'utf8');
  const original=source.match(/<svg[^>]*>([\s\S]*)<\/svg>/)?.[1];
  const reused=sidebar.match(/<svg id="app-logo"[^>]*>([\s\S]*?)<\/svg>/)?.[1];
  assert(original && reused);
  const normalize=value=>value.replace(/\s+/g,' ').trim();
  assert.equal(normalize(reused),normalize(original));
});

test('generated-script guard rejects old English-only setup, empty icons and unwanted language selection',()=>{
  const source=`!define VERSION "${config.version}"\n!define BUNDLEID "${config.identifier}"\n!define DISPLAYLANGUAGESELECTOR "false"\n!define INSTALLERICON "C:/repo/icons/icon.ico"\n!define UNINSTALLERICON "C:/repo/icons/icon.ico"\n!define SIDEBARIMAGE "C:/repo/installer/sidebar.bmp"\n!insertmacro MUI_LANGUAGE "English"\n!insertmacro MUI_LANGUAGE "Russian"\n`;
  assert.equal(verifyInstallerScript(source,config).automaticLanguage,true);
  for(const mutated of [
    source.replace('!insertmacro MUI_LANGUAGE "Russian"',''),
    source.replace('DISPLAYLANGUAGESELECTOR "false"','DISPLAYLANGUAGESELECTOR "true"'),
    source.replace('INSTALLERICON "C:/repo/icons/icon.ico"','INSTALLERICON ""'),
    source.replace('SIDEBARIMAGE "C:/repo/installer/sidebar.bmp"','SIDEBARIMAGE ""'),
    source.replace(config.identifier,'com.giveawayscout.desktop.e2e'),
  ]) assert.throws(()=>verifyInstallerScript(mutated,config));
});
