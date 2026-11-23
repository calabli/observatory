import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(
  new URL("../src/markets/treemap.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { layoutSquarified, heatTileColor } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

const nodes = (weights) =>
  weights.map((weight, i) => ({ id: "n" + i, weight, data: i }));
const area = (r) => r.w * r.h;

test("tiles fill the box without overlap, areas proportional to weight", () => {
  const weights = [60, 30, 25, 12, 8, 5, 3, 1];
  const rects = layoutSquarified(nodes(weights), 10, 20, 400, 300);
  assert.equal(rects.length, weights.length);
  const total = weights.reduce((a, b) => a + b, 0);
  let covered = 0;
  for (const r of rects) {
    const expected = (weights[r.data] / total) * 400 * 300;
    assert.ok(Math.abs(area(r) - expected) < 1e-6 * 400 * 300, r.id);
    assert.ok(r.x >= 10 - 1e-9 && r.y >= 20 - 1e-9);
    assert.ok(r.x + r.w <= 410 + 1e-9 && r.y + r.h <= 320 + 1e-9);
    covered += area(r);
  }
  assert.ok(Math.abs(covered - 400 * 300) < 1e-6);
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) {
      const a = rects[i];
      const b = rects[j];
      const overlapW = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
      const overlapH = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
      assert.ok(
        overlapW <= 1e-9 || overlapH <= 1e-9,
        `${a.id} overlaps ${b.id}`,
      );
    }
  }
});

test("squarified tiles stay reasonably square", () => {
  const rects = layoutSquarified(nodes(Array(16).fill(1)), 0, 0, 400, 400);
  for (const r of rects) {
    assert.ok(Math.max(r.w / r.h, r.h / r.w) < 2.5, `${r.id} ${r.w}x${r.h}`);
  }
});

test("empty input and an empty box give no tiles", () => {
  assert.deepEqual(layoutSquarified([], 0, 0, 100, 100), []);
  const rects = layoutSquarified(nodes([1, 2]), 0, 0, 0, 100);
  assert.ok(rects.every((r) => area(r) === 0));
});

test("heat colors follow each market's convention", () => {
  // CN: red up; US: green up. The same move gets opposite hues.
  assert.equal(heatTileColor(3, "cn"), heatTileColor(-3, "us"));
  assert.equal(heatTileColor(-3, "cn"), heatTileColor(3, "us"));
  assert.notEqual(heatTileColor(3, "cn"), heatTileColor(3, "us"));
  // Bigger moves are darker buckets; flat and missing have their own colors.
  assert.notEqual(heatTileColor(0.5, "cn"), heatTileColor(5, "cn"));
  assert.equal(heatTileColor(0.01, "cn"), heatTileColor(-0.01, "us"));
  assert.equal(heatTileColor(null, "cn"), heatTileColor(Number.NaN, "us"));
});
