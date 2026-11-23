// Print the CHANGELOG.md section for one version, for the release body:
//   node scripts/release-notes.mjs 0.1.14
// Exits non-zero when the section is missing or empty, so a release cannot go
// out without notes.
import fs from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("Usage: node scripts/release-notes.mjs <major.minor.patch>");
  process.exit(1);
}
const lines = fs.readFileSync("CHANGELOG.md", "utf8").split("\n");
const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
if (start === -1) {
  console.error(
    `CHANGELOG.md has no "## [${version}]" section. Rename "## Unreleased" to it before tagging.`,
  );
  process.exit(1);
}
const rest = lines.slice(start + 1);
const end = rest.findIndex((l) => l.startsWith("## "));
const body = (end === -1 ? rest : rest.slice(0, end)).join("\n").trim();
if (!body) {
  console.error(`The ${version} section in CHANGELOG.md is empty.`);
  process.exit(1);
}
console.log(`${body}\n\nThe macOS build is not notarized.`);
