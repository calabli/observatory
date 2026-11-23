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

// Clear any bundle left by an earlier build first, so only this build's output can
// be installed.
fs.rmSync(builtApp, { recursive: true, force: true });
fs.rmSync(builtTar, { force: true });
// A local install never self-updates, so skip the signed update package: it needs
// the release signing key, which is absent locally and made the build exit with an
// error after the app was already built.
const build = spawnSync(
  "npx",
  [
    "tauri",
    "build",
    "--bundles",
    "app",
    "--config",
    JSON.stringify({ bundle: { createUpdaterArtifacts: false } }),
  ],
  { cwd: root, stdio: "inherit" },
);
if (build.status !== 0 || !fs.existsSync(builtApp)) {
  console.error("tauri build failed; nothing installed.");
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
