import { type NativeStatus } from "./types";
import { customTimeRange, presetRange, type RangePreset } from "./timeRange";
import { localDayKey, localDayStart } from "./useLocalDayKey";
import { getFreshInputTokens, type RequestLog } from "../types/usage";

export type Props = {
  interval: string;
  onRefreshReady?: (fn: () => Promise<void>) => void;
  onBusy?: (busy: boolean) => void;
  onStatus?: (status: NativeStatus) => void;
};

export type Range = RangePreset | "all" | "custom" | "cycle";

export type UsageScope = "local" | "multi";

export type ScanFeedback = { notice: string; errors: string[] };

export const multiScanEvent = "observatory-multi-scan-change";

export const saveView = (value: unknown) => {
  try {
    localStorage.setItem("observatory.usage.view", JSON.stringify(value));
  } catch {
    // When storage is unavailable the view is not remembered, which does not
    // affect usage.
  }
};

export function loadView(): {
  scope?: string;
  range?: string;
  app?: string;
  model?: string;
  provider?: string;
  cyclePlanId?: string;
} {
  try {
    return JSON.parse(localStorage.getItem("observatory.usage.view") || "{}");
  } catch {
    return {};
  }
}

export function bounds(
  range: Range,
  start: string,
  end: string,
  dayKey = localDayKey(),
): { startDate?: number; endDate?: number } {
  if (range === "all" || range === "cycle") return {};
  if (range === "custom") {
    return customTimeRange(start, end) ?? {};
  }
  return presetRange(range, localDayStart(dayKey));
}

export const dateInput = (date = new Date()) =>
  [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");

export function metricValue(log: RequestLog) {
  const fresh = getFreshInputTokens(log);
  return (
    fresh + log.outputTokens + log.cacheReadTokens + log.cacheCreationTokens
  );
}

// Called for every row of the detail table: constructing an Intl formatter is
// the most expensive object allocation on the hot path, so reuse a singleton.
export const TIME_TEXT_FORMAT = new Intl.DateTimeFormat("zh-CN", {
  hour12: false,
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

export function timeText(value: number) {
  return TIME_TEXT_FORMAT.format(new Date(value * 1000));
}

export function exportPage(rows: RequestLog[]) {
  const fields = [
    "时间",
    "应用",
    "供应商",
    "模型",
    "新增输入 Token",
    "输出 Token",
    "缓存读取 Token",
    "缓存写入 Token",
    "费用 USD",
    "状态码",
    "数据来源",
  ];
  const quote = (v: unknown) => {
    let text = String(v ?? "");
    if (/^[=+@\t\r-]/.test(text)) text = "'" + text;
    return '"' + text.replace(/"/g, '""') + '"';
  };
  const lines = [
    fields,
    ...rows.map((r) => [
      new Date(r.createdAt * 1000).toISOString(),
      r.appType,
      r.providerName,
      r.model,
      getFreshInputTokens(r),
      r.outputTokens,
      r.cacheReadTokens,
      r.cacheCreationTokens,
      r.totalCostUsd,
      r.statusCode,
      r.dataSource,
    ]),
  ];
  const url = URL.createObjectURL(
    new Blob(
      ["\uFEFF" + lines.map((row) => row.map(quote).join(",")).join("\r\n")],
      { type: "text/csv;charset=utf-8" },
    ),
  );
  const a = document.createElement("a");
  a.href = url;
  a.download = `observatory-usage-${dateInput()}.csv`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
