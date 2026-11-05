# 只读用量 HTTP 接口

## 开启与认证

在「AI 用量 → 管理设备」开启局域网共享，并在「只读用量 HTTP 接口」生成 API 密钥。新密钥仅生成时显示；重置或停用会立即使旧密钥失效。密钥可查询汇总，但不能配对设备、读取请求明细或触发扫描。

接口复用共享端口，默认 17891。本机地址为 `127.0.0.1`，其他设备请使用服务器局域网 IP。HTTP 为明文；只读 API 没有自动配置路由器或公网映射。

## Postman 集合

仓库里带了一份可直接导入的集合和环境文件：

- [`examples/postman-observatory-usage.collection.json`](examples/postman-observatory-usage.collection.json) —— 38 条请求，分「概览 / 时间区间 / 筛选 / 错误场景」四组，带保存好的成功与错误响应示例
- [`examples/postman-observatory-usage.environment.json`](examples/postman-observatory-usage.environment.json) —— 环境变量（`baseUrl`、`apiKey` 与几个日期样例）

在 Postman 里 **Import** 选这两个文件，然后把环境里的 `apiKey` 换成「管理设备」生成的密钥即可。从其他设备访问时把 `baseUrl` 改成服务器局域网 IP。

集合与实现由测试绑定：`native/tests/usage_http_api.rs` 的 `postman_collection_requests_match_the_live_endpoint` 会把集合里的每条请求对着真实服务跑一遍并核对状态码，集合写错了测试就会红。

## 请求

对外只暴露两个查询端点，都在 `/api/v1/usage/` 下：

| 端点 | 用途 |
|---|---|
| `GET /api/v1/usage/summary` | 按时间区间查用量。区间由调用方指定 |
| `GET /api/v1/usage/cycle` | 按 Agent 平台查**当前额度周期**内的用量。区间由服务端按订阅规则算好 |
| `GET /api/v1/usage/cycles` | 按 Agent 平台查**最近若干个周期**的用量序列。看板画趋势柱状图用 |

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
| `endTime` | RFC 3339 | 请求当前时间 | 秒级区间结束，**不包含**；只提供 endTime 时仍必须提供 startTime |
| `appType` | 字符串 | 无 | 多个应用标识以逗号分隔；`claude` 仅 Claude Code，`claude-desktop` 单独查询 |
| `model` | 字符串 | 无 | 多个有效计价模型 ID 以逗号分隔，作为所选平台共用的筛选集合 |
| `modelsByApp` | JSON 字符串 | 无 | 按平台分别指定模型 ID 数组，须同时提供 `appType`，不能与 `model` 同时使用 |
| `groupBy` | 枚举字符串 | `day` | `day`、`week`、`month` |
| `byModel` | 布尔 | `false` | `true` 时响应额外返回 `models`：同一窗口的按模型拆分，不能与 `allTime` 同时使用 |

不填 `appType` 时合计所有允许共享的应用。指定未共享应用返回 403；未支持的应用标识返回 400。未知参数、重复参数、空筛选值、非法日期、不支持的分组方式返回 400。

### 三种时间选择方式

**最近 N 天**：`days` + 可选 `endDate`。都不填时默认最近 7 天、结束于服务器今天。

**指定日期区间**：`startDate` + 可选 `endDate`（省略时为服务器今天）。两端日期均包含，不能与 `days` 同时使用。

**指定秒级区间**：`startTime` 必填，`endTime` 省略时取本次请求的服务器当前时间（截到整秒），且不能与 `days`、`startDate`、`endDate` 混用。必须满足 `startTime < endTime`，结束时刻最多为服务器明天的本地午夜。

三种模式统一归一化为 `[start, endExclusive)`：开始边界包含，结束边界不包含。所有日、周、月边界按**服务器本地时区**划分；请求携带的偏移只用于确定时刻，不改变分组时区。

`endTime` 省略或留空时，每次请求都重新取服务器当前时间（截到整秒）；不再需要客户端维护结束时间。实际采用的结束时刻可从 `range.endExclusive` 查看。例如从周四 19:00 持续查询到现在：

```http
GET /api/v1/usage/summary?startTime=2026-10-29T19:00:00%2B08:00&appType=claude
```

### 分组

| `groupBy` | 自然周期 | `period` 格式 |
|---|---|---|
| `day` | 本地午夜至次日本地午夜 | `2026-11-01` |
| `week` | 周一午夜至下一周周一午夜 | ISO 周编号，如 `2026-W44` |
| `month` | 每月 1 日午夜至次月 1 日午夜 | `2026-11` |

ISO 周编号使用该周所属年份，跨年周不会拼错日历年。只返回与查询范围相交的周期；bucket 的 `start`、`endExclusive` 是自然周期与查询区间的交集。空周期补零，按开始时间升序。

### 按模型拆分（`byModel=true`）

带上 `byModel=true` 时，响应额外多一个 `models` 数组：同一窗口内每个模型的合计和 `daily`，窗口、统计口径与顶层完全一致（含当天的实时明细）。每项的形状与 `/dashboard/usage` 的 `byAppAndModel` 条目相同，按 `usage.totalTokens` 降序：

```json
{"appType": "claude", "model": "claude-opus-5-5",
 "usage": {"totalTokens": 123456, "costUsd": 1.25, "requests": 42,
           "daily": [{"date": "2026-11-05", "totalTokens": 123456, "...": "…"}]}}
```

`model` / `modelsByApp` / `modelPrefix` 的筛选对 `models` 同样生效。不能与 `allTime` 同时使用（返回 400）。需要「今日 / 本周 / 本月」这类与订阅账期无关的模型构成时用这个参数；`/dashboard/usage` 的按模型数据锚定在第一个平台的订阅账期上，订阅到期日当天不包含当天数据，不要用它做自然日口径。

## 当前周期用量

想知道某个 Agent 平台「这个额度周期用了多少」，用这个端点：

```http
GET /api/v1/usage/cycle?appType=claude
X-API-Key: 在管理设备中生成的只读密钥
```

**返回体与 `/api/v1/usage/summary` 完全一致**——同样的字段、同样的统计口径、同样的错误结构。区别只是区间由服务端按订阅的周期规则替你算好了。周期边界可以从 `range.start` 与 `range.endExclusive` 读出。

| 参数 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `appType` | 字符串 | 无 | **必填**。多个 Agent 平台用逗号分隔，第一个平台用于选择订阅周期 |
| `offset` | 整数 | 0 | 周期序号：0 当前周期，-1 上一个，1 下一个 |
| `model` | 字符串 | 无 | 多个模型 ID 用逗号分隔，对所选平台统一筛选后返回合计 |
| `modelsByApp` | JSON 字符串 | 无 | 按所选平台分别指定模型 ID 数组，不能与 `model` 同时使用 |
| `groupBy` | 字符串 | `day` | `day`、`week` 或 `month`；只改变分组，不改变周期范围及总量 |
| `devices` | 字符串 | `local` | 设备 ID 以逗号分隔，`local` 表示本机 |

例如 `GET /api/v1/usage/cycle?appType=claude,codex&model=claude-opus-5-5,gpt-6.1-sol&groupBy=week`：以 Claude Code 的订阅确定周期，筛选两个平台中匹配所列模型的记录，返回合计和按周分组。`model` 是所有所选平台共用的筛选集合；需要分别指定平台模型时使用 `modelsByApp`。

`modelsByApp` 的值是 JSON 对象（URL 中需进行编码）：

```json
{"claude":["claude-opus-5-5"],"codex":["gpt-6.1-sol","gpt-6-sol"]}
```

示例：

```bash
curl -G 'http://127.0.0.1:17891/api/v1/usage/cycle' \
  -H 'X-API-Key: <只读密钥>' \
  --data-urlencode 'appType=claude,codex' \
  --data-urlencode 'modelsByApp={"claude":["claude-opus-5-5"],"codex":["gpt-6.1-sol"]}' \
  --data-urlencode 'groupBy=week'
```

映射中的平台必须出现在 `appType`；已选平台未出现在映射中时，该平台的全部模型参与统计。模型数组不能为空。返回仍是所有匹配记录的合计，不按模型拆行；使用映射时响应会包含 `modelsByApp`，便于核对筛选条件。以上参数也适用于 `summary` 端点。

周期规则来自「AI 用量 → 订阅与额度」里该应用的订阅记录（星期几 + 时分秒），按**服务器本地时区**计算，与页面上的「上一周期 / 下一周期」完全对齐：

- 周期开始包含，结束不包含；
- 周期按订阅有效期**收敛**：开头取「订阅开始时间」与「周期开始时间」的较晚者，结尾取「周期结束时间」与「订阅到期时刻」的较早者；
- 未来的用量还不存在，所以结尾再截到**请求时刻**——当前周期查到的就是「本周期截至此刻」的用量；
- 同一个应用有多条订阅时，取今天生效的那条；没有生效的就取第一条启用的。

```json
{
  "requests": 128,
  "inputTokens": 2068,
  "outputTokens": 1367239,
  "cacheReadTokens": 429848775,
  "cacheWriteTokens": 3755596,
  "totalTokens": 434973678,
  "costUsd": 143.367575,
  "scope": "local",
  "appType": ["claude"],
  "apps": ["claude"],
  "model": null,
  "modelFilterBasis": "effectivePricingModel",
  "days": 4,
  "startDate": "2026-10-29",
  "endDate": "2026-11-02",
  "range": {
    "start": "2026-10-29T19:00:00+08:00",
    "endExclusive": "2026-11-02T05:12:00+08:00",
    "timezone": "server-local"
  },
  "groupBy": "day",
  "currency": "USD",
  "inputTokenSemantics": "fresh",
  "updatedAt": "2026-11-02T04:56:12.881734500+00:00",
  "lastScanAt": 1793561400000,
  "buckets": [],
  "daily": []
}
```

这个端点比 `summary` 多两种错误：

| HTTP 状态 | `code` | 场景 |
|---|---|---|
| 404 | `SUBSCRIPTION_NOT_FOUND` | 该应用没有订阅记录，算不出周期 |
| 404 | `CYCLE_NOT_AVAILABLE` | 该周期尚未开始，或整个落在订阅有效期之外 |

其余错误码与 `summary` 一致：400 `INVALID_PARAMETERS`（未知、重复或缺失参数）、403 `APP_NOT_SHARED`、422 `RANGE_PRECISION_UNAVAILABLE`（周期边界切到了只剩日汇总的历史日期）、401 `INVALID_API_KEY`。

`daily` 始终按自然日返回；`buckets` 根据 `groupBy` 按天、周或月聚合。订阅周期边界可能落在半天上，不能精确拆分的历史日汇总仍会返回 422。

想在 TinyPane 里单独看「本周期用量」，把地址换成 `/api/v1/usage/cycle?appType=claude` 即可——返回体一样，只是区间换成了当前额度周期：

```json
[
  {"type":"metrics","title":"本周期 Token","valuePath":"/totalTokens","format":"compact","color":"#D97757"},
  {"type":"metrics","title":"本周期估算费用","valuePath":"/costUsd","decimals":4,"unit":" USD","color":"#34B399"},
  {"type":"area","title":"本周期每日 Token","itemsPath":"/daily","xPath":"/date","yPath":"/totalTokens","format":"compact","color":"#D97757"}
]
```

## 周期序列

看板要画「近 8 周用量」这种柱状图时，用这个端点：

```http
GET /api/v1/usage/cycles?appType=claude&count=8
X-API-Key: 在管理设备中生成的只读密钥
```

一次返回当前周期与前若干个周期，每个周期一份合计。

| 参数 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `appType` | 字符串 | 无 | **必填**。多个 Agent 平台用逗号分隔，第一个平台用于选择订阅周期 |
| `count` | 整数 | 8 | 返回多少个周期（含当前），1–52 |
| `model` | 字符串 | 无 | 多个模型 ID 用逗号分隔 |
| `modelsByApp` | JSON 字符串 | 无 | 按平台分别指定模型 ID 数组 |
| `devices` | 字符串 | `local` | 设备集合，`local` 表示本机 |

### 归集口径与单周期端点的区别

- `/api/v1/usage/cycle` 按**秒级区间**统计，区间两端会切开某个自然日；
- `/api/v1/usage/cycles` 按**自然日归属**统计——每个自然日整体归入「午夜落在哪个周期」的那个周期。

之所以分开：历史只剩每日汇总时无法把某一天按小时拆开，切开的那天只能整日计入或整日排除（单周期端点会因此返回 422）。自然日归属永远可算，柱子之间口径也一致。

代价是边界日会整体归入**按午夜落点所在的周期**（例如周期从周四 19:00 开始，周四当天整点之后的用量算在上一个周期里），因此与精确到秒的单周期查询可能相差边界当日的部分用量。响应用 `granularity: "day"` 标明这一点，每个周期同时给出精确窗口（`start`/`end`）与实际归属的日期（`startDate`/`endDate`）。

### 返回

```json
{
  "scope": "local",
  "devices": ["local"],
  "appType": ["claude"],
  "apps": ["claude"],
  "model": null,
  "modelsByApp": null,
  "granularity": "day",
  "subscription": {
    "name": "Claude Max",
    "rule": {"startDay": 4, "startTime": "19:00:00", "endDay": 4, "endTime": "19:00:00"},
    "startDate": "2025-11-02",
    "endDate": "2027-11-02"
  },
  "cycles": [
    {
      "requests": 4,
      "inputTokens": 400,
      "outputTokens": 80,
      "cacheReadTokens": 0,
      "cacheWriteTokens": 0,
      "totalTokens": 480,
      "costUsd": 4.0,
      "index": 0,
      "start": "2026-10-29T19:00:00+08:00",
      "end": "2026-11-05T19:00:00+08:00",
      "startDate": "2026-10-30",
      "endDate": "2026-11-05",
      "complete": false
    }
  ],
  "updatedAt": "2026-11-02T09:32:29.824459800+08:00",
  "lastScanAt": null
}
```

`index` 是周期序号：0 是当前周期，-1 是上一个。`complete` 表示该周期是否已经走完。`subscription` 给出周期规则与有效期，看板可以据此显示「每周四 19:00 重置」。

## 返回

`daily` 始终返回（每日粒度），`buckets` 按 `groupBy` 分组。两者是同一批数据的两种分组方式，与顶层合计加总一致。

| 字段 | 类型 | 含义 |
|---|---|---|
| `scope` | string | `local` 只统计本机记录；并入配对设备后为 `multi` |
| `devices` | string[] | 本次实际统计的设备集合，本机记作 `local` |
| `appType` | string[]/null | 指定应用列表，未指定为 null |
| `apps` | string[] | 本次实际纳入的允许共享应用 |
| `model` | string[]/null | 指定模型列表，未指定为 null |
| `modelsByApp` | object/null | 按平台分别指定的模型映射；未用该筛选时为 null |
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
| 400 | `CONFLICTING_TIME_PARAMETERS` | 同时提供冲突的时间模式，或只提供 `endTime` 而未提供 `startTime` |
| 400 | `INVALID_DATE_RANGE` | 开始不早于结束、超过 366 天或超出允许的未来边界 |
| 400 | `UNSUPPORTED_GROUP_BY` | `groupBy` 不是 `day`/`week`/`month` |
| 400 | `UNSUPPORTED_APP_TYPE` | 应用标识不受支持 |
| 400 | `INVALID_LOCAL_BOUNDARY` | 本地日历午夜边界不存在 |
| 401 | `INVALID_API_KEY` | 缺少有效只读密钥或配对凭证 |
| 403 | `APP_NOT_SHARED` | 应用未允许共享 |
| 404 | `SUBSCRIPTION_NOT_FOUND` | 周期端点：该应用没有订阅记录 |
| 404 | `CYCLE_NOT_AVAILABLE` | 周期端点：该周期尚未开始或不在订阅有效期内 |
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

## 看板：跨平台按模型汇总

`POST /api/v1/dashboard/models`，请求头 `X-API-Key` 为只读密钥。示例：

```json
{"platformCode":"codex","modelIds":["gpt-6.1-sol","gpt-6.1-terra"],"billingOffset":0,"quotaOffset":0,"devices":"local"}
```

`platformCode` 决定订阅账期与周额度规则，且必须允许共享。提供非空 `modelIds` 时，所有允许共享的平台中匹配任一模型 ID 的记录参与合计；不提供或提供空数组时，只查询该平台全部模型，与看板 usage 接口一致。模型名称是有效计价模型 ID，精确匹配；数组最多 50 项，不能传空 ID 或包含逗号的 ID。其他参数含义与看板 usage 相同。

返回看板 usage 的 `billingTotal`、`quotaCycles`、`selectedQuotaCycle`、`billingAggregations` 等字段，并附带 `modelIds`。读取操作复用看板接口，跨多次数据库查询期间发生扫描时，结果可能不是同一时刻的快照。历史数据无法精确切分时返回 422。

### 模型前缀筛选

`POST /api/v1/dashboard/usage` 和 `POST /api/v1/dashboard/models` 支持可选 `modelPrefix`，按有效计价模型 ID 的开头匹配，英文字母不区分大小写。例如只查询 mcode 平台的 MiniMax 模型：

```json
{"appType":"mcode","modelPrefix":"minimax","billingOffset":0,"quotaOffset":0,"devices":"local"}
```

`/dashboard/usage` 只查 `appType` 列出的平台。`/dashboard/models` 使用 `platformCode` 的订阅，并在所有允许共享的平台查找前缀匹配的模型：

```json
{"platformCode":"mcode","modelPrefix":"minimax"}
```

未提供 `modelPrefix` 时原有精确匹配和不传模型的行为不变。前缀不能与 `model`、`modelsByApp` 或非空 `modelIds` 同时提供；空白前缀返回 400，无匹配结果返回零用量。查询涉及多次数据库读取，并发扫描时结果可能不是同一时刻的快照。

### 查询全部设备

`devices` 支持 `all`（不区分大小写），表示本机和当前已配对、已启用的所有设备。适用于 `summary`、`cycle`、`cycles` 及两个看板接口。例如：

```json
{"appType":"mcode","modelPrefix":"minimax","devices":"all"}
```

不填 `devices` 仍只查询本机；`all` 必须单独使用，不能与 `local` 或设备 ID 混用。远端使用最近同步到本机的快照，未同步成功的设备不保证具有最新数据。汇总接口的 `devices` 字段列出实际参与的设备 ID。

### 多个前缀与精确模型混合

`POST /api/v1/dashboard/models` 可同时提供 `modelIds`（精确模型 ID 数组）和 `modelPrefixes`（前缀数组）。任一条件匹配即可，重复命中只统计一次；前缀的英文字母不区分大小写，精确 ID 继续区分大小写。只传 `modelPrefixes` 也可，两个数组都为空或未传时仍只查 `platformCode` 的所有模型。

```json
{"platformCode":"mcode","modelIds":["exact-model-id"],"modelPrefixes":["minimax","deepseek"],"devices":"all"}
```

两个数组各最多 50 项；前缀不能是空字符串。原来的单数 `modelPrefix` 仍可单独使用，为兼容旧行为，不能与非空 `modelIds` 或 `modelPrefixes` 同时传入。返回的 `modelIds` 和 `modelPrefixes` 分别反映去重后的输入；无任何模型匹配时用量为零。

### 普通汇总：模型前缀与全部历史

`GET /api/v1/usage/summary?allTime=true&modelPrefix=deepseek&devices=all` 查询本机及所有已配对、已启用设备上允许共享平台中，以 `deepseek` 开头的有效计价模型的全部历史；不依赖订阅。前缀匹配忽略 ASCII 字母大小写。`allTime=true` 从所选设备和允许查询的平台最早记录所在本地日起，统计到服务端当前本地日的结束，不限制 366 天。无匹配模型返回零用量。

`allTime=true` 不能与 `days`、`startDate`、`endDate`、`startTime`、`endTime` 混用；`modelPrefix` 不能与 `model` 或 `modelsByApp` 混用。两项不传时维持原有默认查询最近 7 天和原有精确模型筛选。响应中的 `allTime` 和 `modelPrefix` 表示请求采用的条件。远端数据为最近一次成功同步到本机的快照；完整历史的 `daily` 数组可能较大。
