# AI 用量查询接口设计

版本：设计稿 v1.0  
日期：2026-11-01  
状态：已实现（等待评审）  
适用项目：观测台 Observatory

> 实现说明：本文描述的扩展协议已落地在 `native/src/lan_usage/public_api.rs`、
> `native/src/lan_usage/server.rs` 与 `native/src/services/usage_stats.rs`，
> 验收用例见 `native/tests/usage_http_api.rs`。
> 面向用户的说明见 [只读用量 HTTP 接口](./只读用量HTTP接口.md)。

## 1. 目标

向 TinyPane、自动化脚本及其他 HTTP 客户端提供只读统计查询：

- 查询指定时间范围内的 Token 用量、估算花费和请求次数。
- 返回整个查询区间的总用量、总花费，以及按天、周或月分组的数据。
- 按应用、模型筛选；Claude Code 使用 `appType=claude`，不混入 Claude Desktop。
- 用同一个响应满足数字卡片和趋势图，客户端无需自行筛选明细或计算分组。

本设计的“总”均指**所选时间范围内的合计**，不是账号全历史或订阅额度。例如按月查询一年时，顶层是这一年的合计，每个 bucket 是对应月份的统计。不额外返回另一套全历史累计值。

第一阶段只查询本机保存的统计，不聚合其他设备快照。多设备模式需要另行定义各设备时区、快照新鲜度及缺失状态，暂不纳入本协议。

## 2. 当前能力与新增能力

当前 v0.1.30 使用说明见 [只读用量 HTTP 接口](./只读用量HTTP接口.md)。本文描述扩展后的目标协议；下表「本设计」一列的条目已实现。

| 能力 | v0.1.30 | 本设计（已实现） |
|---|---|---|
| GET `/api/v1/usage/summary` | 已支持 | 保留 |
| `days`、`endDate`、`appType` | 已支持 | 保留原有含义 |
| 数字类型的 Token、费用、请求数 | 已支持 | 保留原有字段 |
| `daily` 每日数据 | 已支持 | 保留，兼容现有组件 |
| `startDate` 指定日期区间 | 未支持 | 新增 |
| `startTime` / `endTime` 秒级区间 | 未支持 | 新增，受历史精度限制 |
| `model` 模型筛选 | 未支持 | 新增 |
| `groupBy=day/week/month` | 未支持 | 新增 |
| `buckets` 分组结果 | 未支持 | 新增 |
| 全响应读取一致性 | 当前无跨查询事务快照保证 | 已由 `Database::with_connection` 保证 |

不更名、不改变顶层 `totalTokens`、`costUsd`、`requests` 和四类 Token 的含义。增加字段而不在外层包裹 `data` 或 `summary`，以保持 TinyPane 的 `/totalTokens` 等路径有效。

## 3. 服务与认证

```http
GET /api/v1/usage/summary
X-API-Key: <只读密钥>
Accept: application/json
```

- 默认服务地址：`http://127.0.0.1:17891`；端口随本机共享服务配置变化。
- 其他机器访问时使用服务器可达的局域网 IP。
- 密钥通过「AI 用量 → 管理设备 → 只读用量 HTTP 接口」生成、重置和停用。
- 保留已配对设备使用 `x-observatory-device` 和 `x-observatory-token` 的兼容方式；两种凭证任一有效即可。
- API 密钥只授予统计查询权限，不能配对、修改设置、读取对话内容或触发扫描。
- 查询范围始终受本机允许共享的应用限制；无应用筛选时也不得返回未允许共享的应用。
- 沿用当前明文 HTTP 部署方式；本设计不配置公网映射、TLS 或账号系统。

当前接口依赖局域网共享服务开关。独立的“对外 API 开关”可另行设计，本文不将它描述为已存在或本次必做的查询参数。

## 4. 请求参数

| 参数 | 类型 | 默认值 | 规则 |
|---|---|---|---|
| `days` | 整数 | 7 | 1–366，包含结束日在内的日历天数 |
| `endDate` | 日期字符串 | 服务器今天 | `YYYY-MM-DD`，包含该日，不得晚于服务器今天 |
| `startDate` | 日期字符串 | 无 | 新增：包含该日，必须不晚于 endDate |
| `startTime` | RFC 3339 字符串 | 无 | 新增：开始时刻，包含；必须携带 UTC 偏移或 Z |
| `endTime` | RFC 3339 字符串 | 无 | 新增：结束时刻，不包含；必须携带 UTC 偏移或 Z |
| `appType` | 字符串 | 无 | 不填时合计所有允许共享的应用；指定值必须受支持且允许共享 |
| `model` | 字符串 | 无 | 新增：按有效计价模型 ID 精确筛选，与页面模型筛选口径一致 |
| `groupBy` | 枚举字符串 | `day` | 新增：`day`、`week`、`month` |

### 4.1 三种时间选择方式

**最近 N 天**：使用 `days` 和可选的 `endDate`。都不填时默认最近 7 天、结束于服务器今天；只填 `endDate` 时查询截至该日的 7 天。

**指定日期区间**：填写 `startDate`，`endDate` 可省略，省略时为服务器今天。不得同时传 `days`。区间两端日期均包含。

**指定秒级区间**：`startTime` 和 `endTime` 必须成对提供，且不能与 `days`、`startDate`、`endDate` 混用。精确到整秒，不接受无时区或小数秒。必须满足 `startTime < endTime`。

三种模式归一化为 `[start, endExclusive)`。开始边界包含，结束边界不包含。例如查询 10 月时，结束时刻为 11 月 1 日 00:00:00，而不是再包含这一秒。

### 4.2 范围上限和时区

- 按服务器本地日历最多覆盖 366 个日期；秒级模式按 `start` 至 `endExclusive - 1 秒` 所触及的本地日期计算。
- 日期模式不允许结束日晚于服务器今天。秒级模式的结束时刻最多为服务器明天的本地午夜，允许查询“截至今天结束”的现有记录；接口不会产生未来数据。
- 所有日、周、月边界都按**服务器本地时区**划分。请求时间携带的偏移用于确定时刻，不会改变分组时区。
- 响应给出起止时刻各自的 UTC 偏移；夏令时转换日不保证是 24 小时。
- 日期模式或 day/week/month 分组遇到本地不存在的午夜边界时，统一返回 400 `INVALID_LOCAL_BOUNDARY`，不静默平移。遇到重复午夜时选择较早的时刻，沿用 v0.1.30 的 `.earliest()` 规则；秒级请求自身通过显式 UTC 偏移确定时刻。
- 参数名称区分大小写；重复参数、未知参数、空筛选值、非法日期及不支持的分组方式返回 400。

### 4.3 应用与模型筛选

`claude` 表示 Claude Code，`claude-desktop` 表示 Claude Desktop；其他标识沿用现有应用列表，例如 `codex`、`gemini`、`dsh`、`zcode`。

`model` 的匹配口径沿用页面：`COALESCE(NULLIF(pricing_model, ''), model)`。计价模型为 NULL 或空字符串时回落到 `model`；其他情况下使用计价模型。此规则同时适用于明细和归档。接口必须在文档及响应中标明 `modelFilterBasis=effectivePricingModel`，不能将它解释为对用户可见的友好名称匹配。

指定未共享应用返回 403；未支持的应用标识返回 400。格式合法但没有匹配记录的模型返回 200 和零值。省略 `appType` 时，只在允许共享的应用集合中按模型筛选。

## 5. 调用示例

### 今天的 Claude Code

```http
GET /api/v1/usage/summary?days=1&appType=claude
X-API-Key: <只读密钥>
```

### 最近 30 天，按天聚合

```http
GET /api/v1/usage/summary?days=30&appType=claude&groupBy=day
```

### 指定日期区间，按周聚合

```http
GET /api/v1/usage/summary?startDate=2026-10-01&endDate=2026-10-31&appType=claude&groupBy=week
```

### 指定日期区间，按月聚合

```http
GET /api/v1/usage/summary?startDate=2026-01-01&endDate=2026-10-31&groupBy=month
```

### 指定秒级范围和模型

```http
GET /api/v1/usage/summary?startTime=2026-10-31T09:00:00%2B08:00&endTime=2026-10-31T18:00:00%2B08:00&appType=claude&model=claude-opus-5-5&groupBy=day
```

URL 中的正号必须编码为 `%2B`，客户端也可使用标准 URL 查询参数编码器。上述示例除第一条外包含新增参数，当前旧版不能直接使用。

## 6. 响应契约

所有成功响应使用 `application/json`，带 `Cache-Control: no-store`。金额和用量均为 JSON number；无数据返回有效数字 0，不用 null 或字符串替代零。

### 6.1 字段

| 字段 | 类型 | 含义 |
|---|---|---|
| `scope` | string | 第一阶段固定 `local` |
| `appType` | string/null | 指定应用，未指定为 null |
| `apps` | string[] | 本次实际纳入的允许共享应用 |
| `model` | string/null | 指定模型，未指定为 null |
| `modelFilterBasis` | string | `effectivePricingModel` |
| `days` | integer | 查询触及的本地日期数量 |
| `startDate` / `endDate` | string | 查询触及的首末本地日期；不代替精确 range |
| `range.start` | string | RFC 3339，实际开始边界 |
| `range.endExclusive` | string | RFC 3339，实际结束边界，不包含 |
| `range.timezone` | string | `server-local` |
| `groupBy` | string | `day`、`week`、`month` |
| `requests` | integer | 区间请求总数 |
| `inputTokens` | integer | 新增输入，不含缓存 |
| `outputTokens` | integer | 输出 Token |
| `cacheReadTokens` | integer | 缓存读取 Token |
| `cacheWriteTokens` | integer | 缓存写入 Token |
| `totalTokens` | integer | 四类 Token 之和 |
| `costUsd` | number | 所选区间的已记录估算费用，USD |
| `currency` | string | 固定 `USD` |
| `inputTokenSemantics` | string | 固定 `fresh` |
| `updatedAt` | string | 本次响应生成时间，RFC 3339 |
| `lastScanAt` | integer/null | 最近扫描完成时间，Unix 毫秒；未扫描为 null |
| `buckets` | object[] | 按 groupBy 聚合的结果 |
| `daily` | object[] | 每日结果，兼容 v0.1.30，无论 groupBy 取何值都保留 |

顶层是所选区间总量；`buckets` 和 `daily` 是同一批数据的两种分组方式。三者各项加总必须一致。

### 6.2 完整示例

下面对应 `days=1&appType=claude&groupBy=day`，全部为模拟值：

```json
{
  "scope": "local",
  "appType": "claude",
  "apps": ["claude"],
  "model": null,
  "modelFilterBasis": "effectivePricingModel",
  "days": 1,
  "startDate": "2026-11-01",
  "endDate": "2026-11-01",
  "range": {
    "start": "2026-11-01T00:00:00+08:00",
    "endExclusive": "2026-11-02T00:00:00+08:00",
    "timezone": "server-local"
  },
  "groupBy": "day",
  "currency": "USD",
  "inputTokenSemantics": "fresh",
  "requests": 128,
  "inputTokens": 125000,
  "outputTokens": 32000,
  "cacheReadTokens": 800000,
  "cacheWriteTokens": 12000,
  "totalTokens": 969000,
  "costUsd": 2.36,
  "updatedAt": "2026-11-01T12:00:00+08:00",
  "lastScanAt": null,
  "buckets": [
    {
      "period": "2026-11-01",
      "start": "2026-11-01T00:00:00+08:00",
      "endExclusive": "2026-11-02T00:00:00+08:00",
      "requests": 128,
      "inputTokens": 125000,
      "outputTokens": 32000,
      "cacheReadTokens": 800000,
      "cacheWriteTokens": 12000,
      "totalTokens": 969000,
      "costUsd": 2.36
    }
  ],
  "daily": [
    {
      "date": "2026-11-01",
      "requests": 128,
      "inputTokens": 125000,
      "outputTokens": 32000,
      "cacheReadTokens": 800000,
      "cacheWriteTokens": 12000,
      "totalTokens": 969000,
      "costUsd": 2.36
    }
  ]
}
```

## 7. 天、周、月聚合

| groupBy | 自然周期 | period 格式 |
|---|---|---|
| day | 本地午夜至次日本地午夜 | `2026-11-01` |
| week | 周一午夜至下一周周一午夜 | ISO 周编号，如 `2026-W44` |
| month | 每月 1 日午夜至次月 1 日午夜 | `2026-11` |

- ISO 周编号使用 ISO 周所属年，不直接拼接日历年，避免跨年周编号错误。
- 只返回与查询范围相交的周期；不扩展顶层查询范围。
- bucket 的 `start`、`endExclusive` 为**自然周期与查询区间的交集**。`period` 仍标识其所属自然周期。
- 空周期补零，按开始时间升序；日期精度下 daily 长度等于 days。
- 秒级区间的每日结果也仅覆盖实际区间，例如 09:00–18:00 不包含当天其他时段。
- `groupBy=week/month` 只改变 buckets，不改变顶层合计和 daily。客户端不能把 daily 与 buckets 再相加。

例如查询 2026-10-01 至 10-31：第一周只统计 10-01 起的部分，最后一周只统计至 11-01 00:00:00，不会计入 9 月或 11 月数据。

## 8. 数据与费用口径

### 8.1 Token

归一化缓存口径后计算：

```text
totalTokens = inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens
```

同一个请求不能因输入计数已含缓存而重复计算缓存。模型筛选先应用于基础记录，再进行所有层级聚合。

### 8.2 费用

- 复用本机已保存的估算费用与既有去重逻辑，不在 GET 中重新按当前价格计价。
- 费用不是订阅余额、额度或账单结算金额。未计价记录可能贡献 Token 而费用为 0；不能把 0 解释为已确认免费。
- 内部使用十进制定点数求和，保留现有六位小数统计精度；各分组先以同一精度计算，顶层从同一数据集汇总。
- `costUsd` 输出为 JSON number，客户端自行格式化小数位；不能把格式化后的字符串再累加。

### 8.3 明细、归档与秒级精度

明细记录按 `start <= createdAt < endExclusive` 过滤；请求整体归入其记录时间所在周期，不按请求持续时间拆分。

历史只剩每日汇总时，只能完整计入或排除对应本地日，不能按小时比例分摊。若查询边界截断了某个具有匹配日汇总的日期，且无法证明仍保留完整有效明细，返回 **422 RANGE_PRECISION_UNAVAILABLE**，提示改选完整日期。

不能因为边界只覆盖半天就静默忽略该日汇总，也不能把旧明细与已归档日汇总重复计数。该检查需使用相同应用、模型和有效数据过滤条件；不匹配的历史归档不应阻止查询。

**归档时区约束**：同一数据目录应保持固定服务器时区。旧日汇总只有日期标签，缺少原始时区，默认沿用现有本地日期标签解释，不推断或保证跨时区重映射。后续归档应保存原始时区及日区间边界。若已知服务器时区发生变化，且匹配的历史日汇总无法按查询时区准确还原，即使请求覆盖完整日期也返回 422 `RANGE_PRECISION_UNAVAILABLE`；有完整明细时可按新时区重建。此情形是固定时区下旧请求兼容规则的明确例外，禁止静默把旧日汇总改认作新时区的一天。

### 8.4 一致性与新鲜度

- 同一响应中的总量、daily 和 buckets 必须基于同一数据库读快照及同一份应用允许列表，避免后台扫描写入时相互矛盾。
- GET 不扫描、不触发远端更新。`updatedAt` 不能被当作最近数据更新时间，客户端应同时查看 `lastScanAt`。
- 用户撤销密钥后，新请求立即失败；已有进行中的只读请求不承诺能够中途取消。
- 空数据返回 200、零值和补零周期。共享应用集合为空时，返回空 apps 与零统计。

## 9. 错误契约

保留字符串 `error`，新增稳定的 `code`，供程序判断：

```json
{
  "error": "查询区间截断了仅有日汇总的历史日期，请改用完整日期",
  "code": "RANGE_PRECISION_UNAVAILABLE",
  "details": {
    "dates": ["2026-10-01"]
  }
}
```

| HTTP 状态 | code 示例 | 场景 |
|---|---|---|
| 400 | INVALID_PARAMETERS | 未知、重复或格式错误的参数 |
| 400 | CONFLICTING_TIME_PARAMETERS | 同时提供冲突的时间模式 |
| 400 | INVALID_DATE_RANGE | 开始不早于结束、超过 366 天或超出允许的未来边界 |
| 400 | UNSUPPORTED_GROUP_BY | 非 day/week/month |
| 400 | UNSUPPORTED_APP_TYPE | 应用标识不受支持 |
| 400 | INVALID_LOCAL_BOUNDARY | 本地日历午夜边界不存在 |
| 401 | INVALID_API_KEY | 缺少有效只读密钥或配对凭证 |
| 403 | APP_NOT_SHARED | 应用未允许共享 |
| 422 | RANGE_PRECISION_UNAVAILABLE | 历史汇总无法支持所要求的秒级范围，或已知归档时区变化后无法准确重建 |
| 500 | USAGE_QUERY_FAILED | 统计查询失败 |

错误不得返回真实密钥、内部 SQL、对话内容或文件系统路径。身份校验在读取统计前完成；错误信息不用于泄露未授权应用的数据。

## 10. 兼容与实施顺序

1. 保持现有 `days/endDate/appType` 和顶层指标，新增参数解析和互斥校验。
2. 实现统一范围归一化与模型筛选，补齐归档精度检测。
3. 基于一致读快照产出 daily，再按本地自然周/月形成 buckets；秒级数据查询仍先过滤到实际范围。
4. 保留 daily 字段，无论 groupBy 为何都返回，兼容现有小组件；新增字段不能替换现有路径。
5. 更新文档、示例和界面调用说明，通过测试后再发布。

TinyPane 大号可读取 `/totalTokens`、`/costUsd`、`/requests`，趋势图使用 `/buckets`、相对标签路径 `/period` 和数值路径 `/totalTokens` 或 `/costUsd`。TinyPane 自身最多显示最后 30 个序列点，接口不会因此截断统计范围。

## 11. 验收清单

- [x] 原 v0.1.30 请求及顶层数字路径保持兼容。→ `v030_response_shape_is_backward_compatible`
- [x] 最近 N 天、指定日期区间和秒级区间都遵守左闭右开边界。→ `date_range_matches_equivalent_days_window`、`instant_range_is_left_closed_right_open`
- [x] 当天无数据、空共享集合、无匹配模型返回正确零值。→ `model_filter_uses_effective_pricing_model`、`app_scope_is_enforced`
- [x] Claude Code 与 Claude Desktop 精确区分；应用允许范围对所有分组生效。→ `claude_code_and_desktop_are_separate`
- [x] 正常日期、夏令时切换日、跨年 ISO 周、闰日和不同长度月份均正确。→ `iso_week_uses_the_iso_week_year`、`leap_day_and_variable_month_lengths_are_covered`、`local_midnight_resolution_rules`
- [x] 重复午夜取较早时刻，不存在的午夜返回约定错误。→ `local_midnight_resolution_rules`（确定性）、`local_midnight_rules_hold_under_other_timezones`（Unix 上按 `TZ` 切换时区实跑；Windows 上 `chrono::Local` 走系统 API、忽略 `TZ`，该路径由前者覆盖）
- [ ] 已知归档时区变更且无法重建时返回 422。→ **未实现**：归档目前只保存日期标签，没有原始时区元数据，无法判定服务器时区是否变更过。需要后续给归档增加时区字段（见 8.3）。
- [x] 首尾不完整周/月只统计实际查询部分，空周期补零并有稳定排序。→ `month_buckets_use_natural_months_and_zero_fill`
- [x] 各 bucket、daily 与顶层的请求、Token 和费用加总一致，重复查询不改变金额。→ `top_level_daily_and_buckets_agree`
- [x] 已归档数据不重复计数；无法准确拆分的历史边界返回 422，不返回低估或虚构结果。→ `partial_day_with_only_rollups_returns_422`、`partial_day_with_details_is_not_blocked`
- [x] 并发扫描写入期间，同一响应保持读快照一致性。→ `summary_stays_internally_consistent_while_the_database_is_written`
- [x] 缺少、错误或撤销的凭证，以及未共享应用、冲突时间参数、未知参数均得到约定错误。→ `http_auth_validation_revocation_and_read_only_key`、`conflicting_time_parameters_are_rejected`、`invalid_parameters_have_stable_codes`
- [x] 所有示例 JSON 可以解析。→ `documented_examples_are_valid_json`
- [ ] Windows 与 Apple Silicon macOS 执行相同的协议与日期测试。→ 协议用例跨平台通用；macOS 由发布流水线单独跑（`release.yml`）。本地仅在 Windows 上验证。

## 12. 实现记录

实现与本文的对应关系，以及实现时做的取舍：

1. **范围归一化**集中在 `public_api::resolve_range`，三种时间模式各自归一化为
   `[start, endExclusive)`。`days` 与 `startDate` 表达同一区间时结果完全一致。
2. **聚合查询**新增连接级函数 `usage_stats::daily_usage_rows` 与
   `usage_stats::rollup_dates`。两者复用页面同一条去重过滤、同一套缓存输入语义
   归一化和同一套日汇总边界规则。
3. **一致性快照**用 `Database::with_connection`：所有读写都走同一把互斥锁，
   持锁期间没有并发写入，因此一次响应里的总量、`daily` 与 `buckets` 必然来自
   同一份数据。注意闭包内不能再调用会自己加锁的 `Database` 方法——
   `std::sync::Mutex` 不可重入，会自死锁。
4. **顶层合计**由 `daily` 累加得到，不另跑一次 SQL 求和，所以金额不因取整
   路径不同而与 `buckets` 打架。
5. **模型筛选**直接复用页面口径 `COALESCE(NULLIF(pricing_model,''), model)`，
   明细与日汇总两条路径同时生效，聚合层无需改动。
6. **精度检测**只影响秒级区间：日期区间总是覆盖整天，不存在被切半的日子。
   被切半且只剩日汇总的日期返回 422，并在 `details.dates` 里列出。
7. **重复参数**由 `server.rs` 的 `duplicate_query_keys` 在解析前拒绝——
   serde 只会保留重复参数的最后一个值。

本文仅为接口设计；除第 2 节标明已支持的能力外，其他能力需后续开发，不能直接当作当前已上线接口使用。
