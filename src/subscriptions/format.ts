/** Display formatting helpers for the subscriptions module. */

/** Currency symbols. Currencies without a symbol fall back to their ISO code. */
const CURRENCY_SYMBOLS: Record<string, string> = {
  USD: "$",
  CNY: "¥",
  EUR: "€",
};

export function currencySymbol(code: string): string {
  return CURRENCY_SYMBOLS[code] ?? code;
}

/**
 * Billing period label.
 *
 * `月` / `年` are fixed periods; `期` means a custom one, described by
 * `customIntervalDays` as the number of days a billing period spans.
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

/** Subscription price, e.g. `$100.00 / 月`, `¥30.00 / 每 30 天`. */
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
