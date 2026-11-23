import { expect, test, type Page } from "@playwright/test";

// Fail a test on any uncaught error in the page.
test.beforeEach(async ({ page }) => {
  page.on("pageerror", (err) => {
    throw err;
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
});

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
    ["多设备", "多设备"],
    ["网络诊断", "网络诊断"],
    ["市场行情", "市场行情"],
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
  ]);
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
  await page.getByRole("button", { name: /中概股 \(53\)/ }).click();
  await expect(page.getByText("美股上市的中国公司 53 家")).toBeVisible();
  await page.getByRole("button", { name: /列表/ }).click();
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
