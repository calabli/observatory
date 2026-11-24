# Observatory — Product Guide

Observatory (观测台) is a local-first desktop app that shows how much you use AI coding tools, what that use costs, and which of your computers it came from. It reads the session logs each tool already writes to disk, keeps its own SQLite database, and never needs an account or a cloud service.

This guide describes what the product does today, the rules behind its numbers, and its known limits. For how it is built, see [architecture.md](architecture.md).

## 1. Who It Is For

| User                        | Situation                                                                | What Observatory answers                                                     |
| --------------------------- | ------------------------------------------------------------------------ | ---------------------------------------------------------------------------- |
| Multi-tool AI developer     | Uses Claude Code, Codex, DeepSeek Harness, ZCode and others side by side | One page with usage and estimated cost across every tool and model           |
| Multi-device user           | Moves between a Windows desktop and a Mac laptop                         | Combined totals for all paired devices, with each device's freshness visible |
| Subscription holder         | Pays a flat fee with a weekly quota                                      | Usage inside the current quota cycle, next to the subscription price         |
| Script and dashboard author | Wants usage numbers in another tool (TinyPane, a script)                 | A read-only HTTP API with a key                                              |

Out of scope: AI gateway routing, key custody, provider failover, team billing, employee monitoring, remote control of other machines.

## 2. Principles

1. **Numbers must be explainable.** Each figure states its source, time range and freshness. A value that cannot be computed precisely produces an explicit error instead of a quietly wrong number.
2. **Local by default.** Statistics, history and diagnostics work offline. Nothing leaves the machine unless the user turns on LAN sharing or clicks an action that calls the internet (models.dev prices, the egress IP check, update check).
3. **Honest capabilities.** "Not connected", "no permission", "offline" and "no data" are shown as different states. The app never shows demo data as if it were real.
4. **Read, don't modify.** Observatory reads other tools' files and never changes their configuration.
5. **Cheap to keep running.** Background work is incremental, rate-limited and skips when nothing changed.

## 3. Pages

The app has a collapsible left sidebar with three groups.

### 3.1 Usage

**Overview (总览)** — Today's cost on this machine and across all included devices, collection health (enabled sources, last scan), multi-device sync state, a 7-day token trend, the default network adapter, and the top four apps by cost. The refresh button runs a scan plus a device sync.

**AI Usage (AI 用量)** — The main analysis page.

- _Scope_: this machine, or several devices (pick any subset of local and paired devices).
- _Filters_: time range (today, yesterday, last 3 / 7 / 30 days, all time, custom date-time), app, provider, model. The day presets are whole local calendar days ending at 23:59:59 ("last 3 days" is the day before yesterday through today) and roll over at local midnight.
- _Metrics_: cost, requests, four token categories (fresh input, output, cache write, cache read), cache hit rate, success rate. The metric bar switches the trend between cost, requests, tokens and hit rate; the trend can be bars or a line.
- _Breakdowns_: per-app ranking, provider table, model table (with output speed and median first-token time), a model speed chart (sorted by usage or by speed, 8 models with "show all", models with fewer than 5,000 measured output tokens grayed as low-sample), a calendar heat map.
- _Requests_: paginated detail list with status-code filter, a detail dialog, and CSV export of the current page. Exact durations (measured) are marked differently from durations estimated from log timestamps.
- _Data sources_: turn each collector on or off, point the history import at a CC Switch database, see per-source request counts.
- _Pricing_: edit, add or delete per-model prices (input, output, cache read, cache write per million tokens); import from models.dev; configure automatic models.dev sync; recalculate costs for records that have no price yet.

**Subscriptions & Quota (订阅与额度)** — Record subscriptions (name, agent, amount, currency, billing period, validity dates, weekly reset time) and view usage per quota cycle or for the last N days. Details: [subscriptions-and-quota.md](subscriptions-and-quota.md).

**Insights (洞察)** — Deep cost, efficiency and session-level review across one machine or all paired devices:

- _Time range_: today, yesterday, last 3 / 7 / 14 / 30 days, or this month. Rolling ranges run up to now; yesterday is a closed day. The same labels and day spans are used on every page.
- _KPIs & budget alerts_: Prompt cache savings (reusing effective model pricing resolution for tiered/dated IDs), average context volume per request, 30-day cost forecast anchored on each source's active usage span, a change versus the previous period that compares the same elapsed length (so a partly finished day or month is not set against a full one), and inline daily + monthly budget thresholds with 80% warning and 100% over-budget banners.
- _Subscription ROI_: compares active subscription bills against equivalent API value in the selected range and current billing cycle (non-USD subscriptions keep their native currency without estimated exchange-rate conversion).
- _Visual analytics (with interactive hover tooltips)_: 7×24 local coding rhythm matrix (cost, requests, output speed), session health scatter (turns × cost × average context), model latency/speed stability dumbbell (auto-selecting speed when TTFT is absent), 100% stacked daily token-mix flow, and status-code health with automatic diagnosis.
- _Project cost & Sessions explorer_: attributes session spend to local workspace/repository directories (click any project to filter) and provides searchable, sortable session rankings with real titles, project paths, git branches and one-click resume command copy.
- _Agent single-task call-chain waterfall_: clicking any session bubble or table row opens an on-demand hierarchical waterfall (User Task → LLM calls → Tool / Subagent calls) with parallel tool union timing, LLM vs. tool duration attribution, top-tool breakdown, TTFT segments and a span inspector (extracted read-only from local logs for Antigravity, Claude Code, Codex, OpenCode and Gemini CLI, and falling back transparently to database turn waterfalls for other platforms or remote devices).
- _Visual report preview & export_: live in-app preview and download of standalone offline HTML reports (embedded interactive SVG charts, total token consumption and four-category breakdown, hover tooltips and print-to-PDF styling) alongside Markdown reports.

### 3.2 Devices

**Multi-device (多设备)** — Turn on LAN sharing, choose which apps to share, discover devices, pair with a code, enable or disable devices in the aggregate, revoke access, run a multi-device scan, and manage the read-only API key. When a device cannot be reached, the error says why in plain terms: nothing listening on its port (app closed or LAN sharing off), a timeout (asleep, offline or blocked by a firewall), an unreachable network (different network or changed IP), or a dropped connection.

**Network (网络诊断)** — Local adapters with IPv4/IPv6, state, gateway, DNS, and live receive/transmit rates with interactive chart hover tooltips. Rates are sampled every 3 seconds only while the page is visible; the chart keeps the last 60 samples in memory. The internet egress IPv4 is looked up through ipify only when the user clicks. The recommended adapter can be overridden because VPNs and split routing can make the guess wrong.

**Model Benchmarks (模型榜单)** — Public benchmark results for AI models from [Artificial Analysis](https://artificialanalysis.ai), next to what you measured locally.

- _Rankings_ across nine dimensions: overall intelligence, engineering, coding, agentic, finance & accounting, strategy & operations, legal, healthcare and economics, plus output speed, first-token time and blended price per million tokens.
- _Charts_: score against price and score against speed, with the Pareto frontier marked, so the best-value models stand out.
- _Effort tiers_: variants of one model (Max, Xhigh, High, Medium, Low, Minimal and others) are grouped as a family and can be compared side by side.
- _Local comparison_: models you use are matched to their benchmark entries and shown with your own measured output speed and spend from the usage data.
- _Setup_: it needs your own Artificial Analysis API key (free or Pro tier; a Pro request that is refused falls back to the free endpoint). The key is stored only in the local database, shown masked, hidden in the Activity Log and never shared with paired devices. Results are cached locally, so the page works offline after a refresh, and each refresh is logged as an outbound request.

**Markets (市场行情)** — A watch board for US, Taiwan, mainland China and Hong Kong stocks plus major global indices, using free public quote endpoints (Tencent, TWSE MIS, Yahoo Finance) chosen per market for the lowest delay.

- _Indices strip_ and a _stock pool_: built-in pools (S&P 500, CSI 300, Taiwan 150, Hong Kong 100, US-listed Chinese companies 中概股, the global AI supply chain) or a custom watchlist, shown as a list with sparklines or as a treemap heat map sized by market cap, traded value, move or equally. Sizes across currencies are compared in USD using live daily exchange rates (ExchangeRate-API), falling back to built-in approximate rates; the page says which it is using.
- _Detail chart_ for the selected symbol over 1 day, 5 days, 1 month, 3 months or 1 year, with volume and traded value. Traded value is exact for A-shares, Hong Kong and US stocks (including small watchlists whose US prices come from Yahoo); Taiwan stocks show an estimate (price × volume) marked "≈". Before the 09:00 open, outside trading hours or when the live TWSE feed leaves gaps, Taiwan quotes fall back to TWSE's official daily summary (closing price, change, volume and turnover), so the pool is never blank.
- _Stale quotes_: a stock with no trade for more than 10 days is grayed on the heat map, labelled "停牌或退市？" with its last trade date, and left out of the up/down counts and sector averages.
- _Search_ by code or name across markets; a bare four-digit code is also offered as a Taiwan listing.
- _Pool editing_: any pool, built-in or custom, can be edited: search and add companies to a sector, remove them, move them between sectors, restore removed defaults, and create, rename or delete custom pools. Edits are stored as changes on top of the built-in lists, so a later list refresh keeps them.
- _Fullscreen_ (F) shows the sector or market filters and the pool editor too; the corner chart closes with its close button, by clicking the selected tile again, by switching pools or sectors, or with Esc.
- _Settings_: red-up/green-down (CN) or green-up/red-down (US) colors and a refresh interval of 5, 15 or 30 seconds or manual. Polling stops while the window is hidden; pools over 100 symbols refresh at most every 15 seconds. One request accepts at most 600 symbols.
- The watchlist and settings are stored locally. Saving them is a preference write: it does not invalidate usage caches or change what paired devices see.

### 3.3 Records

**Activity Log (活动日志)** — A reverse-chronological log of five kinds of events, kept for 7 days:

- _File scan_: what triggered a scan round, files and new records per source, and per-file read offsets.
- _External call_: every HTTP request that reached the LAN service (caller, address, status, duration, size). Only the credential _type_ is recorded, never its value.
- _Device sync_: each pull from a paired device (skipped because unchanged, rows written, or the error).
- _Outbound request (接口请求)_: each call this app makes to an outside service (market quotes, charts and search, models.dev prices, the ipify egress lookup), with sanitized request parameters, every upstream HTTP call (provider, URL, status, size, latency, error) and the response body. At most 500 are kept. Market quote and chart polls repeat every few seconds, so they keep only a short preview (2 KB of the request, 8 KB of the response); other calls keep up to 64 KB and 128 KB.
- _Action_: key user operations and their results. Codes, tokens and keys are shown as "(hidden)".

"Hide routine" (on by default) hides unchanged device syncs, fingerprint checks and successful market polls in the combined view, so they cannot push scans and syncs off the page. The 接口请求 filter always shows every outbound request.

### 3.4 Tray / Menu Bar

Left-clicking the tray (Windows) or menu-bar (macOS) icon opens a quick panel: switch between this machine, a single remote device, or all devices, and between today and the last 7 days; it shows cost, requests, tokens, hit rate, success rate and per-app usage, refreshing every 30 seconds while visible. Right-click shows today's local totals, Open and Quit.

Closing the main window hides it; the app keeps collecting in the background. If the tray cannot be created, closing the window quits the app.

## 4. Data Sources

| Tier                       | Apps                                                                                                                             |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Native log collectors      | Claude Code, Codex, Gemini CLI, Grok Build, OpenCode, Pi, MiniMax Code, Antigravity, Kimi Code, DeepSeek Harness, ZCode          |
| History import / peer only | Claude Desktop, Hermes, OpenClaw (no local collector; data arrives through the CC Switch history import or from a paired device) |

Log locations follow each tool's defaults and honour their environment variables (`CODEX_HOME`, `GEMINI_CLI_HOME`, `GROK_HOME`, `PI_CODING_AGENT_DIR`, `OPENCODE_DB`, `ZCODE_DB`, `DSH_HOME`, `KIMI_CODE_HOME`, `MINIMAX_DATA_DIR`, `MAVIS_DATA_DIR`). Turning a source off stops future scans; existing history stays.

Claude Desktop traffic is folded into Claude Code in every dashboard view, because only the embedded Claude Code runtime is visible and a separate bucket would look like the Desktop's full usage.

## 5. How the Numbers Are Defined

- **Tokens.** Four categories: fresh input, output, cache write, cache read. For OpenAI-style protocols (Codex, Gemini CLI, Grok Build) the logged input already includes cached tokens, so the cached part is subtracted to get fresh input, and cache write is shown as not available.
- **Total tokens** = fresh input + output + cache write + cache read.
- **Cache hit rate** = cache read / (fresh input + cache write + cache read).
- **Cost** is an estimate: tokens × the local price table × the record's cost multiplier, in USD. Remote devices' costs are taken as they computed them and are never re-priced locally.
- **Unpriced vs. not billable.** A billable request with tokens but no matching price is _unpriced_ and is flagged so the user can add a price. A model with no public price (internal or bundled in a subscription) is _not billable_: its zero cost is correct and is not flagged.
- **Recalculation** only fills in records that have no cost yet; it first reports how many records it would touch. Records that already carry a cost are left alone.
- **Ranges** are half-open: `[start, end)`.
- **Retention.** Detail records are kept for 30 days, then folded into per-day rollups. Request lists, exact durations, output speed and first-token medians therefore cover only the last 30 days; totals and trends cover all history. A query whose boundary cuts through a day that exists only as a rollup returns a precision error listing the affected dates, with an option to widen to whole days.
- **Deduplication.** Every imported record is tracked in a long-term ledger, so re-scanning, replaying logs after pruning, or re-importing never double counts.

## 6. Multi-Device Sharing

- LAN sharing is **off by default**. Turning it on starts an HTTP service on TCP 17891 (UDP 17891 for discovery). CC Switch uses 17890; discovery probes both, and a CC Switch peer can be added manually as `IP:17890`.
- **Pairing**: one device generates a 6-digit one-time code (valid 5 minutes, 5 attempts). The other enters it; both sides then hold access tokens for each other. Both devices must have sharing on and be reachable in both directions, otherwise pairing fails and the code is not consumed.
- Only a token's SHA-256 digest is stored for incoming verification. Revoking a device on either side takes effect immediately. If the peer is offline when you disconnect, revoke it again from the peer's "devices allowed to access this machine" list.
- **What is shared**: usage statistics for the apps you select. Never shared: API keys, provider configuration, conversation contents.
- **Sync**: paired devices are pulled on a schedule; an unchanged fingerprint skips the download. Each remote device's latest snapshot is stored locally and replaced (not merged) on each sync. Disabling a device keeps its cached snapshot out of totals; disconnecting deletes it.
- **Multi-device scan**: from the aggregate view, "Scan & update" asks each enabled device to scan itself, then syncs, and shows per-device status ending in "all complete" or "partial failure".
- **Security limit**: transport is plain HTTP. Tokens and statistics cross the LAN unencrypted, so only enable sharing on trusted networks.

## 7. Read-only HTTP API

When sharing is on, the same port serves a read-only usage API authenticated with an API key generated in the Multi-device page (shown once; resetting or revoking invalidates the old key). The key can read aggregates only: it cannot pair, read request details or trigger scans. Reference and Postman collection: [read-only-usage-http-api.md](read-only-usage-http-api.md).

## 8. Data Directory and History Migration

Everything lives in `~/.observatory/` (override with `OBSERVATORY_HOME`): `observatory.db`, `model-pricing.json`, and `engine.lock`, which prevents two instances (for example the desktop app and the dev server) from using the same directory.

On first launch, if the database is empty and `~/.cc-switch/cc-switch.db` exists (override with `CC_SWITCH_DB` or the path field under Data Sources), Observatory imports CC Switch's statistics read-only: detail records, daily rollups, scan cursors and the dedup ledger. Providers keep only their names; no keys or conversation contents are imported. The import is a one-time baseline, not a live mirror. If Observatory has already collected on its own and the source contains rollups without a migration baseline, the import is refused to avoid an ambiguous merge; use a fresh `OBSERVATORY_HOME` to migrate first.

## 9. Known Limits

- LAN traffic is unencrypted and has no certificate-based device identity.
- Remote scan jobs live in memory; if the peer restarts mid-scan, the job shows as unknown.
- Several subscriptions for the same agent cannot be told apart by account.
- Subscription amounts are shown in their own currency with no exchange-rate conversion; API cost is always USD.
- Cost is an estimate from a local price table, not the provider's invoice.
- Collector coverage depends on each tool's version and log format.
- The macOS build is ad-hoc signed and not notarized.
- Benchmark data is Artificial Analysis's, needs your own API key, and is only as current as your last refresh; matching benchmark entries to the model names in your usage data is by name and can miss renamed or unusual variants.
- Market data comes from free public endpoints (Tencent, TWSE MIS and OpenAPI, Yahoo Finance) that offer no service guarantee and may throttle or block a client. When that happens the app backs off and shows the last good quotes marked "（缓存）"; Taiwan traded value is an estimate, and cross-currency comparisons use daily exchange rates.
- The CSI 300 and S&P 500 pools change with index reviews; `npm run universes:refresh` checks them, but the Taiwan and Hong Kong pools are maintained by hand.

## 10. Roadmap Candidates

These are ideas, not commitments:

- Encrypted LAN transport with verifiable device identity.
- Persistent remote scan jobs with per-device retry.
- Real device performance metrics (CPU, memory, disk) correlated with AI usage on one timeline.
- Network diagnostics beyond the adapter view: gateway latency, DNS resolution, connection checks.
