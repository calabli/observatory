import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(
  new URL("../src/markets/marketsShared.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { tileWeight, tileWeights } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

const item = (symbol) => ({ symbol, name: symbol, market: "US" });
const quote = (symbol, extra) => ({
  symbol,
  currency: "USD",
  price: 100,
  volume: 1e6,
  ...extra,
});

test("a missing market cap never borrows the traded-value scale", () => {
  // AMD from Yahoo: no market cap, but a large traded value.
  const amd = quote("AMD", { marketCap: null, turnover: 1.2e11 });
  assert.equal(tileWeight(item("AMD"), amd, "cap"), null);
  // With a cap it is on the market-cap scale (tens to ~150).
  const nvda = quote("NVDA", { marketCap: 3.99e12 });
  const w = tileWeight(item("NVDA"), nvda, "cap");
  assert.ok(w > 100 && w < 200, String(w));
});

test("missing weights get the median of the others", () => {
  const items = ["NVDA", "AVGO", "MSFT", "AMD"].map(item);
  const quotes = new Map([
    ["NVDA", quote("NVDA", { marketCap: 3.99e12 })],
    ["AVGO", quote("AVGO", { marketCap: 1.5e12 })],
    ["MSFT", quote("MSFT", { marketCap: 3.5e12 })],
    ["AMD", quote("AMD", { marketCap: null, turnover: 1.2e11 })],
  ]);
  const w = tileWeights(items, quotes, "cap");
  assert.ok(w.get("NVDA") > w.get("AMD"), "NVDA outweighs the cap-less AMD");
  assert.equal(w.get("AMD"), w.get("MSFT"), "AMD takes the median weight");
  // Equal mode is unaffected.
  assert.deepEqual(
    [...tileWeights(items, quotes, "equal").values()],
    [1, 1, 1, 1],
  );
});
