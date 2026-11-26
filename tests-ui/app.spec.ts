import { expect, test, type Page } from "@playwright/test";

// Fail a test on any uncaught error in the page.
test.beforeEach(async ({ page }) => {
  page.on("pageerror", (err) => {
    throw err;
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
});

/** Open a Markets pool, allowing for a cold engine (first run, no cache yet). */
const openPool = async (page: Page, name: RegExp) => {
  const button = page.getByRole("button", { name });
  await expect(button).toBeVisible({ timeout: 30_000 });
  await button.click();
};

const openPage = async (page: Page, label: string) => {
  await page
    .locator(".app-sidebar")
    .getByRole("button", { name: label, exact: true })
    .click();
};

test("every page in the sidebar opens with its title", async ({ page }) => {
  const pages: [string, string][] = [
    ["AI 用量", "AI 用量"],
    ["洞察", "洞察"],
    ["订阅与额度", "订阅与额度"],
    ["模型榜单", "模型榜单"],
    ["市场行情", "市场行情"],
    ["多设备", "多设备"],
    ["网络诊断", "网络诊断"],
    ["活动日志", "活动日志"],
    ["总览", "运行总览"],
  ];
  for (const [nav, title] of pages) {
    await openPage(page, nav);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(title);
  }
});

test("AI usage offers the shared time ranges and remembers the choice", async ({
  page,
}) => {
  await openPage(page, "AI 用量");
  const range = page.getByLabel("统计时间");
  await expect(range.locator("option")).toHaveText([
    "今天",
    "昨天",
    "近 3 天",
    "近 7 天",
    "近 30 天",
    "全部时间",
    "自定义时间",
    "按周期查询",
  ]);
  await range.selectOption("cycle");
  await expect(page.locator("[data-cycle-navigator]")).toBeVisible();
  await range.selectOption("3d");
  await page.reload();
  await openPage(page, "AI 用量");
  await expect(page.getByLabel("统计时间")).toHaveValue("3d");
});

test("Insights switches between its time ranges", async ({ page }) => {
  await openPage(page, "洞察");
  const group = page.getByRole("group", { name: "时间范围" });
  await expect(group.getByRole("button")).toHaveText([
    "今天",
    "昨天",
    "近 3 天",
    "近 7 天",
    "近 14 天",
    "近 30 天",
    "本月",
  ]);
  await group.getByRole("button", { name: "昨天" }).click();
  await expect(group.getByRole("button", { name: "昨天" })).toHaveClass(
    /active/,
  );
});

test("Markets lists the built-in pools and switches views", async ({
  page,
}) => {
  await openPage(page, "市场行情");
  await expect(
    page.getByRole("button", { name: /中概股 \(53\)/ }),
  ).toBeVisible();
  await openPool(page, /中概股 \(53\)/);
  // The heat map fits in the window: its bottom edge is visible without scrolling.
  const canvas = page.locator(".markets-hm-canvas");
  await expect(canvas).toBeVisible();
  const box = await canvas.boundingBox();
  const viewport = page.viewportSize();
  expect(box && viewport && box.y + box.height <= viewport.height).toBe(true);
  // In list view the pool description is shown (in heat-map view it is a tooltip).
  await page.getByRole("button", { name: /列表/ }).click();
  await expect(page.getByText("美股上市的中国公司 53 家")).toBeVisible();
  await expect(page.locator("table")).toBeVisible();
});

test("Activity Log hides routine polls by default", async ({ page }) => {
  await openPage(page, "活动日志");
  const toggle = page.getByLabel("隐藏例行同步与行情轮询");
  await expect(toggle).toBeChecked();
  await page
    .getByRole("group", { name: "按类型筛选" })
    .getByRole("button", { name: "接口请求" })
    .click();
  await expect(
    page
      .getByRole("group", { name: "按类型筛选" })
      .getByRole("button", { name: "接口请求" }),
  ).toHaveAttribute("aria-pressed", "true");
});

test("Benchmarks without a key offers setup and does not fetch", async ({
  page,
}) => {
  const fetches: string[] = [];
  page.on("request", (req) => {
    const body = req.postData() ?? "";
    if (body.includes("benchmarks_fetch")) fetches.push(body);
  });
  await openPage(page, "模型榜单");
  const setup = page.getByRole("button", { name: "配置 API Key" });
  await expect(setup).toBeVisible();
  await setup.click();
  await expect(
    page.getByPlaceholder(/输入 Artificial Analysis API Key/),
  ).toBeVisible();
  expect(fetches).toEqual([]);
});

test("The pool editor removes a company and restores the default", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await openPage(page, "市场行情");
  await openPool(page, /中概股 \(53\)/);
  await page.getByRole("button", { name: "编辑板块" }).click();
  const dialog = page.getByRole("dialog", { name: "编辑股票板块" });
  await expect(dialog.getByText("共 53 家")).toBeVisible();
  await dialog.getByRole("button", { name: "移除 BABA" }).click();
  await expect(dialog.getByText("共 52 家")).toBeVisible();
  await expect(dialog.getByText("已自定义")).toBeVisible();
  await dialog.getByRole("button", { name: /恢复默认/ }).click();
  await expect(dialog.getByText("共 53 家")).toBeVisible();
  await dialog.getByRole("button", { name: "关闭" }).first().click();
  await expect(dialog).toBeHidden();
});

test("The heat map swaps in a complete snapshot", async ({ page }) => {
  test.setTimeout(60_000);
  await openPage(page, "市场行情");
  await openPool(page, /中概股 \(53\)/);
  const canvas = page.locator(".markets-hm-canvas");
  // While quotes load the canvas is busy (shimmer or dimmed previous map);
  // once they arrive every stock of the pool is drawn at once.
  await expect(canvas).toHaveAttribute("aria-busy", "false", {
    timeout: 30_000,
  });
  await expect(canvas.locator(".markets-hm-tile")).toHaveCount(53);
  await expect(canvas.locator(".markets-hm-skeleton")).toHaveCount(0);
});
