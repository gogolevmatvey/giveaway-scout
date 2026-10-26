import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

export const desktop = fileURLToPath(new URL('../', import.meta.url));
export const tauri = path.join(desktop, 'src-tauri');
export const repository = path.resolve(desktop, '../..');
export const artifact = path.join(repository, '.tools/ui-tests/app/giveaway-scout.exe');

function newest(directory, accept = () => true) {
  return fs.readdirSync(directory).filter(accept)
    .sort((a,b) => b.localeCompare(a, 'en', {numeric:true}))[0];
}

export function windowsToolchain() {
  if (process.platform !== 'win32' || process.arch !== 'x64') throw new Error('Run native UI tests with Windows x64 Node.js, not WSL Node.js.');
  const version = fs.readFileSync(path.join(desktop, 'rust-toolchain.toml'), 'utf8').match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error('Rust pin was not found');
  const rust = path.join(process.env.USERPROFILE, '.rustup/toolchains', `${version}-x86_64-pc-windows-msvc/bin`);
  const vsRoot = path.join(process.env['ProgramFiles(x86)'], 'Microsoft Visual Studio');
  const vsVersion = newest(vsRoot, name => fs.existsSync(path.join(vsRoot, name, 'BuildTools/VC/Tools/MSVC')));
  if (!vsVersion) throw new Error('Visual Studio C++ Build Tools are required');
  const msvcRoot = path.join(vsRoot, vsVersion, 'BuildTools/VC/Tools/MSVC');
  const msvc = path.join(msvcRoot, newest(msvcRoot));
  const sdk = path.join(process.env['ProgramFiles(x86)'], 'Windows Kits/10');
  const sdkVersion = newest(path.join(sdk,'Lib'), name => /^\d+\.\d+\.\d+\.\d+$/.test(name));
  const environment = {...process.env};
  for (const key of Object.keys(environment)) if (key.toLowerCase() === 'path') delete environment[key];
  environment.PATH = [rust, path.join(msvc,'bin/Hostx64/x64'), path.join(sdk,'bin',sdkVersion,'x64'), path.dirname(process.execPath), path.join(process.env.APPDATA,'npm'), process.env.PATH ?? process.env.Path].join(';');
  environment.RUSTC = path.join(rust,'rustc.exe');
  environment.LIB = [path.join(msvc,'lib/x64'), path.join(sdk,'Lib',sdkVersion,'um/x64'), path.join(sdk,'Lib',sdkVersion,'ucrt/x64')].join(';');
  environment.INCLUDE = [path.join(msvc,'include'), ...['ucrt','shared','um','winrt','cppwinrt'].map(name => path.join(sdk,'Include',sdkVersion,name))].join(';');
  environment.VCToolsInstallDir = msvc + path.sep;
  environment.WindowsSdkDir = sdk + path.sep;
  environment.WindowsSDKVersion = sdkVersion + path.sep;
  // Reuse the existing compiler cache, but never replace a Release EXE or dist.
  environment.CARGO_TARGET_DIR = path.join(tauri,'target-windows-x64-next');
  return {environment, cargo:path.join(rust,'cargo.exe')};
}

export function run(executable, args, cwd, env) {
  const result = spawnSync(executable, args, {cwd, env, stdio:'inherit', windowsHide:true});
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Command failed (${result.status}): ${path.basename(executable)} ${args.join(' ')}`);
}
