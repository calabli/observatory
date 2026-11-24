# Changelog

Release notes for each version. The release workflow publishes the section whose
heading matches the pushed tag, so add an entry under **Unreleased** as you work and
rename it to the new version (`## [x.y.z] - YYYY-MM-DD`) before tagging.

## Unreleased

## [0.1.17] - 2026-11-24

- Markets: added All-Time High drawdown (`距历史高点`) and All-Time Low rebound (`距历史低点`) backed by a 7-day SQLite cache + live intraday breakout updates (`markets_extremes`), displayed with multi-column sorting in list view and switchable color modes (`今日涨跌` / `距历史高点` / `距历史低点`) + tooltip details in the heatmap.
- Markets charts: upgraded individual stock charts with extended timeframes (`6月` / `2年` / `5年` / `全部`), true OHLC candlesticks & volume bars, `MA5/10/20/60` moving averages, ATH/ATL/52W reference lines, live range stats (max drawdown, amplitude, avg volume), `MACD` / `RSI` / `KDJ` sub-indicators, normalized market-benchmark overlay (Alpha vs S&P 500 / CSI 300 / HSI / TWII), wheel zoom & drag pan, a wide expanded-chart modal, and 1:1 responsive full-width SVG rendering.

## [0.1.16] - 2026-11-24

- Benchmarks: added a new "模型榜单" (Model Benchmarks) page powered by Artificial Analysis API v2 (multi-page ingestion, local SQLite caching, outbound HTTP logging to Activity Log, masked API key storage, 9-dimension rankings, interactive Pareto frontier scatter charts for price & speed, full effort-tier family grouping & comparison for Max/Xhigh/High/Medium/Low variants, and side-by-side comparison against your local measured speed and spending).
- Markets: Taiwan pools (`TWSE 150`) now automatically fill pre-market / off-hours / MIS-gap quotes via a single cached TWSE OpenAPI `STOCK_DAY_ALL` request (with exact closing prices, change %, volume, and TWD turnover) and use larger MIS batches (50 per call, concurrency 2 with retry), preventing 140+ Taiwan stocks from blanking out before the 09:00 market open.
- Markets: stock pools (both built-in universes and custom pools) can now be edited directly — search and add companies into any sub-sector, remove companies, move a company between sectors, restore removed defaults, or create/rename/delete custom pools, with edits saved locally as deltas so future universe refreshes remain intact.
- Markets fullscreen: added the full sub-sector / market filter bar and pool editing controls inside fullscreen mode, measured real canvas height cleanly below the top bars, and made the corner K-line chart drawer easy to dismiss via a pinned top-right "关闭" button, clicking the selected tile again, switching pools/sectors, or pressing Esc.
- Markets pools: audited and corrected sector classifications across CSI 300 (95 stocks fixed, e.g. East Money moved to 银行与金融, Cambrian/Innolight/GigaDevice to 电子与半导体, telecom/software split into 通信与软件, solar/battery/power equipment unified into 新能源与电气), TWSE Top 150, and HKEX Core 105.

## [0.1.15] - 2026-11-24

- Markets: Tencent's batch quote service and its per-stock intraday service now back off independently. A blocked intraday service had paused the working batch service too, so every quote fell back to Yahoo and lost its market cap and traded value (shown as "—", with equal-sized heat-map tiles).
- Markets: small watchlists get market cap as well as traded value from one Tencent batch request when their prices come from Yahoo.
- Markets list: a stock without intraday points shows a labelled "—" in the trend column instead of a flat dashed line that looked like an unchanged price.

- Markets heat map: a stock without a market cap (Yahoo fallback quotes) no longer gets an oversized tile; it takes the median size, and the engine reuses its last known market cap scaled by price. AMD no longer outsizes NVIDIA.
- Markets heat map and list swap in a pool's quotes as one complete snapshot: the previous map stays (dimmed, with a progress bar) until every quote has arrived, the first load shows a shimmer instead of empty tiles, the new map fades in, and on refresh tiles glide to their new sizes and colors fade (off when the system asks for reduced motion).

- Markets heat map fits the window: it takes the height left below the controls (which stay on single rows in heat-map view), so every stock is visible without scrolling; fullscreen is unchanged.
- Markets: Tencent batch requests run two at a time and a refused batch is retried, so a firewall response (HTTP 501) no longer blanks whole sectors such as S&P 500 Financials.

## [0.1.14] - 2026-11-24

- Startup is lighter: pages other than the overview load on first visit (the startup bundle went from 1 MB to about 320 KB), and the ZCode icon is no longer bundled at 1024 px.
- Markets compares market cap and traded value across currencies with live daily exchange rates (ExchangeRate-API), falling back to built-in rates; small watchlists now show exact traded value for US stocks too.
- Markets copes with a blocked quote source: after a Tencent failure it pauses requests with a growing back-off and routes CN / HK quotes through Yahoo, and a failed quote shows the last good one marked "（缓存）" instead of an error.
- Activity Log records a repeated successful market poll once every 10 minutes per request (failures and recoveries always), instead of every few seconds.
- `npm run mac:install` no longer builds the signed update package, so local builds finish without the missing-key error.
- Release notes now live in `CHANGELOG.md` (the release takes the section matching its tag), and `npm run version:bump` turns the Unreleased notes into the new version's section.
- Faster releases: the check job is split into three parallel jobs, and a new workflow keeps Rust build caches warm on `main` so tag builds no longer compile everything from scratch.
- `npm run universes:refresh` checks the S&P 500 and CSI 300 pools against their current constituents and can apply the changes.
- Browser UI tests (`npm run test:ui`, Playwright in the installed Chrome) cover page navigation, time ranges, Markets pools and Activity Log filters.

## [0.1.13] - 2026-11-24

- Markets: new **中概股** pool of 53 US-listed Chinese companies in 9 sectors. Exact traded value (成交额) for A-shares, Hong Kong and US stocks in the detail panel, heat-map tooltip and "按成交额" sizing (Taiwan shows a "≈" estimate). Stocks with no trade for over 10 days are flagged as possibly suspended or delisted and left out of the up/down counts.
- AI usage: the model speed chart sorts by usage or speed, shows 8 models with "show all", and grays low-sample models.
- Multi-device: an unreachable device now says why (port not listening, timeout or firewall, unreachable network, dropped connection).
- The local database releases free space automatically (incremental vacuum, switched once on existing databases).
- Maintenance: large pages split into smaller modules, new tests for the heat map and report builder, zero clippy warnings, docs checked by Prettier, `npm run version:bump`, and release actions on checkout/setup-node v7.

## [0.1.12] - 2026-11-24

- Insights: the time range adds **今天** (today), **昨天** (yesterday) and **近 3 天** (last 3 days). Yesterday is a closed day compared against the day before.
- All preset time ranges (labels and day spans) are now defined in one table shared by the AI usage page, Insights and the tray panel, so they stay consistent. The AI usage dropdown uses the shorter "近 N 天" labels; previously saved selections still load.
- Insights counts records up to and including the last second of a closed range, matching the usage statistics.

## [0.1.11] - 2026-11-24

- AI usage: the time filter adds **昨天** (yesterday) and **最近 3 天** (last 3 days).
- Dropdowns now share one height and chevron in the macOS app (WebKit ignored the height of native selects), including the multi-device picker.
- Activity Log: successful market polls no longer flood the combined view (hidden with "hide routine"; the 接口请求 filter still lists them), and polls store short previews so the database no longer churns tens of MB.
- Insights: the change versus the previous period compares the same elapsed length instead of a partial period against a full one.
- Markets: watchlist saves no longer reset usage caches or make paired devices re-sync; quick successive edits are kept; duplicate spellings of a symbol all get batch quotes; US share classes survive search (BRK-B); quote requests are capped at 600 symbols.
- Agent trace skips local transcript lookups when only peer devices are selected; the tray quick panel is created at startup so it never opens blank.
- `npm run mac:install` no longer installs a stale bundle after a failed build.

## [0.1.10] - 2026-11-23

- Added outbound external API call logging (`api_out` / **接口请求**) to the Activity Log module, automatically capturing all market quote/chart/search calls (`qt.gtimg.cn`, `web.ifzq.gtimg.cn`, `smartbox.gtimg.cn`, `mis.twse.com.tw`, Yahoo Finance), `models.dev` pricing fetches, and `ipify` IP lookups.
- Recorded full sanitized request parameters, per-request upstream HTTP trace details (provider, method, full URL, HTTP status, response bytes, latency, errors), and formatted response payloads (lazy-loaded with bounded 500-entry retention).
- Enhanced the Activity Log UI with a dedicated **接口请求** filter, upstream HTTP breakdown table, and side-by-side scrollable Request Params & Response Body viewers with one-click copy.

## [0.1.9] - 2026-11-23

- Added a new **Markets (市场行情)** page with zero-API-key, zero-registration live multi-market quotes routed per market for maximum freshness: Tencent Finance real-time + batch quotes (1–3s lag with live market cap) for A-shares, Hong Kong, and US universes, official TWSE/TPEx MIS batch real-time (5s lag) for Taiwan equities, and Yahoo Finance v8 Chart (with US pre/post-market and multi-range intraday/historical charts).
- Added full-market Finviz-style Squarified Treemap heatmaps covering 1,058 live-verified global companies across five built-in sector-grouped universes: **S&P 500 (503)**, **CSI 300 (300)**, **TWSE Top 150**, **HKEX Core (105)**, and **Global AI Supply Chain (49)**, alongside a customizable personal watchlist with one-click starring and global symbol search.
- Added native **Fullscreen Heatmap** mode (`F` / `Esc`) with dynamic viewport-resolution layout, click-to-zoom sector headers, live market breadth counters, market-cap / turnover / move / equal weighting, in-pool filtering, and a floating corner live chart drawer.
- Fixed a flaky parallel Rust database test race by serializing `opening_an_old_database_folds_the_case_duplicate_pricing_rows`.

## [0.1.8] - 2026-11-23

- Fixed stale relative time ranges ("Today", "Last 7/14/30 days", "This month") across AI Usage, Insights, Overview, Tray Panel, and Subscriptions when leaving the app running past midnight or waking from sleep: added a zero-polling `useLocalDayKey` tracker that advances automatically at local midnight, on window focus/visibility restore, and on manual refresh.
- Fixed Insights queries and session trace modals freezing `endDate` at initial page mount time, so newly scanned requests are always included immediately on refresh.
- Updated README, product guide, and architecture documentation for Insights, Agent call-chain waterfalls, HTML reports, and v0.1.7+ optimizations.

## [0.1.7] - 2026-11-22

- Added Agent single-task call-chain waterfall (`session_trace`) inside Insights session drill-down: extracts hierarchical User Task → LLM → Tool/Subagent call trees with parallel tool union timing, LLM vs tool duration attribution, top tools breakdown, and TTFT segments for Antigravity, Claude Code, Codex, OpenCode, and Gemini CLI (with graceful DB turn waterfall fallback for other platforms and remote devices).
- Added project / repository cost ranking (`Project Cost`), searchable & sortable Sessions Explorer with cross-platform session title, project directory, git branch, and one-click resume command copy, plus Subscription ROI gauge and multi-device toggle in Insights.
- Added daily & monthly budget alerts (80% warning / 100% over-budget banners) and rich visual report preview & export supporting standalone offline HTML (with embedded interactive SVG charts, total token consumption breakdown, and hover tooltips) alongside Markdown.
- Added interactive hover tooltips across all report charts, Insights charts (30-day forecast curve, model stability dumbbell, token mix, status bar, project ranking, waterfall spans, context growth sparkline), AI usage trend chart, and network throughput chart.
- Added estimated latency & output speed backfill for DeepSeek Harness and Gemini CLI, wired proxy-shadow deduplication into Insights, and reduced idle background overhead (lazy tray WebView creation, cached 5s ticker checks, and visibility-gated frontend polling).

## [0.1.6] - 2026-11-21

- Fixed Insights cache savings calculation for tiered and dated model IDs (such as Antigravity `gemini-*-high`/`medium`) by reusing `find_model_pricing` resolution.
- Fixed 30-day cost forecast burn rate to anchor on each platform's effective active span (never diluted by unused days before first usage) and aligned period-over-period delta boundaries to whole local days.
- Auto-selected output speed stability dumbbell on platforms without TTFT (such as Antigravity).

## [0.1.5] - 2026-11-21

- Added the new Insights page with cache savings, 30-day cost forecast & budget tracking, 7x24 coding rhythm heatmap, session health scatter plot, model P50-P95 stability dumbbells, token mix stacked area, and status health breakdown.
- Added estimated latency & output speed for Antigravity tool-calling steps, with streaming low-memory transcript parsing and one-time backfill.
- Fixed macOS fullscreen close leaving an empty black Space; closing now exits fullscreen before hiding to the menu bar.
- Updated app ordering, set the macOS Utility app category, and reduced background CPU/SQLite overhead in the tray and scan loops.

## [0.1.4] - 2026-11-21

- Refreshed the dashboard and usage views with widget-style cards, updated charts, and a new app icon.
- Added model output speed and first-token latency to the usage view.
- Local scans read this device only. Scan feedback stays with the local or multi-device view that started it.
- The Windows tray icon follows the taskbar color mode. Secondary text is easier to read in light mode.

## [0.1.3] - 2026-11-19

- Sidebar redesign.

## [0.1.2] - 2026-11-13

- Pricing correctness and a working quick panel.

## [0.1.1] - 2026-11-10

- Use the English product name for the installer and app bundle.

## [0.1.0] - 2026-11-10

- Replace the updater public key with a new signing key.
