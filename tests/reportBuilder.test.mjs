import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const toUrl = (code) =>
  "data:text/javascript;base64," + Buffer.from(code).toString("base64");
async function transpile(path) {
  const source = await readFile(new URL(path, import.meta.url), "utf8");
  return ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
}
// The report imports the subscription formatter; point that import at its own data URL.
const formatUrl = toUrl(await transpile("../src/subscriptions/format.ts"));
const report = (await transpile("../src/insights/reportBuilder.ts")).replace(
  /from\s+["']\.\.\/subscriptions\/format["']/,
  `from "${formatUrl}"`,
);
const { buildHtmlReport, buildMarkdownReport } = await import(toUrl(report));

const data = {
  cacheSavings: {
    actualCostUsd: 12.5,
    cacheReadCostUsd: 1.25,
    savedUsd: 30,
    cacheHitRate: 0.9,
    totalRequests: 120,
    avgContextTokens: 40000,
  },
  forecast: {
    currentCostUsd: 12.5,
    todayCostUsd: 2,
    previousCostUsd: 10,
    deltaPct: 25,
    elapsedDays: 3,
    dailyAvgUsd: 4.17,
    projected30dUsd: 125,
    dailyCosts: [
      { date: "2026-11-22", cost: 4 },
      { date: "2026-11-23", cost: 6.5 },
      { date: "2026-11-24", cost: 2 },
    ],
  },
  rhythm: [
    {
      weekday: 0,
      hour: 10,
      requests: 50,
      costUsd: 5,
      speedOutputTokens: 0,
      speedDurationMs: 0,
    },
  ],
  sessions: [
    {
      sessionId: "s-1",
      appType: "claude",
      title: '<script>alert("x")</script> | pipe',
      projectDir: "/work/app",
      turns: 12,
      costUsd: 3.5,
      avgContextTokens: 30000,
      totalTokens: 400000,
      outputTokens: 9000,
      lastAt: 1_795_000_000,
    },
  ],
  projects: [
    {
      projectDir: "/work/<b>app</b>",
      projectName: "app | main",
      costUsd: 3.5,
      requests: 12,
      sessions: 1,
      totalTokens: 400000,
      apps: ["claude"],
    },
  ],
  stability: [
    {
      model: "claude-x",
      requests: 100,
      ttftP50Ms: 800,
      ttftP95Ms: 2000,
      ttftEstimated: false,
      speedP10: 40,
      speedP50: 70,
      speedEstimated: false,
    },
  ],
  tokenMix: [
    {
      date: "2026-11-24",
      freshInput: 1000,
      output: 500,
      cacheCreation: 200,
      cacheRead: 9000,
    },
  ],
  status: {
    total: 120,
    ok2xx: 118,
    rateLimit429: 2,
    server5xx: 0,
    otherError: 0,
    peak429Hour: 14,
  },
  subscriptions: [
    {
      id: "sub",
      name: "Max | plan",
      appType: "claude",
      price: 100,
      currency: "USD",
      billing: "月",
      rangeApiCostUsd: 12.5,
      cycleApiCostUsd: 150,
      cycleRequests: 300,
      usdRoiMultiple: 1.5,
    },
  ],
};
const opts = {
  data,
  rangeLabel: "近 3 天",
  scopeLabel: "本机",
  sourceName: new Map([["claude", "Claude Code"]]),
  budget: 200,
  dailyBudget: 10,
};

test("HTML report escapes user text and keeps its sections", () => {
  const html = buildHtmlReport(opts);
  assert.ok(
    html.startsWith("<!DOCTYPE html>") || html.startsWith("<!doctype html>"),
  );
  assert.ok(html.includes("近 3 天"));
  assert.ok(!html.includes("<script>alert"), "session title must be escaped");
  assert.ok(html.includes("&lt;script&gt;alert"));
  assert.ok(!html.includes("<b>app</b>"), "project path must be escaped");
  assert.ok(html.includes("$12.50"));
});

test("Markdown report keeps table rows intact", () => {
  const md = buildMarkdownReport(opts);
  assert.ok(md.startsWith("# Observatory AI 编程复盘报告（近 3 天）"));
  // A pipe in a name would split the table cell; it is replaced.
  const projectRow = md.split("\n").find((l) => l.includes("app / main"));
  assert.ok(projectRow, "project name with | is rewritten");
  const subRow = md.split("\n").find((l) => l.includes("Max / plan"));
  assert.ok(subRow, "subscription name with | is rewritten");
  // Every row of a table has as many cells as its header.
  let header = null;
  for (const line of md.split("\n")) {
    if (!line.startsWith("|")) {
      header = null;
      continue;
    }
    const cells = line.split("|").length;
    if (header === null) header = cells;
    else assert.equal(cells, header, line);
  }
});
