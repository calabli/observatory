import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/usage/modelSpeed.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { modelSpeeds } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);
const base = { requestCount: 1, totalTokens: 0, totalCost: "0" };

test("exact speed wins over the estimate", () => {
  const [row] = modelSpeeds([
    {
      ...base,
      model: "a",
      speedOutputTokens: 1000,
      speedGenerationMs: 10000,
      estSpeedOutputTokens: 1000,
      estSpeedDurationMs: 20000,
    },
  ]);
  assert.equal(row.speed, 100);
  assert.equal(row.estimated, false);
});

test("an estimate alone is marked as estimated", () => {
  const [row] = modelSpeeds([
    {
      ...base,
      model: "b",
      estSpeedOutputTokens: 600,
      estSpeedDurationMs: 10000,
      estFirstTokenMedianMs: 1500,
    },
  ]);
  assert.equal(row.speed, 60);
  assert.equal(row.estimated, true);
  assert.equal(row.firstTokenMs, 1500);
  assert.equal(row.firstTokenEstimated, true);
});

test("models without speed data are dropped and the rest sort fastest first", () => {
  const rows = modelSpeeds([
    { ...base, model: "slow", speedOutputTokens: 100, speedGenerationMs: 5000 },
    { ...base, model: "none" },
    { ...base, model: "zero", speedOutputTokens: 100, speedGenerationMs: 0 },
    { ...base, model: "fast", speedOutputTokens: 900, speedGenerationMs: 3000 },
  ]);
  assert.deepEqual(
    rows.map((r) => r.model),
    ["fast", "slow"],
  );
});

test("the list is capped", () => {
  const many = Array.from({ length: 12 }, (_, i) => ({
    ...base,
    model: "m" + i,
    speedOutputTokens: 100 + i,
    speedGenerationMs: 1000,
  }));
  assert.equal(modelSpeeds(many, 5).length, 5);
});
