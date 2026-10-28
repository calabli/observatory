import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/usage/chartData.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { trendTokens, isHourlyTrend, trendLabel, trendHitRate } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

test("cache hit rate uses fresh input plus both cache counters as its denominator", () => {
  assert.equal(
    trendHitRate({
      totalInputTokens: 40,
      totalCacheReadTokens: 60,
      totalCacheCreationTokens: 5,
    }),
    60 / 105,
  );
});
test("cache hit rate is unavailable rather than a false zero without input", () => {
  assert.equal(
    trendHitRate({
      totalInputTokens: 0,
      totalCacheReadTokens: 0,
      totalCacheCreationTokens: 0,
    }),
    null,
  );
});

test("trend includes fresh input, output and both cache counters", () => {
  assert.equal(
    trendTokens({
      totalInputTokens: 40,
      totalOutputTokens: 10,
      totalCacheReadTokens: 60,
      totalCacheCreationTokens: 5,
      totalTokens: 50,
    }),
    115,
  );
});
test("daily RFC3339 buckets show dates rather than midnight", () => {
  const data = [
    { date: "2026-10-26T00:00:00+08:00" },
    { date: "2026-10-27T00:00:00+08:00" },
  ];
  assert.equal(isHourlyTrend(data), false);
  assert.deepEqual(
    data.map((d) => trendLabel(d.date, isHourlyTrend(data))),
    ["10-26", "10-27"],
  );
});
test("hourly RFC3339 buckets retain native local hours", () => {
  const data = [
    { date: "2026-10-27T08:00:00+08:00" },
    { date: "2026-10-27T09:00:00+08:00" },
  ];
  assert.equal(isHourlyTrend(data), true);
  assert.equal(trendLabel(data[1].date, true), "09:00");
});
test("empty and legacy bucket labels remain valid", () => {
  assert.equal(isHourlyTrend([]), false);
  assert.equal(trendLabel("2026-10-27", false), "10-27");
  assert.equal(trendLabel("08:00", true), "08:00");
});
