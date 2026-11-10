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
const { customTimeRange } = await import(
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
