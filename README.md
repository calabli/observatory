# Observatory

Observatory (观测台) is a local-first desktop app for people who use several AI coding tools on several computers. It reads the session logs that Claude Code, Codex, Gemini CLI and other tools already write, and shows requests, tokens, cache hit rate and estimated cost in one place, for one machine or for all of your paired devices. Data stays in a local SQLite database; no account or cloud service is involved.

Built with Tauri 2, React, TypeScript, Rust and SQLite. Releases are published for Windows x64 and macOS on Apple Silicon.

## Features

- **AI usage**: cost, requests, four token categories, cache hit rate, trends, per-app/provider/model breakdowns, request details and CSV export, filtered by time, app, provider and model.
- **11 native collectors**: Claude Code, Codex, Gemini CLI, Grok Build, OpenCode, Pi, MiniMax Code, Antigravity, Kimi Code, DeepSeek Harness, ZCode. Scans are incremental and never double count.
- **Pricing**: editable per-model prices, models.dev import and auto-sync, recalculation of costs for records that were missing a price.
- **Subscriptions & quota**: usage per weekly quota cycle next to what you pay.
- **Multi-device**: pair devices on the LAN with a one-time code and see combined usage. Off by default.
- **Read-only HTTP API** for dashboards and scripts, authenticated with an API key.
- **Network overview**: adapters, addresses, gateway, DNS and live transfer rates.
- **Activity log**: what each scan read, who called the local service, device syncs and key actions.
- **Tray / menu-bar panel** with today's totals; the app keeps collecting after the window is closed.

## Documentation

| Document | Contents |
| --- | --- |
| [docs/product.md](docs/product.md) | Product guide: pages, data sources, metric definitions, multi-device rules, limits, roadmap |
| [docs/architecture.md](docs/architecture.md) | Technical architecture: processes, engine, data model, collection pipeline, LAN service, testing, release |
| [docs/read-only-usage-http-api.md](docs/read-only-usage-http-api.md) | HTTP API reference, with a Postman collection in `docs/examples/` |
| [docs/subscriptions-and-quota.md](docs/subscriptions-and-quota.md) | How subscription cycles and quota queries work |

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
```

Builds: `npm run desktop:build` (debug executable) and `npm run desktop:release` (optimized). Installers are produced by the release workflow when a `v*` tag is pushed; see [docs/architecture.md](docs/architecture.md#12-build-and-release).

## Privacy

Observatory reads usage metadata only. It never imports or exports API keys, provider configuration or conversation contents. LAN sharing is off by default; when enabled it uses plain HTTP, so only turn it on in trusted networks.

## Sources and Licensing

MIT. The collectors and statistics code are adapted from [CC Switch](https://github.com/calabli/cc-switch) (MIT); its license is kept in `licenses/CC-Switch-MIT.txt` and per-file provenance is recorded in `native/migration-manifest.json`. Network adapter collection uses `netdev` (MIT). The Windows installer template is based on the Tauri CLI 2.10.1 NSIS template (MIT or Apache-2.0). The ZCode icon's source is documented in `src/assets/zcode.SOURCE.md`.
