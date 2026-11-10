# Observatory

A standalone Tauri 2 + React + TypeScript + Rust + SQLite desktop app. AI usage and the local network overview are backed by real data; device performance, DNS diagnostics, and the agent call stack are not connected yet.

## Local Network Overview

Shows the local network adapters, IPv4/IPv6, status, gateway, per-adapter DNS, and transmit/receive rates. After entering the network overview, counters are sampled every 3 seconds, and sampling pauses when the window is not visible or the page is left; adapter configuration refreshes every 30 seconds and can also be refreshed manually. The chart keeps only the most recent 60 valid rate samples for the current page, and neither persists them nor shares them automatically. Rates are not computed for the first sample, after an adapter switch, after a counter reset, or when the interval is too long.

The adapter that the system infers as recommended may differ from the intended path because of a VPN or split routing, so it can be selected manually. Adapter status does not indicate internet availability; fields that cannot be obtained are shown as not retrieved. The international egress IPv4 is requested from ipify when the user clicks, showing the egress and the detection time of that request path; it is not bound to the selected adapter and is not queried automatically. Region information, gateway latency, DNS resolution, and application connections will be connected in a later phase. When the system collects gateway information it may resolve the neighbor address of the local gateway. Cross-platform adapter collection uses netdev, under the MIT license.

## Interface Style

Follows CC Switch's black/white/gray hierarchy, orange action color, and cyan charts, while keeping the standalone app's top navigation. The AI usage metric bar can switch directly between cost, requests, tokens, and cache hit rate, and trends support both bar and line charts. Theme values are centralized in `src/theme/ccSwitchTheme.css`, sourced from CC Switch's `src/index.css` and component styles.

## Tray and Menu Bar

The Windows taskbar notification area and the macOS menu bar provide the Observatory icon. Left-clicking opens a quick panel that switches between the local machine, enabled remote devices, or all devices, as well as today or the last 7 days; the panel shows cost, requests, tokens, cache hit rate, success rate, and per-application usage. Remote data uses the most recent snapshot stored locally, and the panel shows the sync time and any failure status. While the panel is visible, statistics are read every 30 seconds without triggering additional scans. Right-clicking keeps the native menu for today's local usage, opening, and quitting.

Clicking the main window's close button hides the window and keeps the app running in the background; it can be reopened or quit from the tray panel or the right-click menu. If the system tray or the panel cannot be created, closing the main window still quits the app.

## Implemented AI Usage

- Local and multi-device aggregation; filtering by time, custom date, application, provider, and model.
- Requests, cost, four categories of tokens, cache hit rate, trends, call activity, and provider and model statistics.
- Request pagination, status code filtering, details, and CSV export of the current page; exact durations are distinguished from log-estimated durations.
- An independent persistent database, collection cursors, and long-term deduplication records; repeated scans do not double count.
- Detail records are kept for 30 days: older ones are automatically retained as daily rollups, while session-level details, exact durations, and output speed only cover the retention period; the deduplication ledger stays valid long term, so pruned entries are not double counted when logs are replayed.
- Editing and deleting the four categories of model prices, backfilling missing costs, models.dev import, and automatic sync configuration. Checked at startup, with at least six hours between successful syncs.
- LAN device discovery, manual connection, per-application sharing, snapshot replacement, keeping the cache for disabled devices, and deleting the copy on disconnect. Remote costs are not recomputed with local prices.

### Data Sources

Directly reuses CC Switch's log collectors: Claude Code, Codex, Gemini CLI, Grok Build, OpenCode, Pi, MiniMax Code, Antigravity, Kimi Code, DeepSeek Harness, ZCode.

Claude Desktop, Hermes, and OpenClaw are currently integrated through CC Switch history-statistics import or peer sharing, and do not have dedicated log collectors yet. Turning a collection switch off stops subsequent scans without deleting the existing history.

## Data Directory and History Migration

The default directory is `~/.observatory/`, containing `observatory.db`, `model-pricing.json`, and an instance lock. `OBSERVATORY_HOME` can point to a separate directory.

On the first launch, if this database holds no statistics and `~/.cc-switch/cc-switch.db` is found, a read-only history migration runs first and native log collection starts afterwards. Only statistics details, daily rollups, cursors, and deduplication records are copied; providers keep only their names, with configuration set to empty objects, and no keys or conversation bodies are imported.

Daily rollups establish the historical baseline only during the first migration. Later repeated imports add only the missing details and do not stack the daily rollups that the source database produced through later pruning. If the target has already collected on its own and the source contains daily rollups without a migration baseline, the ambiguous merge is rejected; keep the original database and use a new `OBSERVATORY_HOME` to migrate first and collect afterwards. History migration is a one-time baseline, not a live mirror of the CC Switch database.

The source database can be set through `CC_SWITCH_DB`, or by entering the path under "AI Usage → Data Sources". Records from other devices should be integrated through LAN sync.

Log paths follow each tool's conventions and honor the original environment variables, such as `CODEX_HOME`, `DSH_HOME`, `ZCODE_DB`, `MINIMAX_DATA_DIR`, `MAVIS_DATA_DIR`, `KIMI_CODE_HOME`, `PI_CODING_AGENT_DIR`, `OPENCODE_DB`.

## LAN Sharing

Disabled by default. Observatory uses TCP/UDP 17891 by default to avoid conflicting with CC Switch's default 17890, and scanning supports both ports. When connecting to CC Switch manually, enter `IP:17890`.

Once sharing is enabled, other devices must pair before they can read the statistics of the selected applications: the local machine generates a 6-digit one-time pairing code (valid for 5 minutes, at most 5 attempts), the peer enters it when connecting, and the two sides then exchange access tokens (to verify a peer's token, only its SHA-256 digest is stored; the token this machine uses to access a peer is kept in the local database). Requests from unpaired or revoked devices are rejected, and revocation can be performed at any time in the authorization list. The transport is still unencrypted HTTP, so tokens and statistics travel over the LAN in plaintext — enable sharing only on trusted networks. API keys, provider configuration, and conversation bodies are never exported.

## Activity Log

"Activity Log" in the top navigation records four kinds of activity in reverse chronological order; click a row to expand its details:

- **File scan**: how each local scan round was triggered, the number of files scanned per source, the number of new records, and tokens; when expanded, one file per line showing the path, source, file size, modification time, the bytes read and the read offset for this round, the number of new records, and tokens. Gemini CLI, Antigravity, and ZCode do not record per-file progress and only show a source summary. Background scheduled scans are only logged when new content is read or an error occurs.
- **External call**: every HTTP request that reaches the local LAN service (peer sync, pairing, the read-only usage API, etc.), including the caller, source address, status code, duration, and response size. Only the kind of credential the request carried is recorded, never the token or API key itself.
- **Device sync**: the outcome of each pull from a paired device — skipped because the fingerprint matched, download size and rows written, or the reason for the failure. The per-round fingerprint query and unchanged syncs on the peer are hidden by default and can be shown on the page.
- **Action**: key operations and their outcomes, such as scans, syncs, connecting/disconnecting devices, changing settings and prices, generating API keys, and pulling models.dev prices. Parameters such as pairing codes, tokens, and keys are shown as "(hidden)"; query-type requests, pagination, and tab switches are not logged.

Logs are stored only in the local database and are cleaned up automatically after 7 days; logging does not affect the statistics cache or LAN sync.

## Development

Requires Node.js 20.19+ or 22.12+, Rust stable, and the platform build toolchain. On Windows, Visual Studio C++ Build Tools, the Windows SDK, and WebView2 are required.

```sh
npm install
npm run native:build
npm run dev
```

The browser development entry point is `http://127.0.0.1:5188/`. Vite listens only on the loopback address and calls the Rust subprocess through a same-origin session token; the production desktop app uses Tauri IPC directly and does not start a Node service.

```sh
npm run desktop:dev
npm run desktop:build
```

`desktop:build` produces a development EXE with the frontend assets embedded; `desktop:release` produces an optimized build. The browser development server and the desktop app share the same data directory, so use only one of them at a time. After changing Rust code on Windows, stop the development service that is using the corresponding engine before rebuilding.

```sh
npm run typecheck
npm test
npm run build
npm run native:test
```

The application-level unit tests from the original statistics code remain in the source tree, while the standalone core uses `native/tests/usage_integration.rs` to verify the migration contract: read-only import and deduplication, pruning baselines, cache semantics, DSH concatenated compressed frames, ZCode durations and long-term deduplication, HTTP dual-instance snapshots, pagination, and price backfill.

## Sources and Licensing

The migration manifest is in `native/migration-manifest.json`. CC Switch's MIT license and the original source comments are retained: `licenses/CC-Switch-MIT.txt`. The migration adaptation mainly involves the data directory, the minimal database, event notification, the Tokio runtime, and the application entry point.

The ZCode icon comes from the official desktop installer; its provenance is documented in `src/assets/zcode.SOURCE.md`.

The Windows installer template is based on the NSIS template from Tauri CLI 2.10.1 (MIT or Apache-2.0 licensed); the default install directory is changed to `Observatory`, while upgrade detection for existing install paths is preserved.
