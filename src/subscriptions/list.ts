/** 订阅列表的检索与排序。独立成纯函数，方便直接测。 */

export type SubscriptionLike = {
  id: string;
  name: string;
  appType: string;
  price: number;
  startDate: string;
};

export type ListOptions = {
  /** `active` 只看生效中的，`all` 看全部。 */
  filter: "active" | "all";
  /** 关键字，匹配名称或应用标识。 */
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
    if (options.sort === "name") return a.name.localeCompare(b.name, "zh-Hans-CN");
    if (options.sort === "price") return b.price - a.price;
    return a.startDate.localeCompare(b.startDate);
  });
}
