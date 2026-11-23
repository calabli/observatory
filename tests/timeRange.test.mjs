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
const { customTimeRange, presetDayRange } = await import(
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

test("day presets cover whole local days ending at the last second", () => {
  const today = new Date(2026, 10, 24);
  const at = (d, h = 0, m = 0, s = 0) =>
    new Date(2026, 10, d, h, m, s).getTime() / 1000;
  assert.deepEqual(presetDayRange("today", today), {
    startDate: at(24),
    endDate: at(24, 23, 59, 59),
  });
  assert.deepEqual(presetDayRange("yesterday", today), {
    startDate: at(23),
    endDate: at(23, 23, 59, 59),
  });
  assert.deepEqual(presetDayRange("days3", today), {
    startDate: at(22),
    endDate: at(24, 23, 59, 59),
  });
  assert.equal(presetDayRange("week", today).startDate, at(18));
  assert.equal(
    presetDayRange("month", today).startDate,
    new Date(2026, 9, 26).getTime() / 1000,
  );
});

test("yesterday crosses a month boundary", () => {
  const range = presetDayRange("yesterday", new Date(2026, 11, 1));
  assert.equal(range.startDate, new Date(2026, 10, 30).getTime() / 1000);
});
