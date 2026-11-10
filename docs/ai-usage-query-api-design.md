# AI Usage Query API Design

Version: design draft v1.0  
Date: 2026-11-01  
Status: implemented (awaiting review)  
Applies to: Observatory

> Implementation note: the extended protocol described in this document has landed in `native/src/lan_usage/public_api.rs`,
> `native/src/lan_usage/server.rs`, and `native/src/services/usage_stats.rs`;
> the acceptance tests live in `native/tests/usage_http_api.rs`.
> For the user-facing description see [Read-only Usage HTTP API](./read-only-usage-http-api.md).

## 1. Goals

Provide read-only statistics queries for TinyPane, automation scripts, and other HTTP clients:

- Query token usage, estimated cost, and request counts over a given time range.
- Return the total usage and total cost of the whole queried range, plus data grouped by day, week, or month.
- Filter by app and model; Claude Code uses `appType=claude` and never mixes in Claude Desktop.
- Serve both the number cards and the trend chart from the same response, so clients never have to filter detail records or compute groupings themselves.

In this design every "total" means the **aggregate over the selected time range**, not the full account history or the subscription quota. For example, when querying one year grouped by month, the top level is the aggregate for that year and each bucket holds the statistics of its own month. No separate all-time cumulative values are returned.

The first phase queries only the statistics stored on this machine and does not aggregate snapshots from other devices. Multi-device mode needs separate definitions for the per-device time zone, snapshot freshness, and missing state; it is not part of this protocol.

## 2. Current and New Capabilities

See [Read-only Usage HTTP API](./read-only-usage-http-api.md) for how to use the current v0.1.30. This document describes the extended target protocol; the entries in the "This design" column below are already implemented.

| Capability | v0.1.30 | This design (implemented) |
|---|---|---|
| GET `/api/v1/usage/summary` | Supported | Kept |
| `days`, `endDate`, `appType` | Supported | Original meaning kept |
| Numeric token, cost, and request counts | Supported | Existing fields kept |
| `daily` per-day data | Supported | Kept, stays compatible with existing components |
| `startDate` explicit date range | Not supported | New |
| `startTime` / `endTime` second-level range | Not supported | New, limited by historical precision |
| `model` model filter | Not supported | New |
| `groupBy=day/week/month` | Not supported | New |
| `buckets` grouped results | Not supported | New |
| Whole-response read consistency | No cross-query transactional snapshot guarantee today | Guaranteed by `Database::with_connection` |

No top-level `totalTokens`, `costUsd`, `requests`, or token-category field is renamed or redefined. New fields are added without wrapping the payload in an outer `data` or `summary` object, so that TinyPane paths such as `/totalTokens` keep working.

## 3. Service and Authentication

```http
GET /api/v1/usage/summary
X-API-Key: <read-only key>
Accept: application/json
```

- Default service address: `http://127.0.0.1:17891`; the port follows the local sharing service configuration.
- From other machines, use the LAN IP that the server is reachable at.
- Keys are generated, reset, and revoked under "AI Usage → Manage Devices → Read-only Usage HTTP API".
- The compatible way for paired devices to use `x-observatory-device` and `x-observatory-token` is kept; either credential on its own is enough.
- An API key only grants statistics query rights: it cannot pair devices, change settings, read conversation content, or trigger scans.
- The query scope is always limited to the apps this machine allows sharing; without an app filter, apps that are not allowed to be shared must not be returned either.
- The current plain HTTP deployment is kept; this design configures no public internet mapping, TLS, or account system.

The current endpoint depends on the LAN sharing service switch. A separate "public API switch" can be designed later; this document does not describe it as existing or as a query parameter required by this change.

## 4. Request Parameters

| Parameter | Type | Default | Rules |
|---|---|---|---|
| `days` | Integer | 7 | 1–366 calendar days, including the end date |
| `endDate` | Date string | Server's today | `YYYY-MM-DD`, inclusive of that day, must not be later than the server's today |
| `startDate` | Date string | None | New: inclusive of that day, must not be later than `endDate` |
| `startTime` | RFC 3339 string | None | New: start instant, inclusive; must carry a UTC offset or `Z` |
| `endTime` | RFC 3339 string | Current request time | End instant, exclusive; when given explicitly it must carry a UTC offset or `Z` |
| `appType` | String | None | When omitted, all apps allowed to be shared are aggregated; a given value must be supported and shareable |
| `model` | String | None | New: exact match on the effective pricing model ID, using the same basis as the page's model filter |
| `groupBy` | Enum string | `day` | New: `day`, `week`, `month` |

### 4.1 Three Ways to Select a Time Range

**Last N days**: use `days` and an optional `endDate`. With neither set, the default is the last 7 days ending on the server's today; with only `endDate` set, the query covers the 7 days ending on that date.

**Explicit date range**: set `startDate`; `endDate` may be omitted and then defaults to the server's today. `days` must not be sent at the same time. Both ends of the range are inclusive.

**Explicit second-level range**: `startTime` is required; when `endTime` is omitted, the server's current time for this request is used (truncated to whole seconds), and it must not be mixed with `days`, `startDate`, or `endDate`. The precision is whole seconds; timestamps without a time zone or with fractional seconds are rejected. `startTime < endTime` must hold.

All three modes are normalized to `[start, endExclusive)`. The start boundary is inclusive and the end boundary is exclusive. For example, when querying October the end instant is 11-01 00:00:00, and that final second is not included.

### 4.2 Range Limits and Time Zone

- At most 366 dates are covered on the server's local calendar; in second-level mode the count is based on the local dates touched by `start` through `endExclusive - 1 second`.
- Date mode does not allow an end date later than the server's today. In second-level mode the end instant may be at most the server's local midnight of tomorrow, which allows querying the records that exist "through the end of today"; the endpoint never produces future data.
- All day, week, and month boundaries are drawn in the **server's local time zone**. The offset carried by the requested timestamps determines the instant but never changes the grouping time zone.
- The response reports the UTC offset of each of the two boundary instants; a daylight-saving transition day is not guaranteed to be 24 hours long.
- When date mode or a day/week/month grouping hits a local midnight boundary that does not exist, the endpoint consistently returns 400 `INVALID_LOCAL_BOUNDARY` instead of silently shifting it. When a midnight occurs twice, the earlier instant is chosen, following the v0.1.30 `.earliest()` rule; a second-level request determines its own instants through the explicit UTC offset.
- Parameter names are case-sensitive; duplicate parameters, unknown parameters, empty filter values, invalid dates, and unsupported groupings return 400.

### 4.3 App and Model Filters

`claude` means Claude Code and `claude-desktop` means Claude Desktop; other identifiers follow the existing app list, for example `codex`, `gemini`, `dsh`, `zcode`.

The matching basis of `model` follows the page: `COALESCE(NULLIF(pricing_model, ''), model)`. When the pricing model is NULL or an empty string it falls back to `model`; otherwise the pricing model is used. This rule applies to both detail records and archives. The endpoint must state `modelFilterBasis=effectivePricingModel` in the documentation and in the response, and it must not be interpreted as matching the user-visible friendly names.

Requesting an app that is not shared returns 403; an unsupported app identifier returns 400. A well-formed model with no matching records returns 200 with zero values. When `appType` is omitted, the model filter applies only within the set of apps that are allowed to be shared.

## 5. Example Calls

### Claude Code Today

```http
GET /api/v1/usage/summary?days=1&appType=claude
X-API-Key: <read-only key>
```

### Last 30 Days, Grouped by Day

```http
GET /api/v1/usage/summary?days=30&appType=claude&groupBy=day
```

### Explicit Date Range, Grouped by Week

```http
GET /api/v1/usage/summary?startDate=2026-10-01&endDate=2026-10-31&appType=claude&groupBy=week
```

### Explicit Date Range, Grouped by Month

```http
GET /api/v1/usage/summary?startDate=2026-01-01&endDate=2026-10-31&groupBy=month
```

### Explicit Second-Level Range and Model

```http
GET /api/v1/usage/summary?startTime=2026-10-31T09:00:00%2B08:00&endTime=2026-10-31T18:00:00%2B08:00&appType=claude&model=claude-opus-5-5&groupBy=day
```

The plus sign in the URL must be encoded as `%2B`; clients can also use a standard URL query-parameter encoder. All the examples above except the first one use new parameters, so the current legacy version cannot use them directly.

## 6. Response Contract

All successful responses use `application/json` with `Cache-Control: no-store`. Amounts and usage counts are JSON numbers; when there is no data, a valid numeric 0 is returned instead of null or a string standing in for zero.

### 6.1 Fields

| Field | Type | Meaning |
|---|---|---|
| `scope` | string | `local` counts only the records of this machine; it becomes `multi` once paired devices are included |
| `devices` | string[] | The set of devices actually counted in this query; this machine is recorded as `local` |
| `appType` | string[]/null | The requested app list, null when not specified |
| `apps` | string[] | The apps allowed to be shared that were actually included in this query |
| `model` | string[]/null | The requested model list, null when not specified |
| `modelsByApp` | object/null | A per-platform model map; null when that filter is not used |
| `modelFilterBasis` | string | `effectivePricingModel` |
| `days` | integer | The number of local dates the query touches |
| `startDate` / `endDate` | string | The first and last local dates the query touches; they do not replace the precise range |
| `range.start` | string | RFC 3339, the effective start boundary |
| `range.endExclusive` | string | RFC 3339, the effective end boundary, exclusive |
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
| `daily` | object[] | Per-day results, kept for v0.1.30 compatibility and returned whatever `groupBy` is |

The top level is the total of the selected range; `buckets` and `daily` are two groupings of the same data. The sums of all three must agree.

### 6.2 Complete Example

The example below corresponds to `days=1&appType=claude&groupBy=day`; all values are simulated:

```json
{
  "scope": "local",
  "devices": ["local"],
  "appType": ["claude"],
  "apps": ["claude"],
  "model": null,
  "modelsByApp": null,
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

## 7. Day, Week, and Month Aggregation

| groupBy | Natural period | `period` format |
|---|---|---|
| day | From local midnight to the next local midnight | `2026-11-01` |
| week | From Monday midnight to the following Monday midnight | ISO week number, e.g. `2026-W44` |
| month | From the 1st of each month at midnight to the 1st of the next month at midnight | `2026-11` |

- ISO week numbers use the year the ISO week belongs to instead of concatenating the calendar year, which avoids wrong week numbers across the year boundary.
- Only periods that intersect the queried range are returned; the top-level query range is never expanded.
- A bucket's `start` and `endExclusive` are the **intersection of the natural period with the queried range**. `period` still identifies the natural period it belongs to.
- Empty periods are zero-filled and sorted by start time; at date precision the length of `daily` equals `days`.
- For a second-level range the per-day results also cover only the actual range, so 09:00–18:00 excludes the other hours of that day.
- `groupBy=week/month` only changes the buckets; it does not change the top-level totals or `daily`. Clients must not add `daily` and `buckets` together.

For example, a query from 2026-10-01 to 10-31 counts only the part of the first week that starts on 10-01, and the last week counts only up to 11-01 00:00:00; no September or November data is included.

## 8. Data and Cost Basis

### 8.1 Tokens

Computed after the cache semantics have been normalized:

```text
totalTokens = inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens
```

The same request must not have its cache counted twice because the input count already includes it. The model filter is applied to the base records first, and every aggregation level is computed afterwards.

### 8.2 Cost

- The estimated cost and the existing deduplication logic stored on this machine are reused; the GET request does not price records again at the current rates.
- The cost is not a subscription balance, a quota, or a billed settlement amount. Records without a price may contribute tokens while costing 0; a 0 must not be read as a confirmed free of charge.
- Sums are computed internally with fixed-point decimals, keeping the existing six-decimal statistical precision; every grouping is computed at that same precision and the top level is summed from the same dataset.
- `costUsd` is emitted as a JSON number and clients format the decimals themselves; formatted strings must not be summed again.

### 8.3 Detail Records, Archives, and Second-Level Precision

Detail records are filtered by `start <= createdAt < endExclusive`; a request is counted entirely in the period that contains its recorded time and is never split by request duration.

When history only has daily rollups left, a local day can only be counted in full or excluded entirely; it cannot be prorated by hour. If a query boundary truncates a date that has a matching daily rollup and there is no proof that complete valid detail records are still kept, the endpoint returns **422 RANGE_PRECISION_UNAVAILABLE** and asks the caller to pick whole dates instead.

The daily rollup must not be silently ignored just because the boundary covers only half a day, and old detail records must not be counted twice alongside an archived daily rollup. The check uses the same app, model, and validity filters; non-matching historical archives must not block the query.

**Archive time zone constraint**: a data directory should keep a fixed server time zone. Old daily rollups only carry a date label and lack the original time zone, so by default they keep being interpreted with the existing local date labels, and no cross-time-zone remapping is inferred or guaranteed. Later archives should store the original time zone and the day boundary instants. If the server time zone is known to have changed and the matching historical daily rollups cannot be restored accurately in the queried time zone, the endpoint returns 422 `RANGE_PRECISION_UNAVAILABLE` even when the request covers whole dates; with complete detail records the data can be rebuilt in the new time zone. This case is an explicit exception to the legacy request compatibility rule under a fixed time zone, and old daily rollups must never silently be reinterpreted as a day of the new time zone.

### 8.4 Consistency and Freshness

- The totals, `daily`, and `buckets` of one response must be based on the same database read snapshot and the same allow-list of apps, so that a background scan writing concurrently cannot make them contradict each other.
- A GET never scans and never triggers a remote update. `updatedAt` must not be treated as the time of the most recent data; clients should look at `lastScanAt` as well.
- After a user revokes a key, new requests fail immediately; in-flight read-only requests are not guaranteed to be cancelled midway.
- Empty data returns 200 with zero values and zero-filled periods. When the set of shareable apps is empty, the response carries empty `apps` and zero statistics.

## 9. Error Contract

The `error` string is kept and a stable `code` is added so that programs can branch on it:

```json
{
  "error": "查询区间截断了仅有日汇总的历史日期，请改用完整日期",
  "code": "RANGE_PRECISION_UNAVAILABLE",
  "details": {
    "dates": ["2026-10-01"]
  }
}
```

| HTTP status | `code` example | Scenario |
|---|---|---|
| 400 | INVALID_PARAMETERS | Unknown, duplicate, or malformed parameters |
| 400 | CONFLICTING_TIME_PARAMETERS | Conflicting time modes supplied at the same time |
| 400 | INVALID_DATE_RANGE | The start is not earlier than the end, more than 366 days, or beyond the allowed future boundary |
| 400 | UNSUPPORTED_GROUP_BY | Not `day`/`week`/`month` |
| 400 | UNSUPPORTED_APP_TYPE | The app identifier is not supported |
| 400 | INVALID_LOCAL_BOUNDARY | The local calendar midnight boundary does not exist |
| 401 | INVALID_API_KEY | Missing valid read-only key or paired-device credential |
| 403 | APP_NOT_SHARED | The app is not allowed to be shared |
| 422 | RANGE_PRECISION_UNAVAILABLE | Historical rollups cannot support the requested second-level range, or cannot be rebuilt accurately after a known archive time-zone change |
| 500 | USAGE_QUERY_FAILED | The statistics query failed |

Errors must never return the real key, internal SQL, conversation content, or file system paths. Identity checks are completed before any statistics are read; error messages must not be used to leak data of apps the caller is not authorized for.

## 10. Compatibility and Implementation Order

1. Keep the existing `days/endDate/appType` parameters and top-level metrics, and add parameter parsing and mutual-exclusion validation.
2. Implement the unified range normalization and the model filter, and complete the archive precision detection.
3. Produce `daily` from a consistent read snapshot, then form the buckets by local natural week/month; a second-level data query still filters to the actual range first.
4. Keep the `daily` field and return it whatever `groupBy` is, for compatibility with the existing widgets; new fields must not replace existing paths.
5. Update the documentation, the examples, and the UI integration notes, and ship only after the tests pass.

A large TinyPane tile can read `/totalTokens`, `/costUsd`, and `/requests`; the trend chart uses `/buckets`, the relative label path `/period`, and the numeric path `/totalTokens` or `/costUsd`. TinyPane itself shows at most the last 30 series points, and the endpoint does not truncate the queried range because of that.

## 11. Acceptance Checklist

- [x] Original v0.1.30 requests and top-level numeric paths stay compatible.→ `v030_response_shape_is_backward_compatible`
- [x] The last N days, an explicit date range, and a second-level range all honor the left-closed, right-open boundaries.→ `date_range_matches_equivalent_days_window`, `instant_range_is_left_closed_right_open`
- [x] No data for the current day, an empty shareable set, and models without matches return the correct zero values.→ `model_filter_uses_effective_pricing_model`, `app_scope_is_enforced`
- [x] Claude Code and Claude Desktop are distinguished precisely; the app allow-list applies to every grouping.→ `claude_code_and_desktop_are_separate`
- [x] Ordinary dates, daylight-saving transition days, ISO weeks across the year boundary, leap days, and months of different lengths are all correct.→ `iso_week_uses_the_iso_week_year`, `leap_day_and_variable_month_lengths_are_covered`, `local_midnight_resolution_rules`
- [x] A repeated midnight takes the earlier instant, and a midnight that does not exist returns the agreed error.→ `local_midnight_resolution_rules` (deterministic), `local_midnight_rules_hold_under_other_timezones` (on Unix the time zone is really switched through `TZ`; on Windows `chrono::Local` goes through the system API and ignores `TZ`, and that path is covered by the former)
- [ ] A known archive time-zone change that cannot be rebuilt returns 422.→ **Not implemented**: archives currently store only a date label, without any original time-zone metadata, so it cannot be determined whether the server time zone ever changed. Archives need a time-zone field later (see 8.3).
- [x] Incomplete leading and trailing weeks/months count only the part that was really queried, empty periods are zero-filled, and the ordering is stable.→ `month_buckets_use_natural_months_and_zero_fill`
- [x] The request, token, and cost sums of every bucket, of `daily`, and of the top level agree, and repeating a query does not change the amount.→ `top_level_daily_and_buckets_agree`
- [x] Archived data is never counted twice; historical boundaries that cannot be split accurately return 422 instead of an understated or invented result.→ `partial_day_with_only_rollups_returns_422`, `partial_day_with_details_is_not_blocked`
- [x] While concurrent scans write, one response stays consistent with its read snapshot.→ `summary_stays_internally_consistent_while_the_database_is_written`
- [x] Missing, wrong, or revoked credentials, unshared apps, conflicting time parameters, and unknown parameters all produce the agreed errors.→ `http_auth_validation_revocation_and_read_only_key`, `conflicting_time_parameters_are_rejected`, `invalid_parameters_have_stable_codes`
- [x] All the example JSON parses.→ `documented_examples_are_valid_json`
- [ ] Windows and Apple Silicon macOS run the same protocol and date tests.→ The protocol cases are platform-agnostic; macOS runs them separately in the release pipeline (`release.yml`). Locally only Windows has been verified.

## 12. Implementation Notes

How the implementation maps onto this document, and the trade-offs made along the way:

1. **Range normalization** is centralized in `public_api::resolve_range`; each of the three
   time modes is normalized to `[start, endExclusive)`. `days` and `startDate` produce
   exactly the same result when they describe the same interval.
2. **Aggregated queries** add the connection-level functions `usage_stats::daily_usage_rows`
   and `usage_stats::rollup_dates`. Both reuse the same deduplication filter, the same
   cache input-semantics normalization, and the same daily-rollup boundary rules as the page.
3. **Consistent snapshots** use `Database::with_connection`: every read and write goes
   through the same mutex, and no concurrent write happens while the lock is held, so the
   totals, `daily`, and `buckets` of one response necessarily come from the same data.
   Note that the closure must not call a `Database` method that takes the lock again —
   `std::sync::Mutex` is not reentrant and would self-deadlock.
4. **Top-level totals** are obtained by summing `daily` instead of running a separate SQL
   sum, so the amount cannot disagree with `buckets` because of a different rounding path.
5. **Model filtering** reuses the page's own basis `COALESCE(NULLIF(pricing_model,''), model)`;
   it takes effect on the detail-record path and on the daily-rollup path alike, and the
   aggregation layer needs no change.
6. **Precision detection** only affects second-level ranges: a date range always covers
   whole days, so there is no day that is cut in half. A day that is cut in half and only
   has a daily rollup left returns 422 and is listed in `details.dates`.
7. **Duplicate parameters** are rejected before parsing by `duplicate_query_keys` in
   `server.rs` — serde only keeps the last value of a duplicated parameter.

This document is an interface design only; apart from the capabilities marked as supported in section 2, the other ones still need to be developed and must not be used as endpoints that are live today.
