// Run the pinned native toolchain with the same MSVC/SDK environment as UI tests.
import { windowsToolchain, tauri } from '../src/GiveawayScout.Desktop/scripts/windows-ui-toolchain.mjs';
import { spawnSync } from 'node:child_process';

const { cargo, environment } = windowsToolchain();
if (process.env.CARGO_TARGET_DIR) environment.CARGO_TARGET_DIR = process.env.CARGO_TARGET_DIR;
const result = spawnSync(cargo, process.argv.slice(2), {
  cwd: tauri, env: environment, stdio: 'inherit', windowsHide: true,
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
