import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/types/usage.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { isNotBillableUsage, isUnpricedUsage } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

const usage = (overrides = {}) => ({
  inputTokens: 100,
  outputTokens: 20,
  cacheReadTokens: 300,
  cacheCreationTokens: 0,
  totalCostUsd: "0",
  statusCode: 200,
  costMultiplier: "1",
  ...overrides,
});

test("a model without a public price is not reported as unpriced", () => {
  const log = usage({ notBillable: true });
  assert.equal(isNotBillableUsage(log), true);
  assert.equal(isUnpricedUsage(log), false);
});

test("a missing price is still reported as unpriced", () => {
  const log = usage();
  assert.equal(isNotBillableUsage(log), false);
  assert.equal(isUnpricedUsage(log), true);
});

test("a priced request is neither unpriced nor not billable", () => {
  const log = usage({ totalCostUsd: "0.0123" });
  assert.equal(isNotBillableUsage(log), false);
  assert.equal(isUnpricedUsage(log), false);
});

test("a failed request without tokens is not unpriced", () => {
  assert.equal(
    isUnpricedUsage(
      usage({ statusCode: 500, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0 }),
    ),
    false,
  );
});
