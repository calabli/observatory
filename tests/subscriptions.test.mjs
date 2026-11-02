import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';

/**
 * 把 TypeScript 源码转译后以 data URL 导入。
 * 这个仓库没有前端测试运行器，node:test + 转译是最轻的做法。
 */
async function loadModule(relativePath) {
  const source = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  const code = ts.transpileModule(source, {
    compilerOptions: {module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022},
  }).outputText;
  return import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'));
}

const {
  weeklyCycle,
  currentBillingPeriod,
  billingQuotaWindows,
  activeSubscription,
  localDate,
  coveringLocalDates,
  cycleOffsetRange,
  clampCycleToWindow,
  cycleOverlapsWindow,
} = await loadModule('../src/subscriptions/cycles.ts');

test('full-day fallback covers the entire interval and excludes a midnight end', () => {
  const start = new Date(2026, 9, 29, 19, 0, 0);
  assert.deepEqual(coveringLocalDates(start, new Date(2026, 10, 5, 19, 0, 0)), {startDate:'2026-10-29',endDate:'2026-11-05'});
  assert.deepEqual(coveringLocalDates(start, new Date(2026, 10, 5, 0, 0, 0)), {startDate:'2026-10-29',endDate:'2026-11-04'});
  assert.throws(() => coveringLocalDates(start, start));
});
const {currencySymbol, billingLabel, formatSubscriptionPrice} = await loadModule(
  '../src/subscriptions/format.ts',
);
const {filterAndSort} = await loadModule('../src/subscriptions/list.ts');
const {parseUsageError} = await loadModule('../src/native/usageClient.ts');

test('weekly cycles calculate current, prior, next and partial weeks across year boundary', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2027, 0, 1, 12, 0, 0),
    current = weeklyCycle(rule, now),
    prior = weeklyCycle(rule, now, -1),
    next = weeklyCycle(rule, now, 1);
  assert.equal(current.start.getDay(), 4);
  assert.equal(current.start.getHours(), 19);
  assert.equal(current.start.getFullYear(), 2026);
  assert.equal(prior.end.getTime(), current.start.getTime());
  assert.equal(next.start.getTime(), current.end.getTime());
  const short = weeklyCycle({...rule, endDay: 6, endTime: '10:30:00'}, now);
  assert.equal(short.end.getDay(), 6);
  assert.equal(short.end.getHours(), 10);
  assert.throws(() => weeklyCycle({...rule, endTime: '99:00:00'}, now));
});

test('subscriptions become active at start and end at the exclusive end date', () => {
  const plan = {enabled: true, startDate: '2026-10-01', endDate: '2026-11-01'};
  assert.equal(activeSubscription(plan, new Date(2026, 9, 1, 0)), true);
  assert.equal(activeSubscription(plan, new Date(2026, 10, 1, 0)), false);
  assert.equal(activeSubscription({...plan, enabled: false}, new Date(2026, 9, 20)), false);
});

test('a non-existent daylight-saving wall time is rejected instead of shifted', () => {
  const original = process.env.TZ;
  process.env.TZ = 'America/New_York';
  try {
    assert.throws(
      () => weeklyCycle({startDay: 0, startTime: '02:30:00', endDay: 0, endTime: '02:30:00'}, new Date(2026, 2, 8, 12)),
      /不存在/,
    );
  } finally {
    if (original === undefined) delete process.env.TZ;
    else process.env.TZ = original;
  }
});

test('local date renders as a zero padded YYYY-MM-DD in local time', () => {
  assert.equal(localDate(new Date(2026, 0, 5, 23, 59)), '2026-01-05');
  assert.equal(localDate(new Date(2026, 10, 30, 0, 1)), '2026-11-30');
});

test('currency symbols and billing labels describe the recorded plan', () => {
  assert.equal(currencySymbol('USD'), '$');
  assert.equal(currencySymbol('CNY'), '¥');
  assert.equal(currencySymbol('EUR'), '€');
  // 未知币种退回 ISO 代码本身，不显示成一个空符号。
  assert.equal(currencySymbol('JPY'), 'JPY');
  assert.equal(billingLabel('月'), '月');
  assert.equal(billingLabel('年'), '年');
  assert.equal(billingLabel('期', 30), '每 30 天');
  assert.equal(billingLabel('期', null), '自定义');
  assert.equal(formatSubscriptionPrice(100, 'USD', '月'), '$100.00 / 月');
  assert.equal(formatSubscriptionPrice(30, 'CNY', '期', 30), '¥30.00 / 每 30 天');
});

test('subscription list filters by keyword and sorts by the chosen order', () => {
  const items = [
    {id: 'a', name: 'Claude Max', appType: 'claude', price: 100, startDate: '2026-01-01'},
    {id: 'b', name: 'Codex Pro', appType: 'codex', price: 20, startDate: '2026-05-01'},
    {id: 'c', name: 'Gemini 额度', appType: 'gemini', price: 50, startDate: '2026-03-01'},
  ];
  const activeIds = new Set(['a']);
  // 只看生效中的。
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'active', query: '', sort: 'start'}).map((s) => s.id),
    ['a'],
  );
  // 关键字匹配名称或应用标识，大小写不敏感。
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'all', query: 'codex', sort: 'start'}).map((s) => s.id),
    ['b'],
  );
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'all', query: 'GEMINI', sort: 'start'}).map((s) => s.id),
    ['c'],
  );
  // 排序：金额从高到低 / 开始日期。
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'all', query: '', sort: 'price'}).map((s) => s.id),
    ['a', 'c', 'b'],
  );
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'all', query: '', sort: 'start'}).map((s) => s.id),
    ['a', 'c', 'b'],
  );
  // 没有命中时返回空表，不报错。
  assert.deepEqual(
    filterAndSort(items, activeIds, {filter: 'all', query: '不存在的名字', sort: 'start'}),
    [],
  );
});

test('structured errors keep their code and details through the string channel', () => {
  const envelope = JSON.stringify({
    observatoryError: {
      error: '查询区间截断了仅有日汇总的历史日期，请改用完整日期',
      code: 'RANGE_PRECISION_UNAVAILABLE',
      details: {dates: ['2026-10-31', '2026-11-01']},
    },
  });
  const parsed = parseUsageError(new Error(envelope));
  assert.equal(parsed.code, 'RANGE_PRECISION_UNAVAILABLE');
  assert.equal(parsed.message, '查询区间截断了仅有日汇总的历史日期，请改用完整日期');
  assert.deepEqual(parsed.details.dates, ['2026-10-31', '2026-11-01']);
});

test('plain error messages pass through the parser unchanged', () => {
  const plain = parseUsageError(new Error('数据库错误: 磁盘已满'));
  assert.equal(plain.code, '');
  assert.equal(plain.message, '数据库错误: 磁盘已满');
  assert.equal(plain.details, undefined);
  // 恰好是合法 JSON、但没有信封标记的消息也不能被误判。
  const jsonish = parseUsageError(new Error('{"code":"x"}'));
  assert.equal(jsonish.code, '');
  assert.equal(jsonish.message, '{"code":"x"}');
});

test('a cycle is clamped to the subscription window: later start, earlier end', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2026, 10, 1, 12, 0, 0);
  // 当前周期：2026-10-29 19:00 → 2026-11-05 19:00
  const cycle = weeklyCycle(rule, now, 0);

  // 订阅在周期中途到期 → 结尾取较早者（到期时刻）。
  const expiring = clampCycleToWindow(cycle, {startDate: '2026-07-01', endDate: '2026-11-01'});
  assert.equal(expiring.start.getTime(), cycle.start.getTime());
  assert.equal(expiring.end.getFullYear(), 2026);
  assert.equal(expiring.end.getMonth(), 10);
  assert.equal(expiring.end.getDate(), 1);
  assert.equal(expiring.end.getHours(), 0);

  // 订阅在周期中途开始 → 开头取较晚者（开始日零点）。
  const starting = clampCycleToWindow(cycle, {startDate: '2026-10-30', endDate: '2026-12-01'});
  assert.equal(starting.start.getMonth(), 9);
  assert.equal(starting.start.getDate(), 30);
  assert.equal(starting.start.getHours(), 0);
  assert.equal(starting.end.getTime(), cycle.end.getTime());

  // 两端都被收敛。
  const both = clampCycleToWindow(cycle, {startDate: '2026-10-30', endDate: '2026-11-01'});
  assert.equal(both.start.getDate(), 30);
  assert.equal(both.end.getDate(), 1);

  // 周期完全落在有效期内 → 原样返回。
  const inside = clampCycleToWindow(cycle, {startDate: '2026-10-01', endDate: '2026-12-01'});
  assert.equal(inside.start.getTime(), cycle.start.getTime());
  assert.equal(inside.end.getTime(), cycle.end.getTime());
});

test('cycle navigation stops where the cycle no longer overlaps the window', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2026, 10, 1, 12, 0, 0);
  const window = {startDate: '2026-10-08', endDate: '2026-11-03'};
  const bounds = cycleOffsetRange(rule, window, now);

  // 下一周期 11-05 → 11-12 整段都在到期日之后 → 不可导航。
  assert.equal(bounds.max, 0);
  // 往回：10-01 → 10-08 与有效期还有交集（10-08 当天的 19 小时），再到
  // 10-01 之前就完全没有交集了。
  assert.equal(bounds.min, -4);
  assert.equal(cycleOverlapsWindow(weeklyCycle(rule, now, bounds.min), window), true);
  assert.equal(cycleOverlapsWindow(weeklyCycle(rule, now, bounds.min - 1), window), false);
  assert.equal(cycleOverlapsWindow(weeklyCycle(rule, now, bounds.max + 1), window), false);

  // 可导航的周期都能收敛出一段非空的区间。
  for (let offset = bounds.min; offset <= bounds.max; offset++) {
    const clamped = clampCycleToWindow(weeklyCycle(rule, now, offset), window);
    assert.ok(
      clamped.end.getTime() > clamped.start.getTime(),
      `offset ${offset} should clamp to a non-empty window`,
    );
  }
});

test('a subscription that starts mid cycle cannot go backwards', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2026, 10, 1, 12, 0, 0);
  const window = {startDate: '2026-10-30', endDate: '2026-12-01'};
  const bounds = cycleOffsetRange(rule, window, now);
  // 上一周期 10-22 → 10-29 整段都在开始日之前 → 不可导航。
  assert.equal(bounds.min, 0);
  assert.ok(bounds.max >= 3, `max should allow later cycles: ${bounds.max}`);
  assert.equal(cycleOverlapsWindow(weeklyCycle(rule, now, -1), window), false);
});

test('an expired subscription keeps history reachable but blocks the future', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2026, 10, 1, 12, 0, 0);
  const bounds = cycleOffsetRange(
    rule,
    {startDate: '2026-07-01', endDate: '2026-11-01'},
    now,
  );
  assert.equal(bounds.max, 0);
  assert.ok(bounds.min < 0, `history before the expiry should stay reachable: ${bounds.min}`);
  // 当前周期被收敛到到期时刻。
  const clamped = clampCycleToWindow(weeklyCycle(rule, now, 0), {
    startDate: '2026-07-01',
    endDate: '2026-11-01',
  });
  assert.equal(clamped.end.getDate(), 1);
  assert.equal(clamped.end.getHours(), 0);
});

test('an invalid window does not restrict navigation', () => {
  const rule = {startDay: 4, startTime: '19:00:00', endDay: 4, endTime: '19:00:00'};
  const now = new Date(2026, 10, 1, 12, 0, 0);
  const bounds = cycleOffsetRange(rule, {startDate: '', endDate: ''}, now);
  assert.deepEqual(bounds, {min: 0, max: 0});
  const cycle = weeklyCycle(rule, now, 0);
  const clamped = clampCycleToWindow(cycle, {startDate: '', endDate: ''});
  assert.equal(clamped.start.getTime(), cycle.start.getTime());
  assert.equal(clamped.end.getTime(), cycle.end.getTime());
});


test('current billing period handles month-end anchors and subscription boundaries', () => {
  const item={enabled:true,startDate:'2026-01-31',endDate:'2026-05-15',billing:'月'};
  const feb=currentBillingPeriod(item,new Date(2026,1,28,12));
  assert.equal(localDate(feb.start),'2026-02-28');
  assert.equal(localDate(feb.end),'2026-03-31');
  const apr=currentBillingPeriod(item,new Date(2026,4,1,12));
  assert.equal(localDate(apr.start),'2026-04-30');
  assert.equal(localDate(apr.end),'2026-05-15');
  assert.equal(localDate(currentBillingPeriod(item,new Date(2026,4,15,12)).start),'2026-04-30');
  assert.equal(localDate(currentBillingPeriod({...item,enabled:false},new Date(2026,2,1)).start),'2026-02-28');
  assert.equal(currentBillingPeriod(item,new Date(2026,0,1)),null);
});

test('current billing period supports annual and custom-day plans', () => {
  const annual=currentBillingPeriod({enabled:true,startDate:'2024-02-29',endDate:'2028-03-01',billing:'年'},new Date(2025,2,1));
  assert.equal(localDate(annual.start),'2025-02-28');
  assert.equal(localDate(annual.end),'2026-02-28');
  const custom=currentBillingPeriod({enabled:true,startDate:'2026-01-01',endDate:'2026-04-01',billing:'期',customIntervalDays:14},new Date(2026,0,20));
  assert.equal(localDate(custom.start),'2026-01-15');
  assert.equal(localDate(custom.end),'2026-01-29');
});


test('billing quota windows clip both edges without double counting', () => {
 const rule={startDay:4,startTime:'19:00:00',endDay:4,endTime:'19:00:00'};
 const start=new Date(2026,10,1), now=new Date(2026,10,10,15), end=new Date(2026,11,1);
 const rows=billingQuotaWindows(rule,{start,end},now);
 assert.equal(rows.length,2);
 assert.equal(rows[0].start.getTime(),start.getTime());
 assert.equal(rows[0].end.getTime(),rows[1].start.getTime());
 assert.equal(rows[1].end.getTime(),now.getTime());
 assert.ok(rows[0].cycleStart<start);
 assert.ok(rows[1].cycleEnd>now);
});
test('billing quota windows preserve gaps in the configured rule', () => {
 const rule={startDay:1,startTime:'00:00:00',endDay:3,endTime:'00:00:00'};
 const rows=billingQuotaWindows(rule,{start:new Date(2026,10,1),end:new Date(2026,11,1)},new Date(2026,10,15));
 assert.equal(rows.length,2);
 assert.ok(rows[0].end<rows[1].start);
});
