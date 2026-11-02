/** 订阅模块的展示格式化助手。 */

/** 币种符号。没有符号的币种退回 ISO 代码本身。 */
const CURRENCY_SYMBOLS: Record<string, string> = {
  USD: "$",
  CNY: "¥",
  EUR: "€",
};

export function currencySymbol(code: string): string {
  return CURRENCY_SYMBOLS[code] ?? code;
}

/**
 * 账期标签。
 *
 * `月` / `年` 是固定周期；`期` 表示自定义，靠 `customIntervalDays` 说明
 * 一个账期跨越多少天。
 */
export function billingLabel(
  billing: string,
  customIntervalDays?: number | null,
): string {
  if (billing === "期") {
    return customIntervalDays ? `每 ${customIntervalDays} 天` : "自定义";
  }
  return billing;
}

/** 订阅金额，例如 `$100.00 / 月`、`¥30.00 / 每 30 天`。 */
export function formatSubscriptionPrice(
  price: number,
  currency: string,
  billing: string,
  customIntervalDays?: number | null,
): string {
  const amount = Number.isFinite(price) ? price : 0;
  return `${currencySymbol(currency)}${amount.toFixed(2)} / ${billingLabel(
    billing,
    customIntervalDays,
  )}`;
}
