import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/usage/timeRange.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { customTimeRange, presetRange, parseRangePreset, USAGE_RANGES } =
  await import(
    "data:text/javascript;base64," + Buffer.from(code).toString("base64")
  );
test("local range retains second precision and includes equal endpoints", () => {
  const start = "2026-10-28T10:11:12",
    end = "2026-10-28T10:11:19";
  assert.deepEqual(customTimeRange(start, end), {
    startDate: new Date(start).getTime() / 1000,
    endDate: new Date(end).getTime() / 1000,
  });
  assert.equal(
    customTimeRange(start, start).endDate,
    new Date(start).getTime() / 1000,
  );
  assert.equal(customTimeRange(end, start), null);
  assert.equal(customTimeRange("", end), null);
});

const today = new Date(2026, 10, 24);
const at = (d, h = 0, m = 0, sec = 0) =>
  new Date(2026, 10, d, h, m, sec).getTime() / 1000;

test("day presets cover whole local days ending at the last second", () => {
  assert.deepEqual(presetRange("today", today), {
    startDate: at(24),
    endDate: at(24, 23, 59, 59),
  });
  assert.deepEqual(presetRange("yesterday", today), {
    startDate: at(23),
    endDate: at(23, 23, 59, 59),
  });
  assert.deepEqual(presetRange("3d", today), {
    startDate: at(22),
    endDate: at(24, 23, 59, 59),
  });
  assert.equal(presetRange("7d", today).startDate, at(18));
  assert.equal(presetRange("14d", today).startDate, at(11));
  assert.equal(
    presetRange("30d", today).startDate,
    new Date(2026, 9, 26).getTime() / 1000,
  );
  assert.equal(presetRange("month", today).startDate, at(1));
});

test("open and now ends apply only to ranges that include today", () => {
  assert.deepEqual(presetRange("7d", today, "open"), { startDate: at(18) });
  const now = new Date(2026, 10, 24, 9, 30).getTime();
  assert.deepEqual(presetRange("today", today, "now", now), {
    startDate: at(24),
    endDate: at(24, 9, 30),
  });
  // A past day stays closed whatever end is asked for.
  assert.deepEqual(presetRange("yesterday", today, "open"), {
    startDate: at(23),
    endDate: at(23, 23, 59, 59),
  });
});

test("yesterday crosses a month boundary", () => {
  const range = presetRange("yesterday", new Date(2026, 11, 1));
  assert.equal(range.startDate, new Date(2026, 10, 30).getTime() / 1000);
});

test("saved usage keys from older versions still load", () => {
  assert.equal(parseRangePreset("week", USAGE_RANGES, true), "7d");
  assert.equal(parseRangePreset("month", USAGE_RANGES, true), "30d");
  assert.equal(parseRangePreset("days3", USAGE_RANGES, true), "3d");
  assert.equal(parseRangePreset("yesterday", USAGE_RANGES, true), "yesterday");
  assert.equal(parseRangePreset("month", USAGE_RANGES), null);
  assert.equal(parseRangePreset(42, USAGE_RANGES), null);
});
