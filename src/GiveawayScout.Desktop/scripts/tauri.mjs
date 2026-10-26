import { spawnSync } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const projectDirectory = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const platformName =
  process.platform === "win32"
    ? "windows"
    : process.platform === "darwin"
      ? "macos"
      : process.platform;
const architectureName = process.arch === "x64" ? "x64" : process.arch;
const environment = {
  ...process.env,
  CARGO_TARGET_DIR:
    process.env.CARGO_TARGET_DIR ??
    path.join(
      projectDirectory,
      "src-tauri",
      `target-${platformName}-${architectureName}`,
    ),
};
const cli = path.join(
  projectDirectory,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
);
const result = spawnSync(process.execPath, [cli, ...process.argv.slice(2)], {
  cwd: projectDirectory,
  env: environment,
  stdio: "inherit",
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
