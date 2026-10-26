// Read-only checks of generated NSIS inputs, NOT a launch or installed acceptance.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export function verifyInstallerScript(source, config) {
  const define=name=>{
    const match=source.match(new RegExp(`^!define ${name} "([^"\\r\\n]*)"`, 'm'));
    assert(match,`Missing ${name}`);
    return match[1];
  };
  const languages=[...source.matchAll(/^!insertmacro MUI_LANGUAGE "([^"]+)"/gm)].map(m=>m[1]);
  assert.deepEqual(languages,['English','Russian'],'Installer must contain both languages, English first');
  assert.equal(define('VERSION'),config.version);
  assert.equal(define('BUNDLEID'),config.identifier,'Wrong installation identity');
  assert.equal(define('DISPLAYLANGUAGESELECTOR'),'false','Use Windows language without a selector');
  const nsis=config.bundle.windows.nsis;
  for(const [macro,filename] of [['INSTALLERICON',nsis.installerIcon],['UNINSTALLERICON',nsis.uninstallerIcon],['SIDEBARIMAGE',nsis.sidebarImage]]) {
    const actual=define(macro).replaceAll('\\','/');
    assert(actual.endsWith('/'+filename),`Wrong ${macro} resource`);
  }
  return {languages,fallbackLanguage:'English',automaticLanguage:true};
}

export function main() {
  assert.equal(process.argv.length,2,'No arguments expected; use CARGO_TARGET_DIR if needed');
  const repository=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
  const tauri=path.join(repository,'src/GiveawayScout.Desktop/src-tauri');
  const config=JSON.parse(fs.readFileSync(path.join(tauri,'tauri.conf.json'),'utf8'));
  const directory=path.join(process.env.CARGO_TARGET_DIR ?? path.join(tauri,'target-windows-x64-next'),'release/nsis/x64');
  const source=fs.readFileSync(path.join(directory,'installer.nsi'),'utf8');
  const result=verifyInstallerScript(source,config);
  const hookInclude=source.replaceAll('\\','/').indexOf('/'+config.bundle.windows.nsis.installerHooks+'"');
  assert(hookInclude>=0 && hookInclude<source.indexOf('!insertmacro MUI_PAGE_WELCOME') &&
    hookInclude<source.indexOf('!insertmacro MUI_PAGE_DIRECTORY'),
    'Text overrides must be included before welcome and directory pages');
  assert(fs.readFileSync(path.join(directory,'English.nsh'),'utf8').length>0);
  assert.match(fs.readFileSync(path.join(directory,'Russian.nsh'),'utf8'),/[\u0400-\u04ff]/);
  console.log('PASS: compiled NSIS inputs contain EN/RU, automatic language, application icons and branded sidebar.');
  console.log(JSON.stringify({...result,nativeInstallerLaunched:false}));
}

if(process.argv[1] && import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href) main();
