import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { verifyComponentInstallerScript, verifyComponentInstallerTranslations, verifyInstallerScript } from './check-installer-bundle.mjs';
import { windowsComponentsOverlay } from './prepare-windows-components.mjs';

const repository=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const tauri=path.join(repository,'src/GiveawayScout.Desktop/src-tauri');
const config=JSON.parse(fs.readFileSync(path.join(tauri,'tauri.conf.json'),'utf8'));
const nsis=config.bundle.windows.nsis;

test('one NSIS setup uses Windows language, RU/EN and English fallback without changing identity',()=>{
  assert.deepEqual(nsis.languages,['English','Russian']);
  assert.equal(nsis.displayLanguageSelector,false);
  assert.equal(config.identifier,'com.giveawayscout.desktop');
  assert.equal(nsis.installMode ?? 'currentUser','currentUser');
  assert.equal(nsis.template,undefined,'Experimental targets retain the upstream NSIS workflow');
});

test('Windows x64 uses a project-owned component template only at packaging time',()=>{
  const windows=JSON.parse(fs.readFileSync(path.join(tauri,'build-windows.conf.json'),'utf8'));
  assert.equal(windows.bundle.windows.nsis.template,'installer/components.nsi');
  const executable=path.resolve(tauri,'target-test/release/giveaway-scout-cli.exe');
  const overlay=windowsComponentsOverlay(executable);
  assert.deepEqual(overlay.bundle.resources,{[executable]:'giveaway-scout-cli.exe'});
  assert.equal(overlay.bundle.windows.nsis.template,windows.bundle.windows.nsis.template);
  assert.equal(config.bundle.resources,undefined,'Plain cargo tests must not require a CLI payload');
});

test('component setup keeps GUI optional, CLI opt-in, safe process checks and preserved update selection',()=>{
  const source=fs.readFileSync(path.join(tauri,'installer/components.nsi'),'utf8');
  assert.equal(verifyComponentInstallerScript(source).cliDefault,false);
  for(const mutated of [
    source.replace('Section /o "$(ScoutCliComponent)" SEC_CLI','Section "$(ScoutCliComponent)" SEC_CLI'),
    source.replace('Call ScoutValidateComponents','; removed validation'),
    source.replace('Call ScoutCheckProfile','; removed profile preflight'),
    source.replace('Call ScoutCheckProcesses','!insertmacro CheckIfAppIsRunning "$INSTDIR\\giveaway-scout.exe" "Scout"'),
    source.replace('QueryFullProcessImageNameW','UncheckedParent'),
    source.replace('Call un.ScoutMaintainInstallation','; removed PATH/autostart cleanup'),
  ]) assert.throws(()=>verifyComponentInstallerScript(mutated));
});

test('component page keeps names wide and binds both hover descriptions after section declarations',()=>{
  const source=fs.readFileSync(path.join(tauri,'installer/components.nsi'),'utf8');
  const result=verifyComponentInstallerScript(source);
  assert.equal(result.wideComponentList,true);
  assert.equal(result.componentHoverDescriptions,true);
  assert.equal(result.readableDescriptionStates,true);
  const layout='!define MUI_COMPONENTSPAGE_SMALLDESC';
  const block=source.match(/^!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN\r?\n[\s\S]*?^!insertmacro MUI_FUNCTION_DESCRIPTION_END\r?\n/m)?.[0];
  assert(block);
  for(const mutated of [
    source.replace(layout,''),
    source.replace(layout,'')+'\n'+layout,
    source.replace('!define MUI_COMPONENTSPAGE_TEXT_TOP "$(ScoutComponentsText)"',''),
    source.replace('!define MUI_COMPONENTSPAGE_TEXT_COMPLIST "$(ScoutComponentsList)"',''),
    source.replace('!define MUI_COMPONENTSPAGE_TEXT_DESCRIPTION_INFO "$(ScoutComponentsDescriptionInfo)"',''),
    source.replace('!define MUI_PAGE_CUSTOMFUNCTION_SHOW ScoutComponentsShow',''),
    source.replace('!define MUI_CUSTOMFUNCTION_ONMOUSEOVERSECTION ScoutDescriptionColors',''),
    source.replace('!define MUI_CUSTOMFUNCTION_ONMOUSEOVERSECTION ScoutDescriptionColors','')+'\n!define MUI_CUSTOMFUNCTION_ONMOUSEOVERSECTION ScoutDescriptionColors\n',
    source.replace('EnableWindow $mui.ComponentsPage.DescriptionText 1','EnableWindow $mui.ComponentsPage.DescriptionText 0'),
    source.replace('"666666" "F0F0F0"','"FFFFFF" "F0F0F0"'),
    source.replace("System::Call 'user32::InvalidateRect(p $mui.ComponentsPage.DescriptionText, p0, i1)'",''),
    source.replace(block,''),
    source.replace('MUI_DESCRIPTION_TEXT ${SEC_GUI} "$(ScoutGuiDescription)"','MUI_DESCRIPTION_TEXT ${SEC_CLI} "$(ScoutGuiDescription)"'),
    source.replace('MUI_DESCRIPTION_TEXT ${SEC_CLI} "$(ScoutCliDescription)"','MUI_DESCRIPTION_TEXT ${SEC_CLI} "$(UnknownDescription)"'),
    block+source.replace(block,''),
  ]) assert.throws(()=>verifyComponentInstallerScript(mutated));
});

test('updater handoff captures the native open error and accepts only exited or verified parents',()=>{
  const source=fs.readFileSync(path.join(tauri,'installer/components.nsi'),'utf8');
  const wait=source.match(/^Function ScoutWaitForParent\r?\n([\s\S]*?)^FunctionEnd/m)?.[1];
  assert(wait);
  // A native regression probe demonstrated that a second System::Call returned
  // ERROR_FILE_EXISTS (80), losing OpenProcess's ERROR_INVALID_PARAMETER (87).
  assert.match(wait,/OpenProcess\(i0x101000, i0, i r0\)p\.r1\?e'\r?\n\s*Pop \$ScoutWaitResult\r?\n\s*StrCpy \$ScoutWaitHandle \$1/);
  assert.doesNotMatch(wait,/GetLastError/,'Only the captured native error may decide that the PID no longer exists');
  assert.match(wait,/\$ScoutWaitHandle = 0[\s\S]*?\$ScoutWaitResult = 87 \$\{\|\} Return/);
  const firstWait=wait.indexOf('WaitForSingleObject(p $ScoutWaitHandle, i0)');
  const query=wait.indexOf('QueryFullProcessImageNameW');
  assert(firstWait>=0 && firstWait<query,'Check the retained process handle before querying a possibly exited image');
  const beforeQuery=wait.slice(firstWait,query);
  assert.match(beforeQuery,/\$0 = 0[\s\S]*?CloseHandle\(p \$ScoutWaitHandle\)'\r?\n\s*Return/);
  assert.match(beforeQuery,/\$0 != 258[\s\S]*?CloseHandle\(p \$ScoutWaitHandle\)'[\s\S]*?Call ScoutFail/,
    'A failed zero-time wait must not be treated as an exited parent');
  const queryFailure=wait.match(/\$\{If\} \$2 = 0\r?\n([\s\S]*?)\$\{EndIf\}/)?.[1];
  assert(queryFailure);
  assert.match(queryFailure,/WaitForSingleObject\(p \$ScoutWaitHandle, i0\)i\.r0/);
  assert.match(queryFailure,/StrCpy \$ScoutWaitResult \$0\r?\n\s*System::Call 'kernel32::CloseHandle\(p \$ScoutWaitHandle\)'/);
  assert.match(queryFailure,/\$ScoutWaitResult = 0 \$\{\|\} Return/);
  assert.match(queryFailure,/Call ScoutFail/,'A live process with an unreadable image must still be rejected');
  assert.equal((wait.match(/OpenProcess\(/g)??[]).length,1,'Rechecking the same handle must not follow a reused PID');
  assert.match(wait,/\$ScoutWaitPath != "\$INSTDIR\\\$\{MAINBINARYNAME\}\.exe"\r?\n\s*\$\{AndIf\} \$ScoutWaitPath != "\$INSTDIR\\giveaway-scout-cli\.exe"/);
  assert.match(wait,/WaitForSingleObject\(p \$ScoutWaitHandle, i30000\)i\.r0'\r?\n\s*StrCpy \$ScoutWaitResult \$0\r?\n\s*System::Call 'kernel32::CloseHandle\(p \$ScoutWaitHandle\)'/);
  assert.doesNotMatch(wait,/TerminateProcess|RmShutdown/);
});

test('component instructions and descriptions are localized and Russian uses guillemets',()=>{
  const hooks=fs.readFileSync(path.join(tauri,nsis.installerHooks),'utf8');
  assert.equal(verifyComponentInstallerTranslations(hooks).localizedComponentInstructions,true);
  assert.match(hooks,/^LangString ScoutGuiComponent 1049 "Графическое приложение"\r?$/m);
  assert.match(hooks,/^LangString ScoutCliComponent 1049 "Консольное приложение"\r?$/m);
  for(const mutated of [
    hooks.replace(/^LangString ScoutGuiDescription 1033 .*\r?$/m,''),
    hooks.replace(/^LangString ScoutCliDescription 1049 .*\r?$/m,''),
    hooks.replace(/^LangString ScoutComponentsDescriptionInfo 1049 .*\r?$/m,''),
    hooks.replace(/^LangString ScoutComponentsText 1049 .*$/m,line=>line.replace('«Далее»',String.raw`$\"Далее$\"`)),
    hooks.replace(/^(LangString ScoutComponentsText 1049 .*)«Далее»/m,'$1Далее'),
  ]) assert.throws(()=>verifyComponentInstallerTranslations(mutated));
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
