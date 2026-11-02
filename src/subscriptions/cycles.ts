export type WeeklyRule = {
  startDay: number;
  startTime: string;
  endDay: number;
  endTime: string;
};
function localOccurrence(base: Date, day: number, time: string): Date {
  if (!/^\d{2}:\d{2}(:\d{2})?$/.test(time)) throw new Error("周期时间无效");
  const [hour, minute, second = 0] = time.split(":").map(Number);
  if (
    !Number.isInteger(day) ||
    day < 0 ||
    day > 6 ||
    !Number.isInteger(hour) ||
    hour < 0 ||
    hour > 23 ||
    !Number.isInteger(minute) ||
    minute < 0 ||
    minute > 59 ||
    !Number.isInteger(second) ||
    second < 0 ||
    second > 59
  )
    throw new Error("周期时间无效");
  const date = new Date(base);
  date.setHours(12, 0, 0, 0);
  date.setDate(date.getDate() - ((date.getDay() - day + 7) % 7));
  const expectedDate = date.getDate();
  date.setHours(hour, minute, second, 0);
  if (
    date.getDate() !== expectedDate ||
    date.getHours() !== hour ||
    date.getMinutes() !== minute ||
    date.getSeconds() !== second
  )
    throw new Error("该周期时间在本机时区不存在，请调整设置");
  return date;
}
function shiftWallTime(date: Date, days: number, time: string): Date {
  const [hour, minute, second = 0] = time.split(":").map(Number);
  const result = new Date(date);
  result.setHours(12, 0, 0, 0);
  result.setDate(result.getDate() + days);
  const expected = result.getDate();
  result.setHours(hour, minute, second, 0);
  if (
    result.getDate() !== expected ||
    result.getHours() !== hour ||
    result.getMinutes() !== minute ||
    result.getSeconds() !== second
  )
    throw new Error("该周期时间在本机时区不存在，请调整设置");
  return result;
}
export function weeklyCycle(rule: WeeklyRule, now = new Date(), offset = 0) {
  if (
    !Number.isInteger(rule.endDay) ||
    rule.endDay < 0 ||
    rule.endDay > 6 ||
    !/^\d{2}:\d{2}(:\d{2})?$/.test(rule.endTime) ||
    !Number.isInteger(offset)
  )
    throw new Error("周期时间无效");
  let start = localOccurrence(now, rule.startDay, rule.startTime);
  if (start > now) start = shiftWallTime(start, -7, rule.startTime);
  if (offset) start = shiftWallTime(start, offset * 7, rule.startTime);
  const [hour, minute, second = 0] = rule.endTime.split(":").map(Number);
  const endBase = new Date(start);
  endBase.setHours(12, 0, 0, 0);
  endBase.setDate(endBase.getDate() + ((rule.endDay - rule.startDay + 7) % 7));
  const day = endBase.getDate();
  endBase.setHours(hour, minute, second, 0);
  if (
    endBase.getDate() !== day ||
    endBase.getHours() !== hour ||
    endBase.getMinutes() !== minute ||
    endBase.getSeconds() !== second
  )
    throw new Error("该周期时间在本机时区不存在，请调整设置");
  const end =
    endBase <= start ? shiftWallTime(endBase, 7, rule.endTime) : endBase;
  return { start, end };
}
export function localDate(value = new Date()) {
  return [
    value.getFullYear(),
    String(value.getMonth() + 1).padStart(2, "0"),
    String(value.getDate()).padStart(2, "0"),
  ].join("-");
}
/** Calendar dates touched by a left-closed, right-open interval. */
export function coveringLocalDates(start: Date, endExclusive: Date) {
  if (
    !Number.isFinite(start.getTime()) ||
    !Number.isFinite(endExclusive.getTime()) ||
    endExclusive <= start
  )
    throw new Error("查询时间范围无效");
  return {
    startDate: localDate(start),
    endDate: localDate(new Date(endExclusive.getTime() - 1)),
  };
}
export function activeSubscription(
  item: { enabled: boolean; startDate: string; endDate: string },
  now = new Date(),
) {
  const today = localDate(now);
  return item.enabled && item.startDate <= today && today < item.endDate;
}

/** 订阅的有效期。`endDate` 不含当日，所以当天零点就是到期时刻。 */
export type SubscriptionWindow = { startDate: string; endDate: string };

/** 本机时区当天零点。日期串非法时返回 null。 */
function startOfDay(date: string): Date | null {
  const matched = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!matched) return null;
  const value = new Date(
    Number(matched[1]),
    Number(matched[2]) - 1,
    Number(matched[3]),
    0,
    0,
    0,
    0,
  );
  return Number.isNaN(value.getTime()) ? null : value;
}

/**
 * 把周期收敛到订阅有效期内。
 *
 * 开头取「订阅开始时间」与「周期开始时间」的较晚者，结尾取「周期结束时间」
 * 与「订阅到期时刻」的较早者——也就是两者的交集。订阅在周期中途开始或到期
 * 时，只统计被订阅覆盖的那一段。
 */
export function clampCycleToWindow(
  cycle: { start: Date; end: Date },
  window: SubscriptionWindow,
): { start: Date; end: Date } {
  const start = startOfDay(window.startDate);
  const end = startOfDay(window.endDate);
  if (!start || !end || end.getTime() <= start.getTime()) return cycle;
  return {
    start: new Date(Math.max(cycle.start.getTime(), start.getTime())),
    end: new Date(Math.min(cycle.end.getTime(), end.getTime())),
  };
}

/** 周期是否与有效期有交集。没有交集的周期不覆盖任何订阅额度。 */
export function cycleOverlapsWindow(
  cycle: { start: Date; end: Date },
  window: SubscriptionWindow,
): boolean {
  const start = startOfDay(window.startDate);
  const end = startOfDay(window.endDate);
  if (!start || !end || end.getTime() <= start.getTime()) return true;
  return (
    cycle.end.getTime() > start.getTime() &&
    cycle.start.getTime() < end.getTime()
  );
}

/**
 * 可导航的周期偏移范围。
 *
 * 与有效期有交集的周期都算可导航：交集里的那部分用量由这份订阅覆盖。
 * 整段都早于开始日、或整段都晚于到期时刻的周期没有交集，就停在那儿。
 *
 * 返回的 `[min, max]` 是相对当前周期的偏移量：偏移越大周期越晚。
 * 有效期不合法时不做限制。
 */
export function cycleOffsetRange(
  rule: WeeklyRule,
  window: SubscriptionWindow,
  now = new Date(),
): { min: number; max: number } {
  const start = startOfDay(window.startDate);
  const end = startOfDay(window.endDate);
  if (!start || !end || end.getTime() <= start.getTime())
    return { min: 0, max: 0 };
  const cycleAt = (offset: number) => {
    try {
      return weeklyCycle(rule, now, offset);
    } catch {
      // 夏令时让这个周期无法计算：不能算成「与有效期有交集」。
      return null;
    }
  };

  // 往回找：整段都早于开始日的周期没有交集，再往前只会更早。
  let min = 0;
  for (let offset = 0; offset > -400; offset--) {
    const cycle = cycleAt(offset);
    if (!cycle) break;
    if (cycle.end.getTime() <= start.getTime()) break;
    min = offset;
  }

  // 往后找：整段都晚于到期时刻的周期没有交集。
  let max = 0;
  for (let offset = 0; offset < 400; offset++) {
    const cycle = cycleAt(offset);
    if (!cycle) break;
    if (cycle.start.getTime() >= end.getTime()) break;
    max = offset;
  }

  return { min, max };
}

/** Current billing interval anchored to the subscription start date. */
export function currentBillingPeriod(item: {
  enabled: boolean; startDate: string; endDate: string; billing: string;
  customIntervalDays?: number | null;
}, now = new Date()): { start: Date; end: Date } | null {
  const start = startOfDay(item.startDate);
  const end = startOfDay(item.endDate);
  if (!start || !end || end <= start || localDate(start) !== item.startDate || localDate(end) !== item.endDate) return null;
  if (now < start) return null;
  // For expired subscriptions use the final instant of the validity window.
  const effectiveNow = new Date(Math.min(now.getTime(), end.getTime() - 1));
  const anchorYear = start.getFullYear(), anchorMonth = start.getMonth(), anchorDay = start.getDate();
  const boundary = (index: number): Date => {
    if (item.billing === "期") {
      const days = item.customIntervalDays;
      if (!days || !Number.isInteger(days) || days < 1 || days > 366) throw new Error("自定义账期无效");
      return new Date(anchorYear, anchorMonth, anchorDay + index * days);
    }
    if (item.billing !== "月" && item.billing !== "年") throw new Error("账期设置无效");
    const month = anchorMonth + index * (item.billing === "年" ? 12 : 1);
    const lastDay = new Date(anchorYear, month + 1, 0).getDate();
    return new Date(anchorYear, month, Math.min(anchorDay, lastDay));
  };
  let index = 0;
  while (boundary(index + 1) <= effectiveNow) {
    if (++index > 12000) throw new Error("账期超出可查询范围");
  }
  return { start: boundary(index), end: new Date(Math.min(boundary(index + 1).getTime(), end.getTime())) };
}

/** Exact weekly quota windows intersecting the elapsed part of a billing period. */
export function billingQuotaWindows(rule: WeeklyRule, billing: { start: Date; end: Date }, now = new Date()) {
  const end = new Date(Math.min(billing.end.getTime(), Math.floor(now.getTime() / 1000) * 1000));
  if (end <= billing.start) return [];
  const windows: { cycleStart: Date; cycleEnd: Date; start: Date; end: Date }[] = [];
  // Use the last instant of the interval as anchor so a historical boundary
  // does not include the following quota cycle.
  const anchor = new Date(end.getTime() - 1);
  for (let offset = 0; offset >= -54; offset--) {
    const cycle = weeklyCycle(rule, anchor, offset);
    if (cycle.end <= billing.start) break;
    const start = new Date(Math.max(cycle.start.getTime(), billing.start.getTime()));
    const clippedEnd = new Date(Math.min(cycle.end.getTime(), end.getTime()));
    if (start < clippedEnd) windows.push({ cycleStart:cycle.start, cycleEnd:cycle.end, start, end:clippedEnd });
    if (offset === -54) throw new Error("账期内的额度周期过多");
  }
  return windows.reverse();
}
