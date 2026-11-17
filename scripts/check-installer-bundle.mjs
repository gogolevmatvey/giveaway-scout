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

export function verifyComponentInstallerScript(source) {
  const section = name => {
    const match = source.match(new RegExp(`^Section ${name}\\r?\\n([\\s\\S]*?)^SectionEnd`, 'm'));
    assert(match, `Missing component section: ${name}`);
    return match[1];
  };
  const gui = section('"\\$\\(ScoutGuiComponent\\)" SEC_GUI');
  const cli = section('/o "\\$\\(ScoutCliComponent\\)" SEC_CLI');
  const preflight = section('"-EarlyChecks"');
  const page = source.indexOf('!insertmacro MUI_PAGE_COMPONENTS');
  const layout = source.search(/^!define MUI_COMPONENTSPAGE_SMALLDESC\s*$/m);
  assert(layout >= 0 && layout < page, 'Use the wide component list with descriptions below it');
  for (const [macro, text] of [
    ['TEXT_TOP', 'ScoutComponentsText'],
    ['TEXT_COMPLIST', 'ScoutComponentsList'],
    ['TEXT_DESCRIPTION_INFO', 'ScoutComponentsDescriptionInfo'],
  ]) {
    const definition = `!define MUI_COMPONENTSPAGE_${macro} "$(${text})"`;
    const index = source.indexOf(definition);
    assert(index >= 0 && index < page, `Missing localized component page text: ${macro}`);
  }
  const show = source.indexOf('!define MUI_PAGE_CUSTOMFUNCTION_SHOW ScoutComponentsShow');
  assert(show >= 0 && show < page, 'Component hint must be readable on initial display');
  const colorCallback = source.search(/^!define MUI_CUSTOMFUNCTION_ONMOUSEOVERSECTION ScoutDescriptionColors\s*$/m);
  assert(colorCallback >= 0 && colorCallback < source.indexOf('!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN'),
    'Define the post-MUI color callback before the hover handler is generated');
  for (const name of ['ScoutComponentsShow', 'ScoutDescriptionColors']) {
    const handler = source.match(new RegExp(`^Function ${name}\\r?\\n([\\s\\S]*?)^FunctionEnd`, 'm'))?.[1];
    assert(handler, `Missing description color handler: ${name}`);
    assert.match(handler, /EnableWindow \$mui\.ComponentsPage\.DescriptionText 1/,
      'Description text must avoid disabled colors inherited from the Windows theme');
    const colors = [...handler.matchAll(/SetCtlColors \$mui\.ComponentsPage\.DescriptionText "([a-f\d]{6})" "([a-f\d]{6})"/gi)];
    assert(colors.length > 0, 'Explicit description text/background colors are required');
    const luminance = hex => {
      const rgb = [0, 2, 4].map(offset => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255)
        .map(channel => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4);
      return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
    };
    for (const [, foreground, background] of colors) {
      const values = [luminance(foreground), luminance(background)].sort((a, b) => a - b);
      assert((values[1] + 0.05) / (values[0] + 0.05) >= 4.5, 'Every description state must have readable contrast');
    }
    if (name === 'ScoutDescriptionColors') {
      const repaint = handler.indexOf("System::Call 'user32::InvalidateRect(p $mui.ComponentsPage.DescriptionText, p0, i1)'");
      assert(repaint > handler.lastIndexOf('SetCtlColors'), 'Apply description colors immediately after changing hover state');
    }
  }
  const descriptions = source.match(/^!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN\r?\n([\s\S]*?)^!insertmacro MUI_FUNCTION_DESCRIPTION_END/m);
  assert(descriptions, 'Component hover descriptions must have an event handler');
  for (const [id, text] of [['SEC_GUI', 'ScoutGuiDescription'], ['SEC_CLI', 'ScoutCliDescription']]) {
    assert(descriptions[1].includes(`!insertmacro MUI_DESCRIPTION_TEXT \${${id}} "$(${text})"`),
      `Missing hover description for ${id}`);
    const declaration = Math.max(source.indexOf(` ${id}\n`), source.indexOf(` ${id}\r\n`));
    assert(declaration >= 0 && declaration < descriptions.index,
      `Description must follow the ${id} section declaration`);
  }
  assert.match(gui, /File "\$\{MAINBINARYSRCPATH\}"/);
  assert.match(gui, /NSISdl::download/);
  assert.match(gui, /WriteRegStr SHCTX "Software\\Classes/);
  assert.match(cli, /File \/oname=giveaway-scout-cli\.exe "\$\{CLIBINARYSRCPATH\}"/);
  assert.doesNotMatch(cli, /WebView|Classes|CreateShortcut/);
  assert.match(preflight, /Call ScoutValidateComponents/);
  assert.match(preflight, /Call ScoutCheckProcesses/);
  assert.match(preflight, /Call ScoutCheckProfile/);
  assert.match(source, /__maintenance check-profile/);
  assert.match(source, /WriteRegDWORD SHCTX "\$\{UNINSTKEY\}" "Components" \$ScoutComponents/);
  assert.match(source, /ReadRegDWORD \$ScoutComponents SHCTX "\$\{UNINSTKEY\}" "Components"/);
  assert.match(source, /"\/COMPONENTS="/);
  assert.match(source, /"\/WAITPID="/);
  assert.match(source, /QueryFullProcessImageNameW/);
  assert.match(source, /WaitForSingleObject\(p \$ScoutWaitHandle, i30000\)/);
  assert.match(source, /__maintenance sync-autostart --components \$ScoutComponents --install-dir/);
  assert.match(source, /--install-dir "\$INSTDIR" --remove/);
  assert.match(source, /Call un\.ScoutMaintainInstallation/);
  assert.doesNotMatch(source, /^\s*!insertmacro CheckIfAppIsRunning/m);
  assert.doesNotMatch(source, /System::Call ['"][^'"\r\n]*(?:RmShutdown|TerminateProcess)/);
  assert.doesNotMatch(source, /^\s*(?:WriteReg(?:Str|ExpandStr)|DeleteRegValue) HKCU "Environment"/m,
    'Only the Rust maintenance helper may manage user PATH');
  for (const name of ['RunMainBinary', 'CreateOrUpdateStartMenuShortcut', 'CreateOrUpdateDesktopShortcut', '.onInstSuccess']) {
    const match = source.match(new RegExp(`^Function ${name.replaceAll('.', '\\.')}\\r?\\n([^\\r\\n]+)`, 'm'));
    assert(match && match[1].includes('$ScoutGuiSelected != 1'), `Missing GUI-only guard: ${name}`);
  }
  return { guiDefault: true, cliDefault: false, preservesComponents: true, killsApplicationProcesses: false,
    wideComponentList: true, componentHoverDescriptions: true, readableDescriptionStates: true };
}

export function verifyComponentInstallerTranslations(hooks) {
  const translations = new Map();
  for (const match of hooks.matchAll(/^LangString (Scout\w+) (1033|1049) "([^"\r\n]+)"\r?$/gm)) {
    translations.set(`${match[1]}:${match[2]}`, match[3]);
  }
  for (const key of ['ScoutGuiComponent', 'ScoutCliComponent', 'ScoutComponentsText',
    'ScoutComponentsList', 'ScoutComponentsDescriptionInfo', 'ScoutGuiDescription', 'ScoutCliDescription']) {
    for (const language of ['1033', '1049']) {
      assert(translations.get(`${key}:${language}`)?.trim(), `Missing ${key} translation for ${language}`);
    }
  }
  const russian = translations.get('ScoutComponentsText:1049');
  assert(russian.includes('«Далее»'), 'Russian component instructions must use guillemets around Далее');
  assert(!russian.includes('$\\"'), 'Russian component instructions must not use straight quotation marks');
  return { localizedComponentInstructions: true };
}

export function main() {
  assert.equal(process.argv.length,2,'No arguments expected; use CARGO_TARGET_DIR if needed');
  const repository=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
  const tauri=path.join(repository,'src/GiveawayScout.Desktop/src-tauri');
  const config=JSON.parse(fs.readFileSync(path.join(tauri,'tauri.conf.json'),'utf8'));
  const directory=path.join(process.env.CARGO_TARGET_DIR ?? path.join(tauri,'target-windows-x64-next'),'release/nsis/x64');
  const source=fs.readFileSync(path.join(directory,'installer.nsi'),'utf8');
  const result=verifyInstallerScript(source,config);
  const components=verifyComponentInstallerScript(source);
  const componentText=verifyComponentInstallerTranslations(fs.readFileSync(path.join(tauri,config.bundle.windows.nsis.installerHooks),'utf8'));
  const hookInclude=source.replaceAll('\\','/').indexOf('/'+config.bundle.windows.nsis.installerHooks+'"');
  assert(hookInclude>=0 && hookInclude<source.indexOf('!insertmacro MUI_PAGE_WELCOME') &&
    hookInclude<source.indexOf('!insertmacro MUI_PAGE_DIRECTORY'),
    'Text overrides must be included before welcome and directory pages');
  assert(fs.readFileSync(path.join(directory,'English.nsh'),'utf8').length>0);
  assert.match(fs.readFileSync(path.join(directory,'Russian.nsh'),'utf8'),/[\u0400-\u04ff]/);
  console.log('PASS: NSIS inputs contain EN/RU, icons, readable GUI/CLI components with descriptions, preserved selection and safe updater handoff.');
  console.log(JSON.stringify({...result,...components,...componentText,nativeInstallerLaunched:false}));
}

if(process.argv[1] && import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href) main();
