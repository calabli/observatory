import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/usage/modelDisplay.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { modelDisplayName } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);
test("friendly Claude names retain raw IDs outside display formatting", () => {
  assert.equal(modelDisplayName("claude-opus-5-5"), "Claude Opus 5.5");
  assert.equal(modelDisplayName("claude-sonnet-4-6"), "Claude Sonnet 4.6");
  assert.equal(
    modelDisplayName("claude-opus-5-5-20261001"),
    "claude-opus-5-5-20261001",
  );
  assert.equal(modelDisplayName("gpt-6.1-sol"), "gpt-6.1-sol");
  assert.equal(modelDisplayName("unknown"), "unknown");
});
