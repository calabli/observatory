// Set the app version everywhere it is recorded, in one step:
//   npm run version:bump -- 0.1.13
// package.json, package-lock.json, both Cargo.toml files, the Cargo.lock entries for
// this repository's own crates, and tauri.conf.json. The release workflow requires
// the pushed tag to match package.json, so run this before tagging.
import fs from "node:fs";
import path from "node:path";

const next = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(next ?? "")) {
  console.error("Usage: npm run version:bump -- <major.minor.patch>");
  process.exit(1);
}

const root = process.cwd();
const current = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
).version;
if (current === next) {
  console.error(`Already at ${current}.`);
  process.exit(1);
}
const v = current.replace(/\./g, "\\.");

// Each pattern names the exact entry (by package or crate name) so another
// dependency that happens to share the version number is never touched.
const targets = [
  ["package.json", new RegExp(`^(  "version": ")${v}(")`, "m")],
  ["src-tauri/tauri.conf.json", new RegExp(`^(  "version": ")${v}(")`, "m")],
  [
    "package-lock.json",
    new RegExp(
      `^(  "name": "observatory-desktop",\\n  "version": ")${v}(")`,
      "m",
    ),
  ],
  [
    "package-lock.json",
    new RegExp(
      `(    "": \\{\\n      "name": "observatory-desktop",\\n      "version": ")${v}(")`,
    ),
  ],
  ["native/Cargo.toml", new RegExp(`^(version = ")${v}(")`, "m")],
  ["src-tauri/Cargo.toml", new RegExp(`^(version = ")${v}(")`, "m")],
  [
    "native/Cargo.lock",
    new RegExp(`(name = "observatory-usage-core"\\nversion = ")${v}(")`),
  ],
  [
    "src-tauri/Cargo.lock",
    new RegExp(`(name = "observatory"\\nversion = ")${v}(")`),
  ],
  [
    "src-tauri/Cargo.lock",
    new RegExp(`(name = "observatory-usage-core"\\nversion = ")${v}(")`),
  ],
];

// Apply everything in memory first; write only if every entry was found.
const texts = new Map();
for (const [file, pattern] of targets) {
  const full = path.join(root, file);
  const text = texts.get(full) ?? fs.readFileSync(full, "utf8");
  if (!pattern.test(text)) {
    console.error(
      `${file}: no version ${current} entry matching ${pattern}. Nothing was changed.`,
    );
    process.exit(1);
  }
  texts.set(full, text.replace(pattern, `$1${next}$2`));
}
for (const [full, text] of texts) fs.writeFileSync(full, text);
console.log(`Version ${current} -> ${next} in ${texts.size} files.`);
