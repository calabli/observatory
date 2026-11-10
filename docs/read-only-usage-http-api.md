# Read-only Usage HTTP API

## Enabling and Authentication

Enable LAN sharing under "AI Usage → Manage Devices" and generate an API key under "Read-only Usage HTTP API". A new key is shown only once, at creation time; resetting or revoking it immediately invalidates the old key. The key can query aggregates, but it cannot pair devices, read request details, or trigger scans.

The endpoint reuses the sharing port, 17891 by default. The local address is `127.0.0.1`; other devices should use the server's LAN IP. HTTP is plaintext; the read-only API does not configure the router or any public internet mapping for you.

## Postman Collection

The repository ships an importable collection and environment file:

- [`examples/postman-observatory-usage.collection.json`](examples/postman-observatory-usage.collection.json) — 38 requests in four groups ("Overview / Time ranges / Filters / Error scenarios"), with saved success and error response examples
- [`examples/postman-observatory-usage.environment.json`](examples/postman-observatory-usage.environment.json) — environment variables (`baseUrl`, `apiKey`, and a few date samples)

In Postman, choose **Import** and select both files, then replace `apiKey` in the environment with the key generated under "Manage Devices". When you access the API from another device, change `baseUrl` to the server's LAN IP.

The collection is bound to the implementation by a test: `postman_collection_requests_match_the_live_endpoint` in `native/tests/usage_http_api.rs` runs every request of the collection against a real server and checks the status codes, so a mistake in the collection turns the test red.

## Requests

Only two query endpoints are exposed, both under `/api/v1/usage/`:

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/usage/summary` | Query usage over a time range. The range is chosen by the caller |
| `GET /api/v1/usage/cycle` | Query the usage of the **current quota cycle** of an Agent platform. The server computes the range from the subscription rules |
| `GET /api/v1/usage/cycles` | Query the usage series of the **most recent cycles** of an Agent platform. Used by dashboards for trend bar charts |

```json
{
  "method": "GET",
  "url": "http://127.0.0.1:17891/api/v1/usage/summary?days=7&appType=claude",
  "headers": {"X-API-Key": "read-only key generated under Manage Devices"}
}
```

### Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `days` | Integer | 7 | An integer from 1–366: calendar days including the end date |
| `endDate` | Date string | Server's today | `YYYY-MM-DD`, inclusive of that day, must not be later than the server's today |
| `startDate` | Date string | None | Inclusive of that day, must not be later than `endDate`; cannot be used together with `days` |
| `startTime` | RFC 3339 | None | Start of the second-level range, inclusive; must carry a UTC offset or `Z`, with whole-second precision |
| `endTime` | RFC 3339 | Current request time | End of the second-level range, **exclusive**; `startTime` is still required when only `endTime` is given |
| `appType` | String | None | Several app identifiers separated by commas; `claude` is Claude Code only, `claude-desktop` is queried separately |
| `model` | String | None | Several effective pricing model IDs separated by commas, used as a filter set shared by the selected platforms |
| `modelsByApp` | JSON string | None | Arrays of model IDs per platform; `appType` must be provided as well, and it cannot be used together with `model` |
| `groupBy` | Enum string | `day` | `day`, `week`, `month` |
| `byModel` | Boolean | `false` | With `true` the response additionally returns `models`: a per-model breakdown of the same window; it cannot be used together with `allTime` |

When `appType` is omitted, all apps that are allowed to be shared are aggregated. Requesting an app that is not shared returns 403; an unsupported app identifier returns 400. Unknown parameters, duplicate parameters, empty filter values, invalid dates, and unsupported groupings return 400.

### Three Ways to Select a Time Range

**Last N days**: `days` + optional `endDate`. With neither set, the default is the last 7 days ending on the server's today.

**Explicit date range**: `startDate` + optional `endDate` (the server's today when omitted). Both end dates are inclusive, and it cannot be used together with `days`.

**Explicit second-level range**: `startTime` is required; when `endTime` is omitted the server's current time for this request is used (truncated to whole seconds), and it must not be mixed with `days`, `startDate`, or `endDate`. `startTime < endTime` must hold, and the end instant may be at most the server's local midnight of tomorrow.

All three modes are normalized to `[start, endExclusive)`: the start boundary is inclusive and the end boundary is exclusive. All day, week, and month boundaries are drawn in the **server's local time zone**; the offset carried by the request only determines the instant and does not change the grouping time zone.

When `endTime` is omitted or left empty, the server's current time is taken again on every request (truncated to whole seconds); clients no longer have to maintain an end time. The end instant that was actually used can be seen in `range.endExclusive`. For example, to keep querying from 19:00 on Thursday up to now:

```http
GET /api/v1/usage/summary?startTime=2026-10-29T19:00:00%2B08:00&appType=claude
```

### Grouping

| `groupBy` | Natural period | `period` format |
|---|---|---|
| `day` | From local midnight to the next local midnight | `2026-11-01` |
| `week` | From Monday midnight to the following Monday midnight | ISO week number, e.g. `2026-W44` |
| `month` | From the 1st of each month at midnight to the 1st of the next month at midnight | `2026-11` |

ISO week numbers use the year the week belongs to, so a week across the year boundary never gets the wrong calendar year. Only periods that intersect the queried range are returned; a bucket's `start` and `endExclusive` are the intersection of the natural period with the queried range. Empty periods are zero-filled and sorted by start time.

### Per-Model Breakdown (`byModel=true`)

With `byModel=true` the response gains one more array, `models`: the totals and the `daily` of every model over the same window, with exactly the same window and statistical basis as the top level (including the live detail records of the current day). Each entry has the same shape as a `byAppAndModel` entry of `/dashboard/usage`, sorted by `usage.totalTokens` in descending order:

```json
{"appType": "claude", "model": "claude-opus-5-5",
 "usage": {"totalTokens": 123456, "costUsd": 1.25, "requests": 42,
           "daily": [{"date": "2026-11-05", "totalTokens": 123456, "...": "…"}]}}
```

The `model` / `modelsByApp` / `modelPrefix` filters apply to `models` as well. They cannot be used together with `allTime` (returns 400). Use this parameter when you need a model breakdown such as "today / this week / this month" that is independent of the subscription billing period; the per-model data of `/dashboard/usage` is anchored to the subscription billing period of the first platform and excludes the current day on the subscription expiry date, so do not use it for natural-day figures.

## Current Cycle Usage

To find out how much a given Agent platform used "in this quota cycle", use this endpoint:

```http
GET /api/v1/usage/cycle?appType=claude
X-API-Key: read-only key generated under Manage Devices
```

**The response body is exactly the same as `/api/v1/usage/summary`** — the same fields, the same statistical basis, the same error structure. The only difference is that the server computes the range for you from the cycle rules of the subscription. The cycle boundaries can be read from `range.start` and `range.endExclusive`.

| Parameter | Type | Default | Description |
|---|---|---|---|
| `appType` | String | None | **Required**. Several Agent platforms separated by commas; the first platform selects the subscription cycle |
| `offset` | Integer | 0 | Cycle index: 0 the current cycle, -1 the previous one, 1 the next one |
| `model` | String | None | Several model IDs separated by commas; the filter applies uniformly across the selected platforms and the totals are returned |
| `modelsByApp` | JSON string | None | Arrays of model IDs per selected platform; it cannot be used together with `model` |
| `groupBy` | String | `day` | `day`, `week`, or `month`; only the grouping changes, not the cycle range or the totals |
| `devices` | String | `local` | Device IDs separated by commas, `local` meaning this machine |

For example `GET /api/v1/usage/cycle?appType=claude,codex&model=claude-opus-5-5,gpt-6.1-sol&groupBy=week`: the cycle is determined by the Claude Code subscription, the records of the two platforms that match the listed models are filtered in, and the totals plus the weekly grouping are returned. `model` is a filter set shared by all selected platforms; use `modelsByApp` when the models have to be specified per platform.

The value of `modelsByApp` is a JSON object (it must be URL-encoded in a URL):

```json
{"claude":["claude-opus-5-5"],"codex":["gpt-6.1-sol","gpt-6-sol"]}
```

Example:

```bash
curl -G 'http://127.0.0.1:17891/api/v1/usage/cycle' \
  -H 'X-API-Key: <read-only key>' \
  --data-urlencode 'appType=claude,codex' \
  --data-urlencode 'modelsByApp={"claude":["claude-opus-5-5"],"codex":["gpt-6.1-sol"]}' \
  --data-urlencode 'groupBy=week'
```

Every platform of the map must also appear in `appType`; when a selected platform is missing from the map, all of its models take part in the statistics. The model arrays must not be empty. The response is still the total of all matching records and is not split per model; when a map is used the response also contains `modelsByApp`, so that the filter conditions can be checked. The parameters above also apply to the `summary` endpoint.

The cycle rules come from the subscription record of the app under "AI Usage → Subscriptions & Quota" (weekday plus time of day) and are computed in the **server's local time zone**, matching the "Previous cycle / Next cycle" controls of the page exactly:

- The cycle start is inclusive and the cycle end is exclusive;
- The cycle is **clamped** to the validity period of the subscription: the start takes the later of "subscription start time" and "cycle start time", the end takes the earlier of "cycle end time" and "subscription expiry moment";
- Future usage does not exist yet, so the end is truncated further to the **moment of the request** — the current cycle therefore reports the usage of "this cycle up to now";
- When an app has several subscriptions, the one that is in effect today is used; if none is in effect, the first enabled one is used.

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

This endpoint has two more errors than `summary`:

| HTTP status | `code` | Scenario |
|---|---|---|
| 404 | `SUBSCRIPTION_NOT_FOUND` | The app has no subscription record, so the cycle cannot be computed |
| 404 | `CYCLE_NOT_AVAILABLE` | The cycle has not started yet, or it falls entirely outside the validity period of the subscription |

The remaining error codes match `summary`: 400 `INVALID_PARAMETERS` (unknown, duplicate, or missing parameters), 403 `APP_NOT_SHARED`, 422 `RANGE_PRECISION_UNAVAILABLE` (the cycle boundary cuts into a historical date that only has a daily rollup), 401 `INVALID_API_KEY`.

`daily` is always returned per calendar day; `buckets` aggregates by day, week, or month according to `groupBy`. A subscription cycle boundary may fall in the middle of a day, so historical daily rollups that cannot be split precisely still return 422.

To view "usage of the current cycle" on its own in TinyPane, simply switch the address to `/api/v1/usage/cycle?appType=claude` — the response body is the same, only the range switches to the current quota cycle:

```json
[
  {"type":"metrics","title":"Tokens This Cycle","valuePath":"/totalTokens","format":"compact","color":"#D97757"},
  {"type":"metrics","title":"Estimated Cost This Cycle","valuePath":"/costUsd","decimals":4,"unit":" USD","color":"#34B399"},
  {"type":"area","title":"Daily Tokens This Cycle","itemsPath":"/daily","xPath":"/date","yPath":"/totalTokens","format":"compact","color":"#D97757"}
]
```

## Cycle Series

When a dashboard needs a bar chart such as "usage over the last 8 weeks", use this endpoint:

```http
GET /api/v1/usage/cycles?appType=claude&count=8
X-API-Key: read-only key generated under Manage Devices
```

One call returns the current cycle and the preceding cycles, one set of totals per cycle.

| Parameter | Type | Default | Description |
|---|---|---|---|
| `appType` | String | None | **Required**. Several Agent platforms separated by commas; the first platform selects the subscription cycle |
| `count` | Integer | 8 | How many cycles to return (including the current one), 1–52 |
| `model` | String | None | Several model IDs separated by commas |
| `modelsByApp` | JSON string | None | Arrays of model IDs per platform |
| `devices` | String | `local` | The set of devices, `local` meaning this machine |

### How the Attribution Differs from the Single-Cycle Endpoint

- `/api/v1/usage/cycle` counts by **second-level range**, and both ends of the range may cut into a calendar day;
- `/api/v1/usage/cycles` counts by **calendar-day attribution** — every calendar day belongs as a whole to the cycle in which its midnight falls.

The reason for the split: when history only has daily rollups, a single day cannot be broken down by hour, so a day that gets cut can only be counted in full or excluded entirely (which is why the single-cycle endpoint returns 422). Calendar-day attribution is always computable, and the bars stay comparable with each other.

The trade-off is that a boundary day is attributed as a whole to **the cycle in which its midnight falls** (for example, if a cycle starts on Thursday at 19:00, the usage after 19:00 on that Thursday counts toward the previous cycle), so it can differ from the second-accurate single-cycle query by the usage of that boundary day. The response marks this with `granularity: "day"`, and every cycle gives both the precise window (`start`/`end`) and the dates actually attributed to it (`startDate`/`endDate`).

### Response

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

`index` is the cycle index: 0 is the current cycle and -1 is the previous one. `complete` says whether the cycle has already run to its end. `subscription` gives the cycle rules and the validity period, so a dashboard can show "resets every Thursday at 19:00".

## Response

`daily` is always returned (at daily granularity), and `buckets` is grouped by `groupBy`. The two are two groupings of the same data and add up to the same totals as the top level.

| Field | Type | Meaning |
|---|---|---|
| `scope` | string | `local` counts only the records of this machine; it becomes `multi` once paired devices are included |
| `devices` | string[] | The set of devices actually counted in this query; this machine is recorded as `local` |
| `appType` | string[]/null | The requested app list, null when not specified |
| `apps` | string[] | The apps allowed to be shared that were actually included in this query |
| `model` | string[]/null | The requested model list, null when not specified |
| `modelsByApp` | object/null | A per-platform model map; null when that filter is not used |
| `modelFilterBasis` | string | Always `effectivePricingModel`, i.e. `COALESCE(NULLIF(pricing_model,''), model)` |
| `days` | integer | The number of local dates the query touches |
| `startDate` / `endDate` | string | The first and last local dates the query touches |
| `range.start` / `range.endExclusive` | string | RFC 3339, the effective start and end boundaries (end exclusive), each with its own UTC offset |
| `range.timezone` | string | `server-local` |
| `groupBy` | string | `day`, `week`, `month` |
| `requests` | integer | The total number of requests in the range |
| `inputTokens` | integer | Fresh input tokens, excluding the cache |
| `outputTokens` | integer | Output tokens |
| `cacheReadTokens` | integer | Cache-read tokens |
| `cacheWriteTokens` | integer | Cache-write tokens |
| `totalTokens` | integer | The sum of the four token categories |
| `costUsd` | number | The recorded estimated cost of the selected range, in USD |
| `currency` | string | Always `USD` |
| `inputTokenSemantics` | string | Always `fresh` |
| `updatedAt` | string | The time this response was generated, RFC 3339 |
| `lastScanAt` | integer/null | The time the most recent scan finished, Unix milliseconds; null when nothing has been scanned yet |
| `buckets` | object[] | The results aggregated by `groupBy` |
| `daily` | object[] | Per-day results, kept compatible with earlier components |

See [`examples/usage-summary.example.json`](examples/usage-summary.example.json) for the complete example.

- Amounts and usage counts are JSON numbers; when there is no data, a valid numeric 0 is returned instead of null or a string standing in for zero.
- The cost reuses the saved estimated costs and is not priced again at the current rates during the GET; it is not a subscription balance or a billed settlement amount.
- `updatedAt` is the time this query was generated and does not mean that a new scan happened; `lastScanAt` is the time of the most recent data. A GET is read-only and never scans the logs on its own.
- The totals, `daily`, and `buckets` of one response come from the same database read snapshot, so a background scan writing concurrently cannot make them contradict each other.

## Precision Limits

When history only has daily rollups, a day can only be counted in full or excluded entirely; it cannot be prorated by hour.

If a second-level range cuts a day in half and that day only has a daily rollup left, the endpoint returns **422 `RANGE_PRECISION_UNAVAILABLE`** and lists the affected dates in `details.dates`; use whole dates instead. The other dates of the range are unaffected.

Archived daily rollups only carry a date label and no original time zone, so they are interpreted with the server's local date labels by default and are never remapped across time zones.

## Errors

Error responses are JSON: `error` is a human-readable explanation, `code` is a stable error code, and `details` is included when needed.

| HTTP status | `code` | Scenario |
|---|---|---|
| 400 | `INVALID_PARAMETERS` | Unknown, duplicate, or malformed parameters |
| 400 | `CONFLICTING_TIME_PARAMETERS` | Conflicting time modes supplied at the same time, or only `endTime` without `startTime` |
| 400 | `INVALID_DATE_RANGE` | The start is not earlier than the end, more than 366 days, or beyond the allowed future boundary |
| 400 | `UNSUPPORTED_GROUP_BY` | `groupBy` is not `day`/`week`/`month` |
| 400 | `UNSUPPORTED_APP_TYPE` | The app identifier is not supported |
| 400 | `INVALID_LOCAL_BOUNDARY` | The local calendar midnight boundary does not exist |
| 401 | `INVALID_API_KEY` | Missing valid read-only key or paired-device credential |
| 403 | `APP_NOT_SHARED` | The app is not allowed to be shared |
| 403 | `DEVICE_SCOPE_FORBIDDEN` | A paired-device credential queried a device other than `local` |
| 404 | `SUBSCRIPTION_NOT_FOUND` | Cycle endpoints: the app has no subscription record |
| 404 | `CYCLE_NOT_AVAILABLE` | Cycle endpoints: the cycle has not started yet or is not inside the validity period of the subscription |
| 422 | `RANGE_PRECISION_UNAVAILABLE` | Historical rollups cannot support the requested second-level range |
| 500 | `USAGE_QUERY_FAILED` | The statistics query failed |

Errors never return the real key, internal SQL, conversation content, or file system paths. Successful responses carry `Cache-Control: no-store`.

Paired devices can also call this endpoint with the original `x-observatory-device` and `x-observatory-token` headers, but they can only query their own usage: `devices` must be omitted or `local`, otherwise the endpoint returns 403 `DEVICE_SCOPE_FORBIDDEN`. Data that this machine synced from other devices is not passed on to paired devices, which matches the rule that LAN sync does not relay remote copies. `devices=all` and explicit device IDs are reserved for the read-only API key.

## TinyPane Large Tile

Set the HTTP address as above, a 60-second refresh, and put `{"X-API-Key":"your key"}` in the request headers. Paste the array below into the chart-mapping editor and, after saving, choose the large tile from the context menu of the component.

```json
[
  {"type":"metrics","title":"Claude Code Total Tokens","valuePath":"/totalTokens","format":"compact","color":"#D97757"},
  {"type":"metrics","title":"Estimated Cost","valuePath":"/costUsd","decimals":4,"unit":" USD","color":"#34B399"},
  {"type":"metrics","title":"Request Count","valuePath":"/requests","format":"compact","color":"#60A5FA"},
  {"type":"area","title":"Daily Tokens","itemsPath":"/daily","xPath":"/date","yPath":"/totalTokens","format":"compact","limit":7,"color":"#D97757"}
]
```

The trend chart can also use `/buckets`, with the relative label path `/period` and the numeric path `/totalTokens` or `/costUsd`. TinyPane shows at most the last 30 series points while this endpoint can return more than 30 days; view longer trends in another client.

The chart title corresponds to `appType=claude` in the URL; change the title as well when you change the app filter.

## Dashboard: Cross-Platform Aggregation by Model

`POST /api/v1/dashboard/models`, with the read-only key in the `X-API-Key` header. Example:

```json
{"platformCode":"codex","modelIds":["gpt-6.1-sol","gpt-6.1-terra"],"billingOffset":0,"quotaOffset":0,"devices":"local"}
```

`platformCode` determines the subscription billing period and the weekly quota rules, and it must be allowed to be shared. When a non-empty `modelIds` is given, the records of every shareable platform that match any of the model IDs take part in the total; without it, or with an empty array, only all models of that platform are queried, which matches the dashboard usage endpoint. Model names are effective pricing model IDs and are matched exactly; the array holds at most 50 entries and must not contain empty IDs or IDs with commas. The other parameters mean the same as in the dashboard usage endpoint.

It returns the dashboard usage fields such as `billingTotal`, `quotaCycles`, `selectedQuotaCycle`, and `billingAggregations`, plus `modelIds`. The read operation reuses the dashboard endpoint; if a scan happens during the multiple database reads, the result may not be a snapshot of a single moment. When historical data cannot be split precisely it returns 422.

### Model Prefix Filter

`POST /api/v1/dashboard/usage` and `POST /api/v1/dashboard/models` support an optional `modelPrefix` that matches the beginning of the effective pricing model ID, case-insensitively for ASCII letters. For example, to query only the MiniMax models of the mcode platform:

```json
{"appType":"mcode","modelPrefix":"minimax","billingOffset":0,"quotaOffset":0,"devices":"local"}
```

`/dashboard/usage` only queries the platforms listed in `appType`. `/dashboard/models` uses the subscription of `platformCode` and looks for prefix-matching models across all platforms that are allowed to be shared:

```json
{"platformCode":"mcode","modelPrefix":"minimax"}
```

Without `modelPrefix` the existing exact matching and the behavior of passing no model stay unchanged. The prefix must not be provided together with `model`, `modelsByApp`, or a non-empty `modelIds`; a blank prefix returns 400 and no matches return zero usage. The query performs multiple database reads, so a concurrent scan may make the result not a snapshot of a single moment.

### Querying All Devices

`devices` accepts `all` (case-insensitive), meaning this machine plus every currently paired and enabled device. It applies to `summary`, `cycle`, `cycles`, and both dashboard endpoints. For example:

```json
{"appType":"mcode","modelPrefix":"minimax","devices":"all"}
```

Omitting `devices` still queries only this machine; `all` must be used on its own and cannot be mixed with `local` or device IDs. Remote devices use the most recent snapshot that was synced to this machine, and a device whose sync failed is not guaranteed to have the latest data. The `devices` field of the aggregate endpoint lists the device IDs that actually took part.

### Combining Prefixes with Exact Models

`POST /api/v1/dashboard/models` can take `modelIds` (an array of exact model IDs) and `modelPrefixes` (an array of prefixes) at the same time. A match on either condition is enough, and records matched by both are counted only once; prefixes are matched case-insensitively for ASCII letters, while exact IDs stay case-sensitive. Passing only `modelPrefixes` also works, and when both arrays are empty or omitted all models of `platformCode` are still queried.

```json
{"platformCode":"mcode","modelIds":["exact-model-id"],"modelPrefixes":["minimax","deepseek"],"devices":"all"}
```

Each array holds at most 50 entries; a prefix must not be an empty string. The original singular `modelPrefix` can still be used on its own and, to stay compatible with the old behavior, must not be passed together with a non-empty `modelIds` or `modelPrefixes`. The returned `modelIds` and `modelPrefixes` reflect the deduplicated inputs; when no model matches at all the usage is zero.

### Regular Summary: Model Prefix and All History

`GET /api/v1/usage/summary?allTime=true&modelPrefix=deepseek&devices=all` queries the entire history of the effective pricing models starting with `deepseek` on this machine and on every paired and enabled device, across the platforms that are allowed to be shared; it does not depend on any subscription. The prefix match ignores ASCII letter case. With `allTime=true` the statistics run from the local day of the earliest record of the selected devices and the queryable platforms through the end of the server's current local day, without the 366-day limit. When no model matches it returns zero usage.

`allTime=true` must not be mixed with `days`, `startDate`, `endDate`, `startTime`, or `endTime`; `modelPrefix` must not be mixed with `model` or `modelsByApp`. When neither is given, the existing defaults stay in place: the last 7 days and the existing exact model filtering. The `allTime` and `modelPrefix` fields of the response state the conditions the request used. Remote data is the most recent snapshot that was synced to this machine; the `daily` array of the full history can be large.
