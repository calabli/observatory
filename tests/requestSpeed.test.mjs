import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(new URL("../src/usage/requestSpeed.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { formatRequestSpeed } = await import("data:text/javascript;base64," + Buffer.from(code).toString("base64"));

test("proxy speed excludes first token wait and requires adequate output and generation window", () => {
  assert.equal(formatRequestSpeed({ dataSource: "proxy", outputTokens: 200, latencyMs: 2500, firstTokenMs: 500 }), "100.0 Token/s");
  assert.equal(formatRequestSpeed({ dataSource: "proxy", outputTokens: 99, latencyMs: 2500, firstTokenMs: 500 }), "—");
  assert.equal(formatRequestSpeed({ dataSource: "proxy", outputTokens: 200, latencyMs: 550, firstTokenMs: 500 }), "—");
  assert.equal(formatRequestSpeed({ dataSource: "proxy", outputTokens: 200, latencyMs: 2500 }), "—");
});
test("session estimates include the full elapsed time and show approximation", () => {
  assert.equal(formatRequestSpeed({ dataSource: "session", outputTokens: 200, latencyMs: 2000, firstTokenMs: 500 }), "≈ 100.0 Token/s");
  assert.equal(formatRequestSpeed({ dataSource: "session", outputTokens: 199, latencyMs: 2000 }), "—");
  assert.equal(formatRequestSpeed({ dataSource: "session", outputTokens: 200, latencyMs: 999 }), "—");
  assert.equal(formatRequestSpeed({ dataSource: "session", outputTokens: 200, latencyMs: NaN }), "—");
});
