import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = process.cwd();
const home = os.homedir();
const debugMode = process.argv.includes("--debug");

function findAndroidHome() {
  const candidates = [
    process.env.ANDROID_HOME,
    process.env.ANDROID_SDK_ROOT,
    path.join(home, "Library/Android/sdk"),
    path.join(home, "Android/Sdk"),
  ].filter(Boolean);
  for (const dir of candidates) {
    if (fs.existsSync(dir)) return dir;
  }
  return null;
}

function findLatestSubdir(parent) {
  if (!fs.existsSync(parent)) return null;
  const entries = fs
    .readdirSync(parent, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => d.name)
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  return entries.length ? path.join(parent, entries[entries.length - 1]) : null;
}

const androidHome = findAndroidHome();
if (!androidHome) {
  console.error(
    "ANDROID_HOME not found. Install Android SDK or set ANDROID_HOME.",
  );
  process.exit(1);
}

const ndkHome =
  process.env.NDK_HOME || findLatestSubdir(path.join(androidHome, "ndk"));
if (!ndkHome) {
  console.error(
    `Android NDK not found under ${path.join(androidHome, "ndk")}.`,
  );
  process.exit(1);
}

const env = {
  ...process.env,
  ANDROID_HOME: androidHome,
  ANDROID_SDK_ROOT: androidHome,
  NDK_HOME: ndkHome,
};

const args = ["tauri", "android", "build", "--apk", "--target", "aarch64"];
if (debugMode) args.push("--debug");

console.log(
  `Building Observatory Android (${debugMode ? "debug" : "release"}, aarch64)...`,
);
console.log(`  ANDROID_HOME=${androidHome}`);
console.log(`  NDK_HOME=${ndkHome}`);

const res = spawnSync("npx", args, { cwd: root, env, stdio: "inherit" });
if (res.status !== 0) {
  process.exit(res.status ?? 1);
}

const pkg = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
);
const variant = debugMode ? "debug" : "release";
const apkCandidates = [
  path.join(
    root,
    "src-tauri/gen/android/app/build/outputs/apk/universal",
    variant,
  ),
  path.join(root, "src-tauri/gen/android/app/build/outputs/apk/arm64", variant),
];
const apkDir = apkCandidates.find(
  (dir) =>
    fs.existsSync(dir) && fs.readdirSync(dir).some((f) => f.endsWith(".apk")),
);
if (!apkDir) {
  console.error(`No APK found in: ${apkCandidates.join(", ")}`);
  process.exit(1);
}
const apks = fs.readdirSync(apkDir).filter((f) => f.endsWith(".apk"));

const builtApk = path.join(apkDir, apks[0]);
const outName = `Observatory-${pkg.version}-Android-arm64${debugMode ? "-debug" : ""}.apk`;
const outPath = path.join(root, outName);

// If building release without a custom keystore, sign with the local Android debug
// keystore (when available) so the APK can be sideloaded directly onto devices.
const buildToolsDir = findLatestSubdir(path.join(androidHome, "build-tools"));
const apksigner = buildToolsDir ? path.join(buildToolsDir, "apksigner") : null;
const debugKeystore = path.join(home, ".android/debug.keystore");

if (
  !debugMode &&
  builtApk.includes("unsigned") &&
  apksigner &&
  fs.existsSync(apksigner)
) {
  if (!fs.existsSync(debugKeystore)) {
    fs.mkdirSync(path.dirname(debugKeystore), { recursive: true });
    spawnSync(
      "keytool",
      [
        "-genkey",
        "-v",
        "-keystore",
        debugKeystore,
        "-storepass",
        "android",
        "-alias",
        "androiddebugkey",
        "-keypass",
        "android",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "10000",
        "-dname",
        "CN=Observatory Debug,O=Observatory,C=US",
      ],
      { stdio: "inherit" },
    );
  }
  if (fs.existsSync(debugKeystore)) {
    const sign = spawnSync(
      apksigner,
      [
        "sign",
        "--ks",
        debugKeystore,
        "--ks-pass",
        "pass:android",
        "--ks-key-alias",
        "androiddebugkey",
        "--key-pass",
        "pass:android",
        "--out",
        outPath,
        builtApk,
      ],
      { stdio: "inherit" },
    );
    if (sign.status === 0) {
      console.log(`Signed Android APK ready: ${outPath}`);
      process.exit(0);
    }
  }
}

fs.copyFileSync(builtApk, outPath);
console.log(`Android APK ready: ${outPath}`);
