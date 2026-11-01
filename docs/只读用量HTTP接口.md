# 只读用量 HTTP 接口

## 开启与认证

在「AI 用量 → 管理设备」开启局域网共享，并在「只读用量 HTTP 接口」生成 API 密钥。新密钥仅生成时显示；重置或停用会立即使旧密钥失效。密钥可查询汇总，但不能配对设备、读取请求明细或触发扫描。

接口复用共享端口，默认 17891。本机地址为 `127.0.0.1`，其他设备请使用服务器局域网 IP。HTTP 为明文；只读 API 没有自动配置路由器或公网映射。

## 请求

```json
{
  "method": "GET",
  "url": "http://127.0.0.1:17891/api/v1/usage/summary?days=7&appType=claude",
  "headers": {"X-API-Key": "在管理设备中生成的只读密钥"}
}
```

### 参数

| 参数 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `days` | 整数 | 7 | 1–366 的整数，含结束日在内的日历天数 |
| `endDate` | 日期字符串 | 服务器今天 | `YYYY-MM-DD`，包含该日，不得晚于服务器今天 |
| `startDate` | 日期字符串 | 无 | 包含该日，必须不晚于 `endDate`；不能与 `days` 同时使用 |
| `startTime` | RFC 3339 | 无 | 秒级区间开始，包含；必须带 UTC 偏移或 `Z`，精确到整秒 |
| `endTime` | RFC 3339 | 无 | 秒级区间结束，**不包含**；必须与 `startTime` 成对提供 |
| `appType` | 字符串 | 无 | 精确应用标识；`claude` 仅 Claude Code，`claude-desktop` 单独查询 |
| `model` | 字符串 | 无 | 按有效计价模型 ID 精确筛选，口径与页面模型筛选一致 |
| `groupBy` | 枚举字符串 | `day` | `day`、`week`、`month` |

不填 `appType` 时合计所有允许共享的应用。指定未共享应用返回 403；未支持的应用标识返回 400。未知参数、重复参数、空筛选值、非法日期、不支持的分组方式返回 400。

### 三种时间选择方式

**最近 N 天**：`days` + 可选 `endDate`。都不填时默认最近 7 天、结束于服务器今天。

**指定日期区间**：`startDate` + 可选 `endDate`（省略时为服务器今天）。两端日期均包含，不能与 `days` 同时使用。

**指定秒级区间**：`startTime` 与 `endTime` 必须成对提供，且不能与 `days`、`startDate`、`endDate` 混用。必须满足 `startTime < endTime`，结束时刻最多为服务器明天的本地午夜。

三种模式统一归一化为 `[start, endExclusive)`：开始边界包含，结束边界不包含。所有日、周、月边界按**服务器本地时区**划分；请求携带的偏移只用于确定时刻，不改变分组时区。

### 分组

| `groupBy` | 自然周期 | `period` 格式 |
|---|---|---|
| `day` | 本地午夜至次日本地午夜 | `2026-11-01` |
| `week` | 周一午夜至下一周周一午夜 | ISO 周编号，如 `2026-W44` |
| `month` | 每月 1 日午夜至次月 1 日午夜 | `2026-11` |

ISO 周编号使用该周所属年份，跨年周不会拼错日历年。只返回与查询范围相交的周期；bucket 的 `start`、`endExclusive` 是自然周期与查询区间的交集。空周期补零，按开始时间升序。

## 返回

`daily` 始终返回（每日粒度），`buckets` 按 `groupBy` 分组。两者是同一批数据的两种分组方式，与顶层合计加总一致。

| 字段 | 类型 | 含义 |
|---|---|---|
| `scope` | string | 固定 `local`，只统计本机记录 |
| `appType` | string/null | 指定应用，未指定为 null |
| `apps` | string[] | 本次实际纳入的允许共享应用 |
| `model` | string/null | 指定模型，未指定为 null |
| `modelFilterBasis` | string | 固定 `effectivePricingModel`，即 `COALESCE(NULLIF(pricing_model,''), model)` |
| `days` | integer | 查询触及的本地日期数量 |
| `startDate` / `endDate` | string | 查询触及的首末本地日期 |
| `range.start` / `range.endExclusive` | string | RFC 3339，实际起止边界（结束不含），各自带 UTC 偏移 |
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
| `buckets` | object[] | 按 `groupBy` 聚合的结果 |
| `daily` | object[] | 每日结果，兼容早期组件 |

完整示例见 [`examples/usage-summary.example.json`](examples/usage-summary.example.json)。

- 金额和用量均为 JSON number；无数据返回有效的 0，不用 null 或字符串替代零。
- 费用复用已保存的估算费用，不在 GET 中重新按当前价格计价；它不是订阅余额或账单结算金额。
- `updatedAt` 是本次查询生成时间，不代表进行了新扫描；`lastScanAt` 才是最近数据时间。GET 只读，不主动扫描日志。
- 同一响应中的总量、`daily` 和 `buckets` 来自同一份数据库读快照，后台扫描写入时不会互相矛盾。

## 精度限制

历史只剩每日汇总时，只能整日计入或整日排除，无法按小时比例分摊。

如果秒级区间把某一天切成两半，而该天只剩日汇总，接口返回 **422 `RANGE_PRECISION_UNAVAILABLE`**，并在 `details.dates` 里列出受影响的日期，请改用完整日期。区间内的其他日期不受影响。

归档的日汇总只有日期标签、没有原始时区，默认按服务器本地日期标签解释，不做跨时区重映射。

## 错误

错误响应为 JSON，`error` 是可读说明，`code` 是稳定错误码，必要时带 `details`。

| HTTP 状态 | `code` | 场景 |
|---|---|---|
| 400 | `INVALID_PARAMETERS` | 未知、重复或格式错误的参数 |
| 400 | `CONFLICTING_TIME_PARAMETERS` | 同时提供冲突的时间模式，或 `startTime`/`endTime` 未成对 |
| 400 | `INVALID_DATE_RANGE` | 开始不早于结束、超过 366 天或超出允许的未来边界 |
| 400 | `UNSUPPORTED_GROUP_BY` | `groupBy` 不是 `day`/`week`/`month` |
| 400 | `UNSUPPORTED_APP_TYPE` | 应用标识不受支持 |
| 400 | `INVALID_LOCAL_BOUNDARY` | 本地日历午夜边界不存在 |
| 401 | `INVALID_API_KEY` | 缺少有效只读密钥或配对凭证 |
| 403 | `APP_NOT_SHARED` | 应用未允许共享 |
| 422 | `RANGE_PRECISION_UNAVAILABLE` | 历史汇总无法支持所要求的秒级范围 |
| 500 | `USAGE_QUERY_FAILED` | 统计查询失败 |

错误不会返回真实密钥、内部 SQL、对话内容或文件系统路径。成功响应带 `Cache-Control: no-store`。

已配对设备也可使用原 `x-observatory-device` 和 `x-observatory-token` 请求头调用本接口。

## TinyPane 大号

设置 HTTP 地址如上，刷新 60 秒，请求头填 `{"X-API-Key":"你的密钥"}`。将下面的数组粘贴到图表映射编辑框，保存后组件右键选择大号。

```json
[
  {"type":"metrics","title":"Claude Code 总 Token","valuePath":"/totalTokens","format":"compact","color":"#D97757"},
  {"type":"metrics","title":"估算费用","valuePath":"/costUsd","decimals":4,"unit":" USD","color":"#34B399"},
  {"type":"metrics","title":"请求次数","valuePath":"/requests","format":"compact","color":"#60A5FA"},
  {"type":"area","title":"每日 Token","itemsPath":"/daily","xPath":"/date","yPath":"/totalTokens","format":"compact","limit":7,"color":"#D97757"}
]
```

趋势图也可以改用 `/buckets`，相对标签路径 `/period`、数值路径 `/totalTokens` 或 `/costUsd`。TinyPane 最多显示最后 30 个序列点，本接口可以返回多于 30 天；需要更长趋势请在其他客户端查看。

图表标题对应 URL 中的 `appType=claude`；修改应用筛选时也应修改标题。
