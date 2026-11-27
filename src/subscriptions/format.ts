/** Display formatting helpers for the subscriptions module. */

export type SubscriptionCurrency = {
  readonly code: string;
  readonly label: string;
  readonly symbol: string;
  /** True for currencies normally priced in whole units (e.g. JPY, KRW, VND, IDR). */
  readonly zeroDecimal?: boolean;
};

/**
 * Major market currencies supported for subscription billing records (ISO 4217).
 * Keep codes in sync with `SUPPORTED_CURRENCIES` in `native/src/subscriptions.rs`.
 */
export const SUBSCRIPTION_CURRENCIES: readonly SubscriptionCurrency[] = [
  { code: "USD", label: "美元", symbol: "$" },
  { code: "CNY", label: "人民币", symbol: "¥" },
  { code: "TWD", label: "新台币", symbol: "NT$" },
  { code: "HKD", label: "港币", symbol: "HK$" },
  { code: "JPY", label: "日元", symbol: "JP¥", zeroDecimal: true },
  { code: "EUR", label: "欧元", symbol: "€" },
  { code: "GBP", label: "英镑", symbol: "£" },
  { code: "KRW", label: "韩元", symbol: "₩", zeroDecimal: true },
  { code: "SGD", label: "新加坡元", symbol: "S$" },
  { code: "AUD", label: "澳元", symbol: "A$" },
  { code: "CAD", label: "加元", symbol: "CA$" },
  { code: "CHF", label: "瑞士法郎", symbol: "CHF" },
  { code: "NZD", label: "新西兰元", symbol: "NZ$" },
  { code: "MOP", label: "澳门元", symbol: "MOP$" },
  { code: "INR", label: "印度卢比", symbol: "₹" },
  { code: "MYR", label: "马来西亚林吉特", symbol: "RM" },
  { code: "THB", label: "泰铢", symbol: "฿" },
  { code: "VND", label: "越南盾", symbol: "₫", zeroDecimal: true },
  { code: "IDR", label: "印尼盾", symbol: "Rp", zeroDecimal: true },
  { code: "PHP", label: "菲律宾比索", symbol: "₱" },
  { code: "SEK", label: "瑞典克朗", symbol: "SEK" },
  { code: "NOK", label: "挪威克朗", symbol: "NOK" },
  { code: "DKK", label: "丹麦克朗", symbol: "DKK" },
  { code: "PLN", label: "波兰兹罗提", symbol: "PLN" },
  { code: "TRY", label: "土耳其里拉", symbol: "₺" },
  { code: "BRL", label: "巴西雷亚尔", symbol: "R$" },
  { code: "MXN", label: "墨西哥比索", symbol: "MX$" },
  { code: "ZAR", label: "南非兰特", symbol: "ZAR" },
  { code: "AED", label: "阿联酋迪拉姆", symbol: "AED" },
  { code: "SAR", label: "沙特里亚尔", symbol: "SAR" },
];

const CURRENCY_BY_CODE = new Map(
  SUBSCRIPTION_CURRENCIES.map((currency) => [currency.code, currency]),
);

const AMOUNT_FORMATS = new Map<number, Intl.NumberFormat>();
function formatAmount(amount: number, decimals: number): string {
  let fmt = AMOUNT_FORMATS.get(decimals);
  if (!fmt) {
    fmt = new Intl.NumberFormat("en-US", {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    });
    AMOUNT_FORMATS.set(decimals, fmt);
  }
  return fmt.format(amount);
}

/** Currency symbol for an ISO 4217 code, falling back to the code itself when unknown. */
export function currencySymbol(code: string): string {
  return CURRENCY_BY_CODE.get(code)?.symbol ?? code;
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

/** Subscription price, e.g. `$100.00 / 月`, `NT$650.00 / 月`, `JP¥3,000 / 月`, `¥30.00 / 每 30 天`. */
export function formatSubscriptionPrice(
  price: number,
  currency: string,
  billing: string,
  customIntervalDays?: number | null,
): string {
  const amount = Number.isFinite(price) && price !== 0 ? price : 0;
  const info = CURRENCY_BY_CODE.get(currency);
  const symbol = info?.symbol ?? currency;
  const decimals = info?.zeroDecimal && Number.isInteger(amount) ? 0 : 2;
  const separator = /[A-Za-z]$/.test(symbol) ? " " : "";
  return `${symbol}${separator}${formatAmount(amount, decimals)} / ${billingLabel(
    billing,
    customIntervalDays,
  )}`;
}
