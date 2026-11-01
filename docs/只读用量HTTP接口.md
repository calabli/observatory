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

| 参数 | 默认值 | 说明 |
|---|---|---|
| days | 7 | 1–366 的整数，含结束日在内的日历天数 |
| endDate | 服务器今天 | 可选 YYYY-MM-DD，不得晚于服务器今天 |
| appType | 全部允许共享的应用 | 精确应用标识；`claude` 仅 Claude Code，`claude-desktop` 单独查询 |

例如 `days=1&appType=claude` 查询今天 Claude Code；`days=7&endDate=2026-10-30&appType=claude` 查询服务器本地日期 10 月 24–30 日。

只统计本机保存的记录，不包含其他设备快照。应用必须在共享设置中勾选，指定未共享应用会返回 403；不指定应用时只合计允许共享的应用。未知参数会返回 400，不能用 `scope=multi` 获取多设备数据。

已配对设备也可使用原 `x-observatory-device` 和 `x-observatory-token` 请求头调用本接口；TinyPane 建议使用只读密钥。

## 返回示例

以下是模拟的单日数据，金额与 Token 均为 JSON number，不是实际账号用量。

```json
{
  "days": 1,
  "startDate": "2026-10-31",
  "endDate": "2026-10-31",
  "range": {
    "start": "2026-10-31T00:00:00+08:00",
    "endExclusive": "2026-11-01T00:00:00+08:00",
    "timezone": "server-local"
  },
  "scope": "local",
  "appType": "claude",
  "apps": ["claude"],
  "currency": "USD",
  "inputTokenSemantics": "fresh",
  "requests": 1,
  "inputTokens": 100,
  "outputTokens": 20,
  "cacheReadTokens": 300,
  "cacheWriteTokens": 10,
  "totalTokens": 430,
  "costUsd": 1.25,
  "updatedAt": "2026-10-31T12:00:00Z",
  "lastScanAt": null,
  "daily": [
    {"date":"2026-10-31","requests":1,"inputTokens":100,"outputTokens":20,"cacheReadTokens":300,"cacheWriteTokens":10,"totalTokens":430,"costUsd":1.25}
  ]
}
```

- `inputTokens` 是已归一化的不含缓存新增输入；总 Token 包含输入、输出、缓存读取和缓存写入。
- 费用复用已保存的估算费用，币种 USD，不是余额或订阅额度。内部保留六位小数统计精度；JSON 输出为数值，展示端自行格式化。
- `daily` 按日期升序，空日期补零，长度等于 `days`。单日查询仍返回日统计，不会返回小时桶。
- 使用服务器本地日历边界，`range` 两端携带各自 UTC 偏移；夏令时日期不固定为 24 小时。结束日包含整日的现有记录，包括已归档日汇总。
- `updatedAt` 是本次查询生成时间，不代表进行了新扫描；`lastScanAt` 是最近扫描完成的毫秒时间戳，未扫描为 null。GET 只读，不主动扫描日志。
- 汇总与每日数值在调用期间从现有数据库查询；发生并发写入时不是跨所有应用的事务快照。

## 错误

错误响应为 JSON，例如 `{"error":"days 必须为 1–366"}`。401 表示缺少/错误/过期的凭证；403 表示应用未允许共享；400 表示参数无效；500 表示统计查询失败。成功响应带 `Cache-Control: no-store`。

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

图表标题对应 URL 中的 `appType=claude`；修改应用筛选时也应修改标题。TinyPane 最多显示最后 30 个序列点，本接口可以返回多于 30 天；需要更长趋势请在其他客户端查看。
