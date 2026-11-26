import assert from "node:assert/strict";
import test from "node:test";
import {
  computeCyclePace,
  computeCycleWindow,
  formatCountdown,
  formatCycleRule,
  normalizeCyclePlan,
  periodDeltaPct,
} from "../src/usage/cycles.ts";

test("weekly cycle aligns to configured weekday and HH:mm with zero overlap", () => {
  const plan = normalizeCyclePlan({
    id: "claude-wed",
    name: "Claude Max",
    type: "weekly",
    weekDay: 3, // Wednesday
    time: "14:00",
    quotaMetric: "cost",
    quotaLimit: 200,
  });

  // Thursday 2026-11-26 16:30 -> current cycle started Wed 2026-11-25 14:00
  const now = new Date(2026, 10, 26, 16, 30, 0);
  const cur = computeCycleWindow(plan, now, 0);
  assert.equal(cur.start.getFullYear(), 2026);
  assert.equal(cur.start.getMonth(), 10);
  assert.equal(cur.start.getDate(), 25);
  assert.equal(cur.start.getHours(), 14);
  assert.equal(cur.start.getMinutes(), 0);
  assert.equal(cur.end.getMonth(), 11);
  assert.equal(cur.end.getDate(), 2);
  assert.equal(cur.end.getHours(), 14);

  // Previous cycle ends exactly 1 second before current cycle starts
  const prev = computeCycleWindow(plan, now, -1);
  assert.equal(prev.endDate + 1, cur.startDate);
  assert.equal(prev.start.getDate(), 18);

  // Before reset time on Wednesday (13:59) still belongs to previous week's Wed 14:00
  const beforeReset = new Date(2026, 10, 25, 13, 59, 0);
  const beforeWin = computeCycleWindow(plan, beforeReset, 0);
  assert.equal(beforeWin.start.getDate(), 18);
  assert.equal(beforeWin.end.getDate(), 25);
});

test("rolling window slides backward cleanly by N hours", () => {
  const plan = normalizeCyclePlan({
    id: "r5",
    name: "5h",
    type: "rolling",
    rollingHours: 5,
  });
  const now = new Date(2026, 10, 26, 16, 30, 0);
  const cur = computeCycleWindow(plan, now, 0);
  const prev = computeCycleWindow(plan, now, -1);
  assert.equal(cur.end.getTime() - cur.start.getTime(), 5 * 3600 * 1000);
  assert.equal(prev.endDate + 1, cur.startDate);
  assert.equal(cur.timePct, 1);
});

test("monthly cycle clamps day 31 on short months", () => {
  const plan = normalizeCyclePlan({
    id: "m31",
    name: "月末账单",
    type: "monthly",
    monthDay: 31,
    time: "00:00",
  });
  // March 15, 2026 -> current cycle started Feb 28, 2026 00:00 and ends Mar 31, 2026 00:00
  const now = new Date(2026, 2, 15, 10, 0, 0);
  const cur = computeCycleWindow(plan, now, 0);
  assert.equal(cur.start.getMonth(), 1);
  assert.equal(cur.start.getDate(), 28);
  assert.equal(cur.end.getMonth(), 2);
  assert.equal(cur.end.getDate(), 31);
});

test("custom_days cycle repeats from anchor date", () => {
  const plan = normalizeCyclePlan({
    id: "c14",
    name: "双周",
    type: "custom_days",
    customDays: 14,
    anchorDate: "2026-11-01",
    time: "08:00",
  });
  const now = new Date(2026, 10, 20, 12, 0, 0);
  const cur = computeCycleWindow(plan, now, 0);
  assert.equal(cur.start.getMonth(), 10);
  assert.equal(cur.start.getDate(), 15);
  assert.equal(cur.end.getDate(), 29);
});

test("pace computation detects fast burn, healthy burn, and over quota", () => {
  const plan = normalizeCyclePlan({
    id: "p",
    name: "Plan",
    type: "weekly",
    weekDay: 3,
    time: "14:00",
    quotaMetric: "cost",
    quotaLimit: 200,
  });
  const now = new Date(2026, 10, 27, 6, 32, 0); // ~24% elapsed
  const win = computeCycleWindow(plan, now, 0);

  const makeSummary = (totalCost) => ({
    totalRequests: 100,
    totalInputTokens: 1000,
    totalOutputTokens: 1000,
    totalCacheCreationTokens: 0,
    totalCacheReadTokens: 0,
    realTotalTokens: 2000,
    totalCost: String(totalCost),
    successRate: 100,
    cacheHitRate: 0.5,
  });

  const fast = computeCyclePace(plan, makeSummary(80), win, 0);
  assert.equal(fast.status, "fast");
  assert.ok(fast.usagePct > 39 && fast.usagePct < 41);
  assert.ok((fast.projectedValue ?? 0) > 200);

  const healthy = computeCyclePace(plan, makeSummary(40), win, 0);
  assert.equal(healthy.status, "healthy");

  const over = computeCyclePace(plan, makeSummary(215), win, 0);
  assert.equal(over.status, "over");
});

test("rule formatting, countdown and delta helpers work", () => {
  const plan = normalizeCyclePlan({
    id: "custom-1",
    name: "Claude 周额度",
    type: "weekly",
    weekDay: 4,
    time: "19:00",
    appType: "claude",
    quotaMetric: "cost",
    quotaLimit: 200,
  });
  assert.equal(plan.weekDay, 4);
  assert.equal(plan.time, "19:00");
  assert.equal(plan.appType, "claude");
  assert.equal(plan.quotaMetric, "cost");
  assert.equal(plan.quotaLimit, 200);
  assert.equal(formatCycleRule(plan), "每周四 19:00 重置");
  assert.equal(
    formatCountdown(5 * 86400_000 + 3 * 3600_000 + 12 * 60_000),
    "5天 3小时 12分",
  );
  assert.equal(periodDeltaPct(120, 100), 20);
  assert.equal(periodDeltaPct(50, 0), null);
});
