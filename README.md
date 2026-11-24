# Observatory

Observatory (观测台) is a local-first desktop app for people who use several AI coding tools on several computers. It reads the session logs that Claude Code, Codex, Gemini CLI and other tools already write, and shows requests, tokens, cache hit rate and estimated cost in one place, for one machine or for all of your paired devices. Data stays in a local SQLite database; no account or cloud service is involved.

Built with Tauri 2, React, TypeScript, Rust and SQLite. Releases are published for Windows x64 and macOS on Apple Silicon.

## Features

- **AI usage**: cost, requests, four token categories, cache hit rate, interactive trends, per-app/provider/model breakdowns, request details and CSV export, filtered by time, app, provider and model.
- **Insights & Agent tracing**: prompt cache savings, 30-day cost forecast with daily/monthly budget alerts, subscription ROI gauge, 7×24 coding rhythm heatmap, project/repository cost ranking, searchable sessions explorer with one-click resume commands, on-demand Agent single-task call-chain waterfall (User Task → LLM → Tool/Subagent timing and cost attribution), and offline visual HTML / Markdown report preview and export.
- **11 native collectors**: Claude Code, Codex, Gemini CLI, Grok Build, OpenCode, Pi, MiniMax Code, Antigravity, Kimi Code, DeepSeek Harness, ZCode. Scans are incremental and never double count.
- **Pricing**: editable per-model prices, models.dev import and auto-sync, recalculation of costs for records that were missing a price.
- **Subscriptions & quota**: usage per weekly quota cycle next to what you pay.
- **Multi-device**: pair devices on the LAN with a one-time code and see combined usage. Off by default.
- **Read-only HTTP API** for dashboards and scripts, authenticated with an API key.
- **Network overview**: adapters, addresses, gateway, DNS and live transfer rates.
- **Model benchmarks**: Artificial Analysis rankings across nine dimensions with price and speed charts, effort-tier comparison, and your locally measured speed and spend alongside (needs your own API key).
- **Markets**: real-time quotes for US, Taiwan, mainland China and Hong Kong stocks and global indices, built-in stock pools with market-cap treemap heat maps, detail charts and cross-market search.
- **Activity log**: what each scan read, who called the local service, outbound requests to outside services, device syncs and key actions.
- **Tray / menu-bar panel** with today's totals; the app keeps collecting after the window is closed.

## Documentation

| Document                                                             | Contents                                                                                                  |
| -------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| [docs/product.md](docs/product.md)                                   | Product guide: pages, data sources, metric definitions, multi-device rules, limits, roadmap               |
| [docs/architecture.md](docs/architecture.md)                         | Technical architecture: processes, engine, data model, collection pipeline, LAN service, testing, release |
| [docs/read-only-usage-http-api.md](docs/read-only-usage-http-api.md) | HTTP API reference, with a Postman collection in `docs/examples/`                                         |
| [docs/subscriptions-and-quota.md](docs/subscriptions-and-quota.md)   | How subscription cycles and quota queries work                                                            |
| [CHANGELOG.md](CHANGELOG.md)                                         | Release notes for every version                                                                           |

## Development

Requirements: Node.js 20.19+ or 22.12+, Rust stable, and your platform's build tools (on Windows: Visual Studio C++ Build Tools, the Windows SDK and WebView2).

```sh
npm install
npm run native:build   # build the engine binary used by the browser dev server
npm run dev            # http://127.0.0.1:5188/
```

The browser dev server talks to the Rust engine through a loopback-only bridge. To run the real desktop shell instead:

```sh
npm run desktop:dev
```

The dev server and the desktop app share the data directory (`~/.observatory/`, or `OBSERVATORY_HOME`), so run only one at a time.

Checks:

```sh
npm run typecheck && npm run lint && npm run format:check
npm test
npm run native:test
npm run test:ui        # browser tests in the installed Chrome; needs npm run native:build
```

Builds: `npm run desktop:build` (debug executable), `npm run desktop:release` (optimized), and `npm run mac:install` (macOS local install to `/Applications/Observatory.app` with temporary bundle cleanup). To release, add notes under `## Unreleased` in `CHANGELOG.md`, run `npm run version:bump -- <x.y.z>` (it updates every version entry and turns those notes into the new version's section), commit and push a `v<x.y.z>` tag; the release workflow builds the installers and publishes them with those notes. See [docs/architecture.md](docs/architecture.md#12-build-and-release).

## Privacy

Observatory stores usage metadata only in `observatory.db`. It never imports or exports API keys, provider configuration or conversation contents. On-demand session drill-down (titles, project directories, git branches and Agent tool-call trace waterfalls) reads local session logs directly from disk without persisting conversation bodies to the database. LAN sharing is off by default; when enabled it uses plain HTTP, so only turn it on in trusted networks. The only requests to outside services are for public data: market quotes, charts and search (Tencent, TWSE, Yahoo Finance), daily exchange rates (ExchangeRate-API), models.dev prices, the egress IP lookup (ipify), and model benchmarks from Artificial Analysis (with your own API key, sent only in a request header and stored only in the local database), made while the page that needs them is open (models.dev also by the optional automatic price sync) and logged in the Activity Log. They carry stock symbols or search text, never usage data.

## Sources and Licensing

MIT. The collectors and statistics code are adapted from [CC Switch](https://github.com/calabli/cc-switch) (MIT); its license is kept in `licenses/CC-Switch-MIT.txt` and per-file provenance is recorded in `native/migration-manifest.json`. Network adapter collection uses `netdev` (MIT). The Windows installer template is based on the Tauri CLI 2.10.1 NSIS template (MIT or Apache-2.0). The ZCode icon's source is documented in `src/assets/zcode.SOURCE.md`.
