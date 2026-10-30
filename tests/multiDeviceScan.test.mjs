import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";
const source = await readFile(
  new URL("../src/usage/multiDeviceScan.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { MultiDeviceScanCoordinator } = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);
const target = (deviceId, extra = {}) => ({
  deviceId,
  deviceName: deviceId,
  local: deviceId === "local",
  enabled: true,
  paired: true,
  ...extra,
});
const transport = () => ({
  scanLocal: async () => ({ errors: [] }),
  startRemote: async (id) => ({ jobId: id }),
  waitRemote: async () => ({ errors: [] }),
  syncRemote: async () => ({ imported: 1 }),
});
test("successful scans synchronize each authorized device and publish stages", async () => {
  const events = [],
    calls = [];
  const t = transport();
  t.syncRemote = async (id) => {
    calls.push(id);
    return { imported: 2 };
  };
  const c = new MultiDeviceScanCoordinator(t, (b) => events.push(b));
  const result = await c.start("batch", [
    target("local"),
    target("one"),
    target("disabled", { enabled: false }),
  ]);
  assert.equal(result.status, "completed");
  assert.deepEqual(calls, ["one"]);
  assert.equal(result.devices[2].state, "not_participating");
  assert.ok(events.some((b) => b.devices[1].state === "syncing"));
});
test("partial scan, unauthorized device and sync failure prevent all-complete", async () => {
  const t = transport();
  t.waitRemote = async (id) => ({
    errors: id === "partial" ? ["source error"] : [],
  });
  t.syncRemote = async (id) => {
    if (id === "failure") throw Error("offline");
    return { imported: 0 };
  };
  const r = await new MultiDeviceScanCoordinator(t, () => {}).start("b", [
    target("local"),
    target("partial"),
    target("failure"),
    target("old", { paired: false }),
  ]);
  assert.equal(r.status, "incomplete");
  assert.deepEqual(
    r.devices.map((d) => d.state),
    ["completed", "partial_failure", "failed", "not_participating"],
  );
});
test("prevents overlapping batches and obeys concurrency", async () => {
  let release;
  const gate = new Promise((r) => {
    release = r;
  });
  let inFlight = 0,
    peak = 0;
  const t = transport();
  t.startRemote = async (id) => {
    inFlight++;
    peak = Math.max(peak, inFlight);
    await gate;
    inFlight--;
    return { jobId: id };
  };
  const c = new MultiDeviceScanCoordinator(t, () => {}, 2);
  const pending = c.start("b", [
    target("local"),
    target("a"),
    target("b"),
    target("c"),
  ]);
  await assert.rejects(c.start("other", [target("local")]), /正在进行/);
  release();
  await pending;
  assert.ok(peak <= 2);
});
