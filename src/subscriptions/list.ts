/** Search and sort for the subscription list. Kept as pure functions for easy testing. */

// localeCompare resolves the locale on every call: use a module-level Collator
// in the sort comparator instead.
const NAME_COLLATOR = new Intl.Collator("zh-Hans-CN");

export type SubscriptionLike = {
  id: string;
  name: string;
  appType: string;
  price: number;
  startDate: string;
};

export type ListOptions = {
  /** `active` shows only active subscriptions, `all` shows everything. */
  filter: "active" | "all";
  /** Keyword matched against the name or the app identifier. */
  query: string;
  sort: "start" | "name" | "price";
};

export function filterAndSort<T extends SubscriptionLike>(
  items: T[],
  activeIds: ReadonlySet<string>,
  options: ListOptions,
): T[] {
  const base =
    options.filter === "active"
      ? items.filter((item) => activeIds.has(item.id))
      : items;
  const needle = options.query.trim().toLowerCase();
  const filtered = needle
    ? base.filter(
        (item) =>
          item.name.toLowerCase().includes(needle) ||
          item.appType.toLowerCase().includes(needle),
      )
    : base;
  return [...filtered].sort((a, b) => {
    if (options.sort === "name") return NAME_COLLATOR.compare(a.name, b.name);
    if (options.sort === "price") return b.price - a.price;
    return a.startDate.localeCompare(b.startDate);
  });
}
