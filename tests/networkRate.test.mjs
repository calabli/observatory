import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/network/networkRate.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { calculateRate } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);
const first = { id: "eth", received: 1000, transmitted: 2000, at: 1000 };
test("rates use elapsed time and counters from the same interface", () => {
  assert.deepEqual(
    calculateRate(first, {
      ...first,
      received: 4000,
      transmitted: 8000,
      at: 4000,
    }),
    { down: 1000, up: 2000 },
  );
  assert.equal(calculateRate(first, { ...first, id: "vpn", at: 4000 }), null);
});
test("counter reset, wrap, stale and identical timestamps do not produce bogus rates", () => {
  assert.equal(calculateRate(first, { ...first, received: 1, at: 4000 }), null);
  assert.equal(calculateRate(first, { ...first, at: 20000 }), null);
  assert.equal(calculateRate(first, first), null);
});
