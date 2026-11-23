# Observatory — Technical Architecture

This document explains how Observatory is built: the processes, the modules, the data model, and the rules the code relies on. For what the product does and how its numbers are defined, read [product.md](product.md) first.

## 1. Stack

| Layer | Technology |
| --- | --- |
| Desktop shell | Tauri 2 (`src-tauri/`): windows, tray, single instance, updater |
| Engine | Rust library `observatory-usage-core` (`native/`): collectors, SQLite, statistics, LAN service |
| Storage | SQLite via `rusqlite` (bundled), WAL mode |
| Async runtime | Tokio; `axum` for the LAN HTTP service, `reqwest` (rustls) for outbound calls |
| Frontend | React 18 + TypeScript + Vite, TanStack Query for data fetching, Radix Select, lucide icons |
| Tests | `cargo test` (unit + `native/tests/` integration), `node --test` for pure frontend logic |

## 2. Repository Layout

```
src/                    React frontend
  main.tsx              App shell: sidebar, overview page, page routing, tray entry
  native/usageClient.ts The single call path into the engine (`usageCall`)
  usage/                AI usage page, devices, pricing, multi-device scan
  insights/             Insights analytics, Agent trace waterfall, HTML/Markdown report builder
  markets/              Markets page, built-in stock pools (universes.json), treemap layout
  subscriptions/        Subscriptions & quota page and its cycle math
  network/              Network page and rate calculation
  activity/             Activity log page
  tray/                 Quick panel rendered in the tray window
  lib/                  models.dev price import and auto-sync
  types/usage.ts        Shared wire types for usage data
  theme/                CC Switch-derived tokens and the sidebar redesign
native/                 Rust engine crate
  src/api.rs            UsageBackend: open, background loops, the op dispatcher
  src/main.rs           `observatory-engine` binary: JSON-lines over stdio (dev only)
  src/insights.rs       Insights aggregates, project cost attribution, session metadata cache
  src/trace.rs          On-demand Agent single-task call-chain trace extraction (read-only)
  src/markets.rs        Quote, chart and symbol search fetchers (Tencent, TWSE MIS, Yahoo) and watchlist config
  src/database/         Connection management, schema, CC Switch import, price seed
  src/services/         Collectors (`session_usage_*.rs`), statistics, pricing
  src/session_manager/  Per-tool session file parsers used by the collectors
  src/proxy/usage/      Token parsing and cost calculation
  src/lan_usage/        LAN service, discovery, pairing, sync client, projection, public API
  src/activity.rs       Activity log recording, including outbound request traces
  src/subscriptions.rs  Subscription storage and weekly cycles
  src/scan_jobs.rs      In-memory remote scan jobs
  src/network_overview.rs Adapter snapshot and egress IP
  tests/                Integration tests against real databases and HTTP servers
src-tauri/              Tauri shell (main.rs), config, icons, NSIS template
scripts/                Vite dev bridge (usageBridge.ts), cargo wrapper, macOS local installer (mac-install.mjs)
tests/                  Frontend unit tests (*.test.mjs)
packages/index.html     Promo page published to GitHub Pages
docs/                   This documentation and API examples
```

## 3. Runtime Modes

The engine is one Rust library used by two hosts. The frontend does not know which one it is talking to; `usageCall(op, args)` picks the transport.

```
Production (desktop app)                 Development (browser)
┌──────────────────────────┐             ┌──────────────┐  fetch   ┌─────────────────────┐
│ WebView (React)          │             │ Browser      │ ───────▶ │ Vite + usageBridge  │
│   invoke("usage_call")   │             │ (React)      │          │ /__observatory/*    │
├──────────────────────────┤             └──────────────┘          └──────────┬──────────┘
│ src-tauri main.rs        │                                         stdin/stdout JSON lines
│   UsageBackend (in-proc) │                                       ┌──────────▼──────────┐
└──────────────────────────┘                                       │ observatory-engine  │
                                                                   │   UsageBackend      │
                                                                   └─────────────────────┘
```

- **Desktop**: `src-tauri/src/main.rs` registers two commands. `usage_call` forwards `(operation, args)` to `UsageBackend::call`. `tray_action` lets only the tray window open, hide or quit. The backend is created lazily in a `OnceCell` and warmed up in the background at startup, so the first request waits for the same initialization instead of opening the database twice.
- **Development**: `npm run dev` starts Vite on `127.0.0.1:5188`. The `usageBridge` plugin spawns `native/target/debug/observatory-engine` and relays requests. It accepts only loopback `Host` headers and same-origin requests, and requires a random per-session token fetched from `/__observatory/session`. Requests time out after 120 s (600 s for `sync`).

Both modes use the same data directory, and `engine.lock` (an exclusive file lock) makes the second one fail fast with a clear message.

### Windows

| Label | Purpose |
| --- | --- |
| `main` | The app. Close hides it when the tray exists (exiting macOS fullscreen cleanly before hiding so Space transitions never leave a blank window), otherwise quits. |
| `tray-panel` | Frameless quick panel loaded as `index.html?surface=tray`. Created hidden when the tray is set up, so its page has loaded by the first click; `ensure_tray_panel` recreates it on demand if it is ever missing. It is hidden rather than destroyed. |

A background thread in the shell refreshes the tray menu text (today's cost, requests, tokens) every 30 s, skipping SQLite queries whenever `today_cache_key` (`(data_epoch, local_day)`) is unchanged.

## 4. Engine Core (`native/src/api.rs`)

`UsageBackend` owns `Arc<Database>`, a `tokio::sync::Mutex` scan lock, and the instance lock.

**Startup (`open`)**: create the data directory, take `engine.lock`, open the database, write default settings, replay `model-pricing.json` (the full cost backfill is deferred until after the first scan), and run the first-launch CC Switch import if the database is empty.

**Background loops (`start_background`)**:

- *Local scan loop*: ticks every 5 s, caching `(data_epoch, refresh_interval, import_failed)` so idle ticks do zero `spawn_blocking` or SQLite reads; scans when the configured interval (30 s, 60 s or manual) has elapsed since the last completed scan. Uses `try_lock` on the scan lock so it never queues behind a manual scan. Runs `wal_checkpoint(TRUNCATE)` and `PRAGMA optimize` at most hourly, right after a scan.
- *LAN loops* (`lan_usage::start_background`): starts the HTTP service if sharing is on, then pulls paired devices on the configured interval.

**Dispatcher (`call` → `dispatch`)**: one `match` over string op names. Every op that touches SQLite runs in `spawn_blocking`, because the connection lock is a `std::sync::Mutex` and long aggregates must not stall Tokio workers shared with the LAN service. Ops listed in `activity::action_label` are also written to the activity log with sanitized arguments. Ops listed in `activity::api_out_label` (market quotes, charts and search, `modelsdev_fetch`, `network_international_ipv4`) run inside `capture_http_traces`, a Tokio task-local collector that every outbound HTTP helper reports to through `record_http_trace`; the call is then recorded as an `api_out` event with its request, upstream calls and a capped response preview (see section 5).

| Group | Ops |
| --- | --- |
| Status & settings | `status`, `preferences_save`, `sources_save`, `scan_revision` |
| Dashboards & Insights | `dashboard`, `tray_dashboard`, `request_detail`, `insights`, `session_turns`, `session_trace` |
| Scanning | `sync` (local scan + pull peers), `scan_local_only`, `import_cc_switch` |
| Pricing | `pricing_list`, `pricing_update`, `pricing_batch`, `pricing_delete`, `pricing_backfill`, `modelsdev_fetch`, `modelsdev_config_get`, `modelsdev_config_save`, `modelsdev_record_result` |
| Subscriptions | `subscriptions_get`, `subscriptions_save`, `subscription_usage`, `subscription_cycle_usage`, `subscription_cycle_series` |
| LAN | `lan_status`, `lan_settings_save`, `lan_discover`, `lan_connect`, `lan_pair_code`, `lan_device_enabled`, `lan_disconnect`, `lan_incoming_list`, `lan_incoming_revoke`, `scan_remote_start`, `scan_remote_status`, `scan_remote_sync` |
| Read-only API key | `usage_api_key_status`, `usage_api_key_rotate`, `usage_api_key_revoke` |
| Network | `network_snapshot`, `network_international_ipv4` |
| Markets | `markets_config_get`, `markets_config_save`, `markets_quotes`, `markets_chart`, `markets_search` |
| Activity | `activity_list`, `activity_detail` |

Errors cross the boundary as strings. Errors that need a code and details (for example a precision error with affected dates) are wrapped as `{"observatoryError": {...}}`, which `parseUsageError` in the frontend unwraps.

## 5. Data Model

Schema: `native/src/database/schema.sql`. The database is metrics-only; it never stores conversation contents or credentials of the tools it reads.

| Table | Contents |
| --- | --- |
| `proxy_request_logs` | One row per AI request: app, provider, model, four token counts, costs (decimal strings), latency, first-token time, duration, status, `data_source`, `created_at` (Unix seconds). Kept 30 days. |
| `usage_daily_rollups` | Per day × app × provider × model aggregates of pruned detail rows and imported CC Switch rollups. |
| `session_usage_dedup` | Long-term ledger of every imported record id and its semantic id. Survives pruning so replayed logs are not counted twice. |
| `session_log_sync` | Per-file collector cursors: modification time, line/byte offset, tail fingerprint. |
| `model_pricing` | Per-model prices per million tokens, model id case-insensitive. |
| `settings` | Key-value store: preferences, LAN settings, tokens and grants, API key hash, subscriptions, import state. |
| `providers` | Provider names kept from the CC Switch import (configuration is always `{}`). |
| `activity_log` | Activity events; pruned after 7 days. `api_out` rows are also capped at 500 (checked every 25 writes). Polling ops (`markets_quotes`, `markets_chart`) store 2 KB / 8 KB request / response previews, other outbound ops 64 KB / 128 KB. `activity_list` takes `hidePolling` to drop successful polls in SQL. Writes go through `write_quietly`, so they never move the write epoch. |
| LAN tables | Paired devices and their latest snapshots (created by `create_lan_usage_tables`). |

**Token semantics.** `input_token_semantics` records whether `input_tokens` already includes cache reads (OpenAI-style) or not. Queries normalize to fresh input through the SQL helpers in `services/sql_helpers.rs`.

**Write epoch.** `Database::write_epoch` increments on every row-level write (SQLite update hook). Caches key on it, so an empty scan round does not invalidate anything. Bookkeeping writes that do not affect statistics (cursor rows, the activity log, the Markets watchlist via `set_setting_quietly`) skip the epoch, which also keeps the LAN export fingerprint unchanged.

## 6. Collection Pipeline

`scan_database` (in `api.rs`) runs each enabled collector in a fixed order: Claude, Codex, Gemini, OpenCode, Grok Build, Pi, MiniMax Code, Antigravity, Kimi, DeepSeek Harness, ZCode. Each collector:

1. Lists the tool's session files or database.
2. Loads all cursors once (`load_sync_cursors`) and skips files whose modification time and tail fingerprint are unchanged.
3. Reads from the saved byte offset, parses records with the matching `session_manager` provider (Antigravity uses a streaming low-memory deserializer that skips thinking/media/tool payload bodies), computes estimated latencies where native durations are absent (Antigravity, DeepSeek Harness, Gemini CLI), and computes cost with the cached price table.
4. Inserts rows in a per-file transaction, checking `session_usage_dedup` first.
5. Advances the cursor.

After all collectors: `rollup_and_prune(30)` folds detail rows older than 30 days into `usage_daily_rollups` (a cheap `EXISTS` check exits early when nothing is due), any deferred price backfill runs, the scan revision and completion time are recorded, and the activity log gets a per-source and per-file summary (built by probing cursor and row high-water marks before and after each collector).

The scan lock serializes background scans, manual scans, remote-requested scans and the CC Switch import.

## 7. Querying and Caching

- Statistics live in `services/usage_stats.rs`. Every aggregate combines detail rows and rollups for the requested range, and refuses a range that splits a rollup-only day.
- Insights analytics live in `native/src/insights.rs` (reusing `effective_usage_log_filter_for_range` proxy-shadow deduplication, `find_model_pricing` for cache-savings math, and an in-memory cache with per-platform batch reads for session titles, project directories and git branches).
- Agent task call-chain traces live in `native/src/trace.rs`: `session_trace` parses local session transcripts read-only on demand (Antigravity, Claude Code, Codex, OpenCode, Gemini CLI) to build hierarchical LLM + tool/subagent waterfalls with parallel interval union math, falling back to `proxy_request_logs` turns for other platforms or remote devices. Conversation contents are never written to `observatory.db`.
- The Insights period-over-period change compares the current range up to now with the same elapsed length of the preceding range (day-aligned start). When that window's last part-day has already been rolled up, it counts only the detail rows still present.
- `session_trace` reads local transcript files only when the session can be on this machine; with only peer devices selected it builds the waterfall from the synced turns.
- `dashboard`, `tray_dashboard` and `insights` compute their parts inside one read snapshot (`with_read_connection`), so a concurrent scan cannot make totals, trends and groupings disagree.
- Results are cached in a 16-entry LRU keyed by `(database instance, write epoch, op + args [+ local date for open-ended ranges])`.
- `status` caches its two full-table counts by write epoch.
- The frontend polls the cheap `scan_revision` op and invalidates TanStack Query caches only when the revision or epoch changes.

**Multi-device projection** (`lan_usage/projection.rs`): multi-device queries run the same SQL against an in-memory SQLite database that contains the local rows plus each selected peer's snapshot, namespaced by device. Projections are cached by device selection and data version. Remote rows are never written into the local log tables.

## 8. Pricing

- Seed prices ship in `database/pricing_seed.rs`; user edits persist to `model-pricing.json` in the data directory and are replayed at startup.
- Model ids are matched case-insensitively on the effective pricing model (`pricing_model`, falling back to `model`).
- `pricing_backfill` with `preview: true` returns how many records lack a cost; without it, it recomputes only those records.
- models.dev import: the engine fetches `https://models.dev/api.json` (`modelsdev_fetch`); the frontend (`src/lib/modelsDev*.ts`) flattens it, applies the user's selection and writes prices through `pricing_batch`. Auto-sync runs at startup at most once every six hours.

## 9. LAN Service (`native/src/lan_usage/`)

| File | Role |
| --- | --- |
| `server.rs` | `axum` router, auth extraction, request logging into the activity log |
| `discovery.rs` | UDP broadcast discovery on the sharing port (and 17890 for CC Switch peers) |
| `identity.rs` | Device id, name and share settings |
| `pairing.rs` | Pairing codes, grants (SHA-256 of tokens), revocation |
| `client.rs` | Outbound calls: info, pair, revoke, fingerprint, snapshot sync, remote scan |
| `storage.rs` | Device list, snapshot storage, local export |
| `projection.rs` | In-memory multi-device query database |
| `public_api.rs` | Read-only usage API: summaries, cycles, dashboards |
| `interfaces.rs` | Local interface addresses for discovery |

**Routes** (all on the sharing port, default 17891):

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /api/lan/info` | none | Device id and name, used before pairing |
| `POST /api/lan/pair` | pairing code | Two-way pairing; returns the access token |
| `POST /api/lan/revoke` | peer token | Peer revokes its own access |
| `GET /api/lan/usage/fingerprint` | peer token | Cheap change check before a sync |
| `GET /api/lan/usage` | peer token | Full snapshot of shared apps |
| `POST /api/lan/scan`, `/api/lan/scan-status` | peer token | Start or poll a remote scan job (`scan_jobs.rs`, idempotent by requester + key) |
| `GET /api/v1/usage/summary`, `/cycle`, `/cycles` | API key | Read-only usage API |
| `POST /api/v1/dashboard/usage`, `/dashboard/models` | API key | Dashboard aggregates |

Peer requests carry `X-Observatory-Device` and `X-Observatory-Token`; the API uses `X-API-Key`. Pairing runs in one round trip: the initiator first stores a grant for the reverse token, then calls `/api/lan/pair` with the code and that token; if the call fails the previous grant is restored. Transport is plain HTTP (see the security limits in [product.md](product.md#6-multi-device-sharing)).

The public API contract, parameters, errors and examples are in [read-only-usage-http-api.md](read-only-usage-http-api.md); `native/tests/usage_http_api.rs` runs every request in the Postman collection against a live server.

## 10. Frontend

- **Data access**: all calls go through `usageCall`. Query keys start with `"usage"` so a scan can invalidate everything at once; background status polling (`scan_revision`, `lan_status`) is gated on `document.visibilityState === "visible"`.
- **Routing**: `main.tsx` keeps the current page in React state; each page is a component (`UsageWorkspace`, `InsightsWorkspace`, `SubscriptionWorkspace`, `MarketsWorkspace`, `DeviceManager`, `NetworkOverview`, `ActivityLog`). The tray window renders `TrayPanel` instead of `App`.
- **Pure logic is separated for testing**: `usage/appRanking.ts`, `chartData.ts`, `modelDisplay.ts`, `requestSpeed.ts`, `statsSort.ts`, `timeRange.ts` (custom ranges and the day presets via `presetDayRange`), `multiDeviceScan.ts`, `network/networkRate.ts`, `subscriptions/*.ts` each have a matching test in `tests/`. `insights/reportBuilder.ts` generates both standalone offline HTML reports (with inline SVG charts and hover tooltips) and Markdown reports without external dependencies.
- **Styling**: `styles.css` (base), `usage/usage.css`, `insights/insights.css`, `markets/markets.css`, `theme/ccSwitchTheme.css` (colors from CC Switch), `theme/redesign.css` (sidebar shell), and `theme/widget.css`, the widget-style theme imported last in `main.tsx` so it overrides the rest. Light/dark follows the system and can be toggled; the resolved choice is set as `html[data-theme]` and passed to the native title bar.
- **Form controls**: the macOS app renders in WebKit, which ignores height and padding on `<select>` elements that keep the native look. `widget.css` therefore sets `appearance: none` on every `<select>`, gives it a 32 px pill height and draws a chevron matching `AppSelect` (one SVG per theme). The multi-device `<details>` picker uses the same height and chevron. Check control sizes in the app, not only in a Chromium preview, where native selects do honor the CSS.
- **Local storage** holds only view preferences (sidebar state, theme, last filters, Insights budget thresholds, the selected Markets pool and view).
- **Markets** queries poll with `refetchInterval` only while the document is visible. Config saves update the query cache optimistically (and roll back on error) so quick successive edits build on each other.

## 11. Testing

| Command | Covers |
| --- | --- |
| `npm run typecheck` | TypeScript |
| `npm run lint` | ESLint (TypeScript + React hooks rules) |
| `npm run format:check` | Prettier |
| `npm test` | Frontend logic tests in `tests/*.test.mjs` |
| `npm run native:test` | Engine unit tests and `native/tests/` integration tests: CC Switch import and dedup, pruning baselines, cache semantics, pricing backfill scope, subscriptions, pairing codes, LAN hardening, two-instance sync, the HTTP API and the Postman collection |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Window close behavior of the shell |

Tests that touch process-wide environment variables use `serial_test`.

## 12. Build and Release

- `npm run desktop:dev` runs the Tauri app against the Vite dev server.
- `npm run desktop:build` builds a debug executable without installers; `desktop:release` builds an optimized one.
- `npm run mac:install` (`scripts/mac-install.mjs`) deletes any leftover bundle, builds the macOS `.app` bundle, installs it to `/Applications/Observatory.app` with `ditto`, and removes the temporary build bundle so Launchpad and Spotlight only index one copy. `tauri build` exits non-zero locally when updater signing keys are missing, so a freshly produced bundle is the success signal; if none is produced, nothing is installed.
- Pushing a `v*` tag runs `.github/workflows/release.yml`: it creates one draft release, builds the Windows x64 NSIS installer plus a portable `.exe` and the macOS Apple Silicon DMG, signs updater artifacts, runs lint and tests in parallel, and publishes `latest.json` only when checks pass. The tag must match `package.json`'s version.
- Version numbers live in `package.json`, `native/Cargo.toml`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`; keep them in sync.
- The updater checks `releases/latest/download/latest.json` on GitHub at startup and every six hours.
- Changes under `packages/` publish the promo page through `.github/workflows/pages.yml`.

## 13. Provenance

The collectors, statistics, pricing and parts of the LAN code come from [CC Switch](https://github.com/calabli/cc-switch) (MIT, license in `licenses/CC-Switch-MIT.txt`). `native/migration-manifest.json` records each migrated file's upstream path, the upstream hash at migration (`sha256`) and the hash of the current adapted copy (`currentSha256`); refresh `currentSha256` when you change one of those files. The Windows installer template derives from Tauri CLI 2.10.1's NSIS template. The ZCode icon's source is documented in `src/assets/zcode.SOURCE.md`.
