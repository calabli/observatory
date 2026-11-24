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
const {
  tileWeight,
  tileWeights,
  applyPoolEdit,
  addCompanyToPoolEdit,
  removeCompanyFromPoolEdit,
  formatPoolLabel,
} = await import(
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

test("pool edits add, remove, reassign sector, and restore built-in companies cleanly", () => {
  const base = [
    {
      symbol: "NVDA",
      name: "英伟达",
      market: "US",
      sector: "算力芯片",
      capHint: 3500,
    },
    { symbol: "AMD", name: "AMD", market: "US", sector: "算力芯片" },
    { symbol: "TSM", name: "台积电", market: "US", sector: "代工与设备" },
  ];

  // Add a brand-new company
  const edit1 = addCompanyToPoolEdit(base, undefined, {
    symbol: "CRWV",
    name: "CoreWeave",
    market: "US",
    sector: "大模型与云",
  });
  const afterAdd = applyPoolEdit(base, edit1);
  assert.equal(afterAdd.length, 4);
  assert.equal(afterAdd[0].symbol, "CRWV");

  // Remove a built-in company
  const edit2 = removeCompanyFromPoolEdit(base, edit1, "AMD");
  const afterRemove = applyPoolEdit(base, edit2);
  assert.deepEqual(
    afterRemove.map((i) => i.symbol),
    ["CRWV", "NVDA", "TSM"],
  );

  // Move a built-in company to a new sector while preserving capHint
  const edit3 = addCompanyToPoolEdit(base, edit2, {
    symbol: "NVDA",
    name: "英伟达",
    market: "US",
    sector: "AI 核心",
  });
  const afterMove = applyPoolEdit(base, edit3);
  const nvda = afterMove.find((i) => i.symbol === "NVDA");
  assert.equal(nvda.sector, "AI 核心");
  assert.equal(nvda.capHint, 3500);

  // Restoring a removed built-in item and removing the custom addition collapses edit back to undefined
  const restored = addCompanyToPoolEdit(base, edit2, base[1]);
  const clean = removeCompanyFromPoolEdit(base, restored, "CRWV");
  assert.equal(clean, undefined);

  assert.equal(formatPoolLabel("中概股 (53)", 53, false), "中概股 (53)");
  assert.equal(formatPoolLabel("中概股 (53)", 55, true), "中概股 (55)");
  assert.equal(
    formatPoolLabel("台股 Top 150", 148, true),
    "台股 Top 150 (148)",
  );
});
