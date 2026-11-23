import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

if (process.platform !== "darwin") {
  console.error("mac:install is only supported on macOS.");
  process.exit(1);
}

const root = process.cwd();
const bundleDir = path.join(root, "src-tauri/target/release/bundle/macos");
const builtApp = path.join(bundleDir, "Observatory.app");
const builtTar = path.join(bundleDir, "Observatory.app.tar.gz");
const targetApp = "/Applications/Observatory.app";

// Clear any bundle left by an earlier build first: tauri build exits non-zero at the
// end when updater signing keys are absent locally, so the bundle's presence is the
// success signal, and a stale one would hide a failed build.
fs.rmSync(builtApp, { recursive: true, force: true });
fs.rmSync(builtTar, { force: true });
const build = spawnSync("npx", ["tauri", "build", "--bundles", "app"], {
  cwd: root,
  stdio: "inherit",
});
if (!fs.existsSync(builtApp)) {
  console.error(
    "tauri build did not produce Observatory.app; nothing installed.",
  );
  process.exit(build.status || 1);
}

const install = spawnSync("ditto", [builtApp, targetApp], { stdio: "inherit" });
if (install.status !== 0) {
  process.exit(install.status ?? 1);
}

// Remove the duplicate build bundle so macOS Launchpad / Spotlight only index /Applications.
fs.rmSync(builtApp, { recursive: true, force: true });
fs.rmSync(builtTar, { force: true });
console.log(`Installed ${targetApp} and cleaned up temporary build bundle.`);
