# Observatory Product Plan

> Document version: 1.0 · Discussion draft  
> Authored: 2026-10-28  
> Corresponding product baseline: local Windows development build 0.1.3  
> Audience: product owners, developers, designers, and early testers  
> Repository: https://github.com/calabli/observatory

This document is based on the existing source code, the project notes, and confirmed requirements. Anything marked "planned" does not imply that it has been shipped; phase durations, performance targets, and business direction are assumptions pending validation.

## 1. Product Overview and Core Decisions

Observatory is a local-first desktop observability tool that helps users who rely heavily on AI coding tools and own several computers view AI consumption, device runtime status, network state, and agent execution in a single workbench.

The product first answers "how much was used, where did the money go, which device did it come from, and is the data trustworthy", and then answers "why is it slow, which stage failed, and is it related to the device or the network". Over time it forms a troubleshooting path from cost to call, and from call to the runtime environment.

The most valuable foundations the product currently has are AI usage collection and multi-device aggregation, and these two should be made accurate and stable first. Performance, network, and the call stack are connected to real data incrementally, so that a design preview is never mistaken for working monitoring.

### 1.1 Confirmed Product Decisions

| Decision | Product requirement |
| --- | --- |
| Standalone desktop app | Evolves independently from CC Switch's statistics capabilities, with its own data directory and lifecycle |
| Brand name | Observatory |
| Layout | Keeps the compact top navigation and reduces window hierarchy and duplicate titles |
| Visual style | Follows CC Switch: black/white/gray hierarchy, orange for actions, cyan for charts, with light and dark themes |
| Local and multi-device usage | Switched through tabs on the AI usage page, with identical filters, metrics, tables, and refresh behavior |
| Controlled sync | Sharing can be enabled per application; the application sharing list is checked by default |
| Master switch default | LAN sharing is off by default; checking an application does not mean sharing is already exposed to the network |
| Harness attribution | DeepSeek Harness is displayed and aggregated independently in the source filter |
| Icons | Real application icons are used when a reliable source exists, and options stay consistent with the selected item |
| Data independence | Reads the original tools' data without depending on modifying their configuration to produce basic statistics |

### 1.2 Product Principles

1. **Metrics must be explainable.** Every number should state its source, how it is computed, its time range, and its freshness.
2. **Works locally.** Single-machine statistics, history queries, and basic diagnostics do not depend on an account or a cloud service.
3. **Progressive disclosure.** The home page gives the conclusion; details then present tokens, source quality, and the diagnostic process.
4. **Low-overhead residency.** Background collection should have a budget, a pause mechanism, and error backoff.
5. **Honest capabilities.** Not connected, no permission, offline, and no data are shown separately; no values are fabricated.
6. **Reuse proven work.** Keep the provenance and license of CC Switch-related code and reuse its validated collection and statistics logic.

## 2. Users, Scenarios, and Requirement Assumptions

The primary scenario today is individual developers with a small, trusted set of devices. The profiles below are used to set priorities and have not been validated through user interviews or market samples.

| User type | Typical environment | Most important question | Successful outcome |
| --- | --- | --- | --- |
| Multi-tool AI developer | Uses Codex, Claude Code, DeepSeek Harness, ZCode, and others at the same time | Consumption is scattered across tools, model names are inconsistent, and costs are hard to reconcile | Traceable usage and cost estimates on a single page |
| Multi-device user | Switches between a Windows desktop and a Mac laptop | Device discovery is asymmetric, syncs are missed, or records are double counted | Reliable aggregation under the same filter, with clear visibility of which devices have fresh data |
| Local run-and-debug user | Runs an IDE, agents, builds, and several proxies in parallel | When AI slows down, device load, network, and upstream latency cannot be told apart | Correlated evidence along a timeline to narrow the troubleshooting scope |
| Agent workflow developer | A single task spans multiple models, tools, and subtasks | The call sequence is opaque, and failures and costs cannot be attributed | Tracing from a single task down to specific steps and their metrics |

### 2.1 Representative Tasks

- Which models were used today? Where did the cost increase come from — which application and which device?
- Are the DeepSeek Harness call counts and tokens attributed correctly?
- What is the total across the two devices? Up to what time does the offline computer's data go?
- Why is one model call shown as unknown? Is the source log missing, or is the parser not yet supported?
- For a long ZCode request, is the first output slow, or is the whole generation slow?
- A Mac can connect to Windows, but Windows cannot discover the Mac — what should be checked next?
- At which step did a given agent task fail? How much had already been consumed before the failure?

### 2.2 Out of Scope for Now

Early versions do not take on AI gateway routing, key custody, provider failover, enterprise billing settlement, employee behavior auditing, remote control, or general-purpose operations platform duties. The network module does not treat scraping application bodies, automatically modifying system DNS, or taking over proxies as core functionality.

Observing call behavior is not the same as executing agents, and establishing a temporal correlation between resources and the network is not the same as being able to prove the root cause automatically.

## 3. Current Capability Inventory

### 3.1 Implemented and Boundaries

| Module | Current status | Existing capabilities and limits |
| --- | --- | --- |
| Local AI statistics | Real data | Filtering by time, application, provider, and model; cost, requests, four categories of tokens, hit rate, trends, and detail records |
| Multi-device usage | Real data | Device discovery, manual connection, per-application sharing, persisted snapshots, and aggregation |
| Model pricing | Real data | Editing the four price categories, import, automatic sync settings, and backfilling missing costs |
| History migration | Real data | Read-only import of CC Switch's statistics, cursors, and deduplication records, with protection against ambiguous merges |
| Application menu icons | Complete | Reuses existing brand assets; a unified terminal icon is used when a dedicated asset is missing |
| Overview | Partly real, partly preview | The presentation of newly added modules still has to be replaced item by item with real data |
| Device performance | Design preview | Does not yet provide complete real sampling, persistence, or history queries |
| Network diagnostics | Design preview | Does not yet provide complete real IP, DNS, and link probing |
| Agent tracing | Design preview | Has not yet produced a real call tree or cross-event correlation |

### 3.2 Application Integration Tiers

| Tier | Applications | Current meaning |
| --- | --- | --- |
| Dedicated collection | Claude Code, Codex, Gemini CLI, Grok Build, OpenCode, Pi, MiniMax Code, Antigravity, Kimi Code, DeepSeek Harness, ZCode | Log collectors have been reused; the actual coverage still depends on tool versions, log availability, and field completeness |
| Import or peer sharing | Claude Desktop, Hermes, OpenClaw | Imported or shared records can be displayed; there is no dedicated log collector today |
| Not yet adapted | Other tools | Shown as unsupported; they can be integrated through later adapters or a standard event entry point |

"Selectable in the application list" must not be described as "directly supported for collection". The integration status has to be shown clearly on the data sources page.

### 3.3 Key Current Limitations

- 0.1.3 is a Windows x64 development build; it cannot be used to claim a stable release or a verified cross-platform official release.
- The current LAN protocol has no pairing authentication; any device that can reach the shared port can read the metrics of the selected applications. The new pairing mechanism is a key building block before wider distribution.
- Costs are mainly used for estimation and trend analysis, and are not the amount actually paid on the provider's invoice.
- Some durations come from log-time estimates and must not be promoted alongside exact measurements.
- CSV currently exports the current page, not the full filtered result set.
- A CC Switch import establishes a historical baseline; it is not continuous two-way database sync.
- Persistent multi-device snapshots are in place, but that does not mean the latest records of every device have fully arrived.

## 4. Product Information Architecture

```text
Observatory
├─ Overview
│  ├─ Today's Usage and Recent Trends
│  ├─ Devices and Sync Status
│  └─ Anomaly Notices and Recent Tasks (integrated incrementally)
├─ AI Usage
│  ├─ Local Usage
│  ├─ Multi-Device Aggregate
│  ├─ Request Details / Detail / Export
│  ├─ Price Management
│  └─ Data Sources and Statistical Semantics
├─ Agent Tracing
│  ├─ Task List
│  └─ Call Tree / Timeline / Step Details
├─ Device Performance
│  ├─ Live Resources
│  └─ Historical Trends / Limited Process Information
├─ Network Diagnostics
│  ├─ Network Overview: Interfaces, IP, Gateway, DNS
│  ├─ DNS and Connection Diagnostics
│  └─ Diagnostic History
└─ Multi-Device
   ├─ Discovery, Pairing, and Connection
   ├─ Sharing Scope and Sync Status
   └─ Device Disablement, Disconnection, and Data Management
```

General settings are reachable through a fixed entry point, covering appearance, refresh, collection, data directory, backup, and about. Price management and data sources stay close to the AI usage action area, to avoid adding top-level navigation for secondary settings.

### 4.1 Filters and Device Context

The local and multi-device tabs of AI usage share the same filter semantics for time, application, provider, model, and status. The new device filter only determines which devices are included, and does not change the meaning of the other filters.

Device performance and network diagnostics show one device at a time. Until remote module sharing exists, only the local machine can be selected; it is not acceptable to merely change the title while showing the same local data.

When the application filter changes, the existing behavior of clearing the provider and model is retained, to prevent invalid combinations from causing misreadings. Every filter change resets the detail page number, while a refresh does not reset the filters.

Target time semantics: event time is stored as a unified timestamp; "today" and date ranges are interpreted by default in the time zone selected on the viewing side, and pages, trends, and exports use that same time zone. Date ranges include the start and exclude the end, are divided by calendar day in that time zone, and account for daylight saving time. Historical data that has only daily rollups and no event details keeps its original aggregation time zone and dates; where an exact conversion is impossible, this is stated separately rather than forcing the data into another calendar day. Whether existing data retained this metadata has to be determined by the M1 audit and then migrated.

## 5. AI Usage Module Plan

### 5.1 Page Layout

The page consists of scope tabs, a filter area, sync and collection status, a metric bar, token composition, trends, application distribution, and a request table. The layout stays continuous with the existing CC Switch style.

The metric bar keeps four primary entry points: estimated cost, request count, total tokens, and cache hit rate. Clicking a metric switches the trend, and the other components continue to obey the same filter. When the table has many rows, the region may scroll horizontally, but the main page must not overflow horizontally.

### 5.2 Metric Definitions

| Metric | Product definition | When it cannot be computed |
| --- | --- | --- |
| Request count | Call records deduplicated by source within the current filter; historical rollups may have only counts and no per-record details | Distinguish 0 from an unreadable source |
| Total tokens | New input + output + cache read + cache write; normalized according to each source's input semantics | Mark unrecorded fields as missing; never fill unknowns with a definite 0 |
| Cache hit rate | Cache read ÷ (new input + cache read + cache write) | Show "—" when the denominator is 0 |
| Cost estimate | The priceable portion is computed from the four token categories and the effective unit prices, keeping the origin of the amount | Mark the unpriced portion as "not priced" and show the computable portion alongside it |
| Success rate | Successful requests with a known status ÷ requests with a known status; status coverage is shown | Do not show 100% when all statuses are unknown |
| Time to first token | From the start of the request to the first output, distinguishing exact values from log-estimated values | Show "not recorded" when there is no reliable basis |
| Request duration | From the start of the request to the end; excluding data that cannot be correlated | Keep the estimated flag |

The table above is the target product semantics. Existing collectors may not express missing fields and statuses entirely consistently, so a field audit must come first, followed by item-by-item migration; satisfying the requirements cannot be claimed by merely changing frontend labels.

When only some of the four token categories are known, show a "known token subtotal" together with the missing-field/record coverage, and do not call it a complete total. When the cache denominator is incomplete, do not emit a definite hit rate; for mixed data sets it is acceptable to compute only over the requests with complete fields, while also showing their coverage. Details, aggregates, and exports all preserve this quality meaning.

### 5.3 Statistical Trustworthiness and Unknown Models

Each request target retains: source application, source device, raw model name, canonical model name, pricing model name, time, token fields, cost origin, precision, and parser version.

Model resolution order: the request-level model field is preferred; if it is absent, a response field that can be reliably correlated to the same request is read; only if it is still missing is session metadata with an explicit correlation used. Historical request models must not be inferred solely from that provider's default configuration.

Unrecognized records are given one of the following reasons: not provided by the source log, model alias not matched, log format not yet supported, or field missing from the history migration. Users can see the raw fields and submit redacted samples; manual alias mapping is allowed, but the original value and the change record must be preserved.

Unknown models still count toward confirmable requests and tokens. An unknown price must not make an entire request disappear from the statistics. The model recognition rate and the price coverage are shown separately, to avoid equating "has a model name" with "the cost is accurate".

### 5.4 Pricing Maintenance

The target is an auditable priority order: explicit user overrides > verified price sources > the current built-in defaults. Sync jobs must not overwrite manually locked prices.

Price management shows the origin, the last sync, the billing unit, and the effective information. Remote records keep the peer's amounts and pricing basis; the receiving side does not recompute automatically.

A "price version" concept and an explicit recalculation action need to be added. By default, price updates affect subsequent new records and records that genuinely lack a price; historical records that already carry an amount are not repriced silently. When a user triggers a recalculation, the scope and the estimated impact are shown first, the recalculated version is saved, and restoring the previous result is supported.

Subscriptions, promotional credits, plan discounts, and taxes affect the actual invoice. Early versions only present estimates and do not promise invoice reconciliation. Multiple currencies keep their original currency for now; amounts are never added across currencies without an explicit exchange-rate source and date.

The current cost implementation is based on USD. If other currencies are added, currency filtering and grouped amounts come first; the cost trend shows one currency at a time, and anomaly comparisons are limited to the same currency and the same valuation method. Until those capabilities exist, other currencies must not be merged directly into USD metrics.

The target distinguishes the "amount recorded by the source" from the "estimated amount computed from tokens". When the source is explicit and the currency is known, the recorded amount takes precedence as the default statistic; only when it is missing is the computed estimate used, with the basis shown. When the two disagree, the difference is preserved, and the source amount still does not equal the actual invoice. A historical recalculation only produces an estimated version and does not overwrite source amounts; when another valuation method is selected, the metrics, trends, details, and exports must switch together, and must not be mixed and then compared.

### 5.5 Details, Export, and Long-Term Data

Keep the current-page export and add an "export filtered results" action that runs in the background in batches, showing the count, the progress, a cancel option, and the failure reasons. Large exports must not load all records at once on the UI thread.

Details show the full statistical fields and the source quality, and do not store prompts, response bodies, keys, or raw tool parameters by default. When a historical rollup cannot be restored to per-record details, the difference between the coverage of the rollup and the number of detail records is stated explicitly.

The long-term retention policy has to distinguish details from rollups. Before deleting details, generate a rollup and verify that the totals match; cleanup must not delete the long-term deduplication basis, so that the next scan does not re-accumulate old logs.

## 6. Multi-Device Sync and Management

### 6.1 Target Experience

Two computers each install Observatory and each collect their own local usage. After the user enables sharing and authorizes the other device, the authorized device's cached aggregate can be viewed from either side; an offline device keeps the result of its last successful sync and shows the data cutoff time.

The multi-device aggregate is an aggregation of snapshots with clear provenance, not several endpoints writing to the same SQLite file. The receiving device does not share the remote copy again, which avoids cyclic accumulation.

The local machine is included in the multi-device aggregate by default and counted only once; "N devices included" counts the local machine. Once the planned device filter is added, it will be possible to explicitly exclude the local machine; when every device is excluded, show "no device selected" instead of showing local data. A local copy with the same device identity must not be included again through a remote connection.

### 6.2 Three Independent Switches

| Control | Effect | Default |
| --- | --- | --- |
| Local collection | Whether to keep reading a given application's logs | Enabled by default for supported sources, preserving the current behavior |
| Share outward | Whether authorized devices may obtain this application's local statistics | The master sharing switch is off; all applications in the list are checked |
| Include in the aggregate | Whether a connected device is included in the current aggregate | Enabled once the connection completes, and can be disabled at any time |

Turning collection off preserves the history; cancelling sharing blocks any further outward provision and propagates the scope change to paired, reachable receivers; disabling a remote device keeps its cache but excludes it from the aggregate. When a device is disconnected, the deletion of the local copy is explained and executed.

Cancelling sharing on the sending side alone cannot guarantee that an offline receiver immediately deletes the data it has already obtained. The pairing protocol should provide a delete command with a confirmation state, and the UI should explain the boundary of historical copies.

The future performance, network, and trace modules use separate module-level sharing permissions, are off by default, and do not inherit the "the AI application list is fully checked by default" behavior. Process command lines, network history, and tool parameters are not shared automatically with the statistics.

### 6.3 Discovery and Connection

Keep both entry points: automatic discovery and manual IP:port. When device discovery fails, show the following steps: the local machine's active adapters, the target address, the shared service's status, TCP reachability, protocol compatibility, and the most recent failure reason.

Discovery has to cover wired, wireless, and multi-adapter environments, and must avoid mistakenly treating a proxy or a virtual adapter as the only egress. Diagnostics only suggest possible causes based on the results observable in the current attempt, and never assert directly that "not discovered" is a firewall problem.

### 6.4 Pairing and Trust (Planned, P0)

Add a device identity, one-time pairing codes, a persistent trust list, and a revocation entry point. A formal pairing protocol must address device authentication, transport protection, replay protection, and token custody together; it cannot merely add a reusable plaintext passphrase.

The new protocol is handled separately from the existing CC Switch-compatible interface. The old protocol becomes a compatibility mode that the user must explicitly enable, is labeled "no pairing authentication", and is not treated as a trusted device by default. A failed pairing never automatically downgrades to an unauthenticated connection.

### 6.5 Sync Consistency

A target snapshot contains the source device identifier, the database generation, an incrementing version, the generation time, the covered time period, the sharing scope, the protocol version, and content checksum information. Applying a snapshot has to be transactional, keeping the old snapshot on failure; showing a partially updated snapshot is forbidden.

Importing the same snapshot repeatedly does not increase the totals, and an older version must not overwrite a newer one. A reinstall, a backup restore, or a database rollback must be recognized as a generation change, offering a comprehensible choice between replacing and keeping.

Duplicate collection across applications and logs copied across devices are a different kind of duplication risk. Without a stable source record identifier, records are not automatically deduplicated because they have a "similar time and the same tokens". The first release surfaces possible overlaps and allows excluding devices or sources; statistical deduplication and the attribution of the same business task are separate concerns.

### 6.6 Multi-Device Page States

Each device shows: name, OS, connection method, authorized scope, last seen time, last successful sync, data cutoff time, cache size, and whether it is included in the aggregate.

"Device online", "sync succeeded", and "data is fresh" are three different states. For example, when a device is online but the local collection has not updated, the sync completion time must not be used as the time the usage occurred. The summary area shows "N devices included / M devices with stale data"; the staleness threshold is determined by the refresh policy.

## 7. Device Performance Module

### 7.1 First-Phase Metrics

| Category | First-phase content | Explicit boundary |
| --- | --- | --- |
| CPU | Overall utilization, logical core count, recent trends | Unsupported temperature and frequency are shown as unsupported |
| Memory | Used amount, total, utilization trend | Clarify how the platform's cache memory is counted |
| Disk | Partition capacity, free space, read/write rates (when obtainable) | Personal file contents are not scanned |
| Network interface | Current upstream/downstream rates, cumulative totals, interface status | Clarify whether virtual adapters and loopback are included |
| Processes | CPU/memory rankings, process names, and necessary identifiers | Full command lines and environment variables are not collected by default |

GPU, temperature, battery, and device sensors are treated as later adaptation items, with the support status shown per platform.

### 7.2 Sampling and Retention Policy (Recommended Values)

The foreground performance page refreshes every 2 seconds by default; background live sampling can drop to every 10 seconds. Raw high-frequency points are recommended to be kept for 24 hours, minute-level aggregates for 30 days, and hour-level aggregates for 180 days, all of which may be adjusted or turned off.

These are starting budgets, not implemented behavior. The final values must be decided through resource testing on low-spec devices. The overview must not start a second sampling task.

### 7.3 Temporal Correlation

It is allowed to jump from near a request's time to the device load interval and show correlated evidence such as "CPU was high during the same period". Without application-level sampling data, no precise attribution such as "this request consumed X% CPU" is shown.

## 8. Network Monitoring and Diagnostics Module

### 8.1 Network Overview

Shows the currently active interfaces, the LAN IPv4/IPv6, the default gateway, the DNS configuration, the interface traffic, and proxy environment hints. The fields are read from the interfaces available on the system; when permissions or the platform do not allow it, each one is flagged separately.

Public IP lookups are active probes that send requests to external services, so the targets and the frequency have to be listed in the settings. Continuous external probing is not started by default; IP attribution and network quality information are auxiliary data and cannot guarantee service availability.

### 8.2 Diagnostic Tasks

| Task | User input | Result shown |
| --- | --- | --- |
| DNS query | Domain name, system DNS, or a user-specified server | Returned addresses, duration, error type, and execution time |
| TCP connectivity | Host and port | Connection succeeded/timeout/refused and the duration |
| HTTPS probe | An address explicitly selected by the user | Measurable phases such as DNS, connection, TLS, and first response |
| LAN device check | A saved device address | Port reachability, protocol handshake, and sharing scope status |
| Multi-target comparison | A small set of user-saved targets | Latency, failure rate, and sample count under the same probe strategy |

When the probe fields cannot be broken down, show the overall duration instead of fabricating a phase breakdown. Privileged packet capture, route tracing, and ICMP support are evaluated as separate capabilities; the basic functionality does not require administrator privileges to be resident.

### 8.3 Diagnostic Boundaries and Risk Control

By default, use a low frequency, short timeouts, and limited concurrency; automatically scanning the entire public address range is forbidden. Custom probe targets entered by the user run directly on the local machine and are not proxied through the cloud.

Distinguish configuration reads, one-off probes, and continuous monitoring in the results. A single failure only describes what was observed at that moment and along that path. If only environment variables were inspected without reading the system's full proxy configuration, the UI must clearly show the coverage.

Diagnostic history may expose internal network topology and service domain names; it is stored locally by default, can be cleaned up, and is not automatically included in the LAN sharing packages.

## 9. Agent Tracing and Call Stack

### 9.1 Product Goals

Show the step structure of a task, its time consumption, its call status, and its model usage. Users can move from a task to the failing step, and can also trace back from a cost record to a reliably correlated task.

The "call stack" in the product uses a task tree and a timeline to express asynchronous, parallel, and subtask relationships. The ordering of ordinary log lines must not be treated directly as a real parent-child call relationship.

### 9.2 Data Model and Sources

| Object | Key fields | Purpose |
| --- | --- | --- |
| Task | Stable identifier, title, source, device, start/end, status | One unit of work that users can understand |
| Trace | Trace identifier, source, protocol, and correlation quality | Collects one execution process |
| Span | Identifier, parent identifier, type, name, start/end, status | Steps such as models, tools, and sub-agents |
| Event | Time, event type, redacted summary | Retries, cancellations, errors, and checkpoints |
| UsageLink | Request identifier, span identifier, correlation basis | Connects the existing statistics records and prevents double billing |

First pick one source that genuinely provides parent-child relationships and close the loop, then extend the adapters. Both standard trace-event ingestion and tool-specific adapters can be evaluated, but the concrete protocol, fields, and compatible versions require separate technical design and are not considered supported in this plan.

### 9.3 Page Design

The task list filters by device, application, time, and status. In the task detail view, the left side holds a collapsible call tree, the middle a timeline, and the right side the status, metrics, and redacted events of the selected step.

Support locating failures, viewing retries, expanding sub-agents, and filtering model calls. Long tasks load progressively; unfinished tasks are marked as in progress, and tasks missing an end event are marked as incomplete.

### 9.4 Cost and Duration Attribution

A model request is the basic billable unit. The cost on a Span is a reference or an aggregation of the already-linked requests; no second copy is inserted into the global usage. An aggregated parent cost is not added to the child cost.

The total duration of parallel tasks uses the difference between the start and end times; the durations of all sub-steps must not be summed. When the intervals overlap, the critical path and the resource usage are computed only when the data is sufficient. Requests that cannot be reliably correlated remain "unlinked", and fuzzy inference is not used to fabricate a complete call tree.

Cross-device correlation also stores the device clock skew or its uncertainty; when the time evidence is insufficient, relative time and warnings are used to avoid an incorrect ordering.

## 10. Overview and Anomaly Notices

The overview prioritizes the capabilities that are already backed by real data: today's cost estimate, requests, tokens, collection health, the included devices, and the sync freshness. While performance and trace data are not connected, an integration notice replaces the sample curves.

Later additions may include: cost growth against a recent baseline, a drop in model recognition, persistent collection failures, a device that has not synced for a long time, sustained resource threshold breaches, and persistent DNS or connection failures.

Anomaly notices use explicit rules in the first phase, showing the trigger condition, its duration, and the evidence. Cost anomalies are compared over the same time span and state the unpriced proportion explicitly; when the data is incomplete, no seemingly precise growth conclusion is triggered.

Desktop notifications are enabled per category and are conservative by default; identical anomalies are merged into a single notification, and recovery notifications are optional. Early versions do not introduce cross-organization notifications, automated remediation, or complex rule orchestration.

## 11. UI and Interaction Specifications

### 11.1 Style Constraints

- Keep the existing compact top navigation and the system window title bar; do not place a duplicate mock title bar inside the app.
- Orange expresses the primary actions and the selected state, cyan is used for the primary trends, and the other colors express specific data categories and statuses.
- Tables, menus, dialogs, empty states, and filters use shared components, with the light and dark themes sharing the same structure.
- Icons use assets with traceable provenance; when there is no official asset, a consistent fallback is used and the future replacement is tracked.
- Use the system fonts and the existing information density, prioritizing the readability of Chinese text, amounts, dates, and numbers.

### 11.2 States and Feedback

Every data area covers at least loading, success, empty data, no permission, not yet supported, collection failure, and a stale cache. During a refresh the old data may be kept while "updating" is shown; a failure must not make the old data look freshly updated.

Cancelling sharing, disabling a device, disconnecting a device, and clearing the local history each use different wording. Operations that delete unrecoverable data show the scope and the consequences; ordinary filters, refreshes, and reversible settings take effect immediately.

### 11.3 Keyboard and Accessibility

The app menu supports arrow keys, Home/End, type-ahead, and Esc; all primary operations can be completed with the keyboard. Charts provide numeric summaries, and color is never the only status signal. The UI respects the system's reduce-motion preference.

The desktop app uses 820px minimum width as the official acceptance baseline; a narrower browser layout is a development-preview compatibility item. Tables may scroll internally, and dialogs must not exceed the visible area.

## 12. Technical and Data Architecture Plan

### 12.1 Keep the Existing Technical Mainline

The frontend continues to use React, TypeScript, Vite, and query caching; the desktop host uses Tauri 2; collection, sync, and aggregation are handled by Rust; SQLite handles the local persistence. The production app calls the local core directly and does not depend on a resident Node service.

Do not rewrite the existing statistics kernel immediately for the new modules. First unify the source contract and the background task management, then connect performance, network, and trace as separate services.

```text
Desktop UI: Overview / AI Usage / Trace / Performance / Network / Devices
                                              │
                            Local command and event interface
                                              │
            ┌─────────────────────────────────┼─────────────────────────────────┐
            │                                 │                                 │
 Collection & scheduling             Query & aggregation                  Pairing & sync
            │                                 │                                 │
             Tool adapters / system sampling / network probing / trace ingestion
                                              │
                 Local SQLite, price versions, settings, and diagnostic logs
```

### 12.2 Unified Source Contract

Each adapter declares the application identifier, the supported versions, the collection method, the field coverage, the precision, and the permission requirements. The result contains the added, updated, ignored, and error counts plus a cursor, and supports pause, cancel, and retry.

Log format changes should be isolated inside the adapter, so that a single failing source does not block the other sources. Data whose completeness is uncertain keeps a quality marker; disguising it as complete with default values is forbidden.

### 12.3 Data Governance

The data is split into: raw local statistics, historical rollups, remote snapshots, performance time series, network diagnostics, trace events, price versions, and settings. The source device and the data category must be traceable.

Database migrations have a version number, a pre-migration backup, and failure recovery; a failed upgrade does not overwrite the old database. Multiple modules share the background scheduler and the database access mechanism, to avoid duplicate scans and high-frequency full-table queries.

Backups should cover the database and the necessary configuration, and state whether they include trust credentials. Restoring to another device generates a new local identity; an identity must not be copied and then be online on both devices at once. Credential backups and ordinary statistics exports are handled differently.

### 12.4 Privacy and Local Control

Prompts, response bodies, API keys, full command lines, and environment variables are not collected by default. Error logs have to be redacted, and users can preview the content and the scope of a diagnostic bundle before exporting it.

The plan does not rely on third-party behavioral analytics. Early results are validated through local health reports, explicit test data, and voluntary user feedback; if telemetry is introduced later, its opt-in switch, field list, and retention policy are designed separately.

The data directory, disk usage, the retention policy, export, backup, and cleanup are all visible in the unified settings. Concealing background sharing state is unacceptable; users should always know whether sharing is on and which parties are authorized.

## 13. Release Roadmap and Implementation Order

The following are proposed milestones, and each stage is entered after the previous one passes acceptance. Design and research can proceed in parallel, but the critical data contracts have to be stabilized first. The estimates assume one primary developer working continuously with Windows and Mac test devices, and do not constitute a release date commitment.

| Phase | Proposed version | Work package | Completion criteria | Rough effort |
| --- | --- | --- | --- | --- |
| M0: engineering baseline | 0.1.x | Organize the source tree, licenses, documentation, and ignore rules; push the repository; set up CI; clarify the difference between the development build and the release build | A new environment can build following the documentation, and the test results are traceable | 3–5 person-days |
| M1: trustworthy AI usage | 0.2 | Field quality, unknown-model diagnostics, semantics audit, price coverage, full filtered export, migration and backup experience | The core statistics and the historical data have no duplicates and no silent loss | 10–15 person-days |
| M2: reliable multi-device | 0.3 | Pairing and transport design, scope management, snapshot generations, sync health, multi-adapter and cross-platform validation | Two real devices sync bidirectionally, and the retries and the revocation all pass | 10–20 person-days |
| M3: real device performance | 0.4 | Local sampling, trends, retention, resource budget; remove the related sample values from the overview | The metrics are real, the background overhead stays within budget, and restricted permissions degrade gracefully | 8–12 person-days |
| M4: network diagnostics | 0.5 | IP/DNS configuration, active probing, LAN connection diagnostics, history and target management | Typical DNS, port, and connection failures can be reproduced and explained | 8–12 person-days |
| M5: agent closed loop | 0.6 | First connect a real task tree, timeline, errors, and request correlation from one source | The same task is traceable without double billing, and the missing relationships are shown correctly | 15–25 person-days |
| M6: stable release | 1.0 candidate | Cross-platform packaging, upgrade and rollback, continuous operation, documentation and issue convergence | The unified stability and data-compatibility thresholds are met | 10–15 person-days |

Each phase's effort includes development and basic testing, and excludes uncontrollable time such as signing-account paperwork, waiting on external interfaces, and reverse engineering unknown logs. The actual schedule is re-estimated after M0 on the basis of the dependency list, and the rows are not simply added up as a delivery commitment.

### 13.1 Next-Round Priorities

1. Organize the local project and push it to a new repository, keeping the license and provenance list and excluding the build artifacts, the real databases, and the temporary logs.
2. Establish the frontend checks, the Rust core tests, and the Windows/macOS build verification, and clarify the naming of the development builds versus the official packages.
3. Provide visible states for the source completeness, the unknown models, and the unpriced records, and correct how the missing fields are expressed in the statistical semantics.
4. Write the sync protocol design and complete a pairing prototype, and then run a two-machine validation on Windows and Mac.
5. Replace the preview areas of the overview with real data item by item; the modules that are not finished yet keep a clear "preview" label.

### 13.2 Deferred Items

Public network relay, account systems, cloud storage, team workspaces, remote agent execution, full invoice reconciliation, automatic root-cause analysis, a plugin marketplace, and mobile clients all go into the candidate pool. They enter the schedule only once the core data is trustworthy, the sync is reliable, and there is a clear use case.

## 14. Acceptance Criteria and Quality Thresholds

### 14.1 Functional Acceptance

| ID | Scenario | Pass criteria |
| --- | --- | --- |
| A01 | Repeated scans of the same logs, and a scan after a restart | The request counts and the tokens do not increase repeatedly |
| A02 | Continuing the native collection after a history import | The baseline stays correct; an ambiguous merge is explicitly blocked and the source database is not written to |
| A03 | A source whose input tokens already include the cache | After the normalization the four totals are correct, and the cache is not counted twice |
| A04 | A missing model, price, status, or duration | Each missing reason is explained, the confirmable metrics are still counted, and no complete values are fabricated |
| A05 | Viewing the local and the multi-device views with the same filter | The aggregate equals the explicitly included local and remote scope, and the stale and missing devices are shown |
| A06 | Retrying, failing, and arriving out of order for the same remote snapshot | The retries do not accumulate, a failure does not half-update, and an old version does not overwrite a new one |
| A07 | Turning off a source, cancelling sharing, disabling and disconnecting devices | Each operation behaves as documented, and the scope of the retained or deleted data is clear |
| A08 | Pairing a new device and revoking it | An unauthorized device cannot read the new-protocol data; the access fails after the revocation |
| A09 | Wired/wireless and multi-adapter environments on Windows and Mac | The discovery works or a manual connection can be made explicitly; the failure diagnostics provide concrete evidence |
| A10 | Price sync and manual overrides | A locked price is not silently overwritten; the historical recalculation is traceable |
| A11 | Exporting all the filtered results | The counts, the filters, and the time semantics are consistent; the cancellation is supported; spreadsheet formula injection is prevented |
| A12 | Parallel agents and parent-child steps | The global cost is counted only once, and the task duration is not the sum of the parallel sub-steps |
| A13 | Insufficient performance or network permissions | The other modules remain usable, no fake values are shown, and a capability explanation is given |
| A14 | A database upgrade or an abnormal process exit | The app can recover to a consistent state, and the old-version data has a backup and a clear compatibility notice |

### 14.2 Performance and Stability Targets (Pending Benchmark Validation)

The first-round reference environment is planned as a 4-core CPU, 16GB of memory, an SSD, 100,000 usage detail records, and cached data from 3 devices: the P95 of the common aggregate queries must not exceed 1 second, the P95 of the paginated queries must not exceed 500ms, and entering a screen shows the cache first instead of waiting for a full scan.

The target is below 1% average CPU while idle in the background and below 250MB of total resident memory; Windows and Mac have to be measured separately and the targets adjusted to reality. When performance sampling, log scanning, and sync work at the same time, the additional overhead and the peaks should be reported, rather than only measuring an idle page.

Stability acceptance includes at least 72 hours of operation, sleep and wake cycles, network switching, offline device reconnection, interrupted database writes, and the first import of a large amount of historical data. When the performance targets are not met, adjust the sampling and the query strategy first instead of sacrificing the data consistency.

### 14.3 Release Gates

An official release candidate must not carry over known data duplication, silent rewriting of amounts, migration loss, unauthorized sharing, or modifications to the source database. A feature preview must not be mixed into the real statistics totals.

Each release is accompanied by the version, the platform, the build provenance, the checksum, the migration notes, the known limitations, and the rollback instructions. Windows and macOS are validated separately; an unverified package is not labeled as stably supported. The auto-update channels, the signing, and the rollback policy are enabled only after a separate acceptance.

## 15. Success Metrics and Validation Methods

The product success is measured primarily by "the data is trustworthy and the task can be completed", and does not presuppose collecting more user behavior.

| Metric | Definition | Validation method |
| --- | --- | --- |
| First-run success rate | In the test environments where a supported tool is installed and readable logs exist, the share of the first-time integrations that succeed | A fixed test matrix and voluntary feedback, recording the permission and the version issues separately |
| Model recognition coverage | The share of the records that are successfully attributed when the raw record contains a valid model clue | A source sample set; counted separately from the cases where the source field itself is missing |
| Price coverage | The share of the recognizable token requests that have an applicable price | Displaying the denominator and the proportion of the unpriced requests and tokens |
| Sync correctness | The number of the duplicate, missing, and out-of-order errors on a preset two-machine dataset | Automated snapshot tests and real-device acceptance |
| Query experience | The aggregate and paginated response times at the reference data volume | A local benchmark report that does not depend on uploading real records |
| Troubleshooting effectiveness | Whether the related evidence can be found when given DNS, connection, and load failures | Scenario task tests and user feedback |
| Residency cost | The CPU/memory/disk usage while idle, scanning, syncing, and sampling | Continuous-operation reports per platform |

Without real samples, no conclusion about active users or conversion rates is presumed. If retention is evaluated later, use voluntary test-user interviews or opt-in statistics, and never silently upload the application and device details.

## 16. Key Risks and Mitigations

| Risk | Impact | Mitigation and schedule |
| --- | --- | --- |
| A tool upgrade changes the log format | Unknown models, missed collection, or duplicates | Adapter versions and sample sets, source health status, maintained continuously from M1 |
| Historical rollups overlap with the details | The totals cannot be merged credibly | Keep the baseline contract and the rejection of ambiguous merges, and provide backup and separate-directory options |
| The same source logs are copied on several machines | Double counting across devices | Keep the source identifier, surface the overlaps, and never apply a fuzzy deduplication when the evidence is insufficient |
| Pricing aliases and source changes | The cost estimates become distorted | Separate the raw/canonical/pricing models, and add price versions and coverage |
| Asymmetric discovery or proxy interference | Users wrongly conclude that no data is collected | Multi-adapter probing, manual connection, and step-by-step diagnostics, in M2 |
| The old sync protocol has no authentication | The metrics are exposed to the reachable devices | Off by default, with an explicit compatibility mode; finish the new protocol before expanding the distribution |
| High-frequency monitoring costs too much | It degrades the development environment | Shared scheduling, downsampling, retention caps, and cancellation, in M3 |
| The trace data relationships are incomplete | It produces wrong call trees and costs | Prefer the real identifiers, keep the unlinked records, and make no unfounded inferences, in M5 |
| The scope keeps expanding | The quality of the AI core declines | Each stage is gated by acceptance, and the candidate features do not enter the mainline directly |
| The brand assets and the reused code have an unclear provenance | The provenance is hard to review at release time | Keep the licenses, the comments, and the asset provenance records, and complete the material check before the public release |

## 17. Distribution, Maintenance, and Follow-up Decisions

The current recommendation is to serve the early users with a local desktop app distributed through GitHub. A development build allows a quick UI validation; a stable channel has to provide reproducible builds, platform validation, and data-compatibility documentation.

Commercialization is not a prerequisite for the current phase. Accounts, cloud backup, and team collaboration would significantly change the sync and the privacy models, so they should be planned separately once the real requirements appear, and the paid and account infrastructure should not be built in advance.

Still to be confirmed in the corresponding phases:

| Decision | Current recommendation | Confirm by |
| --- | --- | --- |
| macOS support scope | First select an architecture that can actually be tested, then expand | Before the M0 build matrix is finalized |
| Official release signing and updates | Manual download upgrades first, then add rollback-capable updates | Before M6 |
| Relationship between the new pairing protocol and the old one | The new protocol by default; the old one only as an explicit compatibility mode | Before the M2 implementation |
| Retention duration for the details and the monitoring | Use the recommended defaults and allow the users to change them | Before the M1/M3 data cleanup features |
| The first trace source | Choose a tool that provides stable parent-child identifiers and is easy to verify | Before M5 starts |
| Whether to publish the repository | Currently private; decide once the release intent is clear | Before the first public release |
| Full export and backup formats | Separate the statistics export from the restorable backup, and define the versions | Before the M1 implementation |

## 18. Reading Checklist and Maintenance Conventions

This plan should directly answer the following questions: what is genuinely usable today; who the first phase serves; how the multi-device data avoids duplication; what the default sharing behavior is; how the missing metrics are presented; how the new modules are integrated; how each stage is judged complete; and what has not been promised.

Each later stage maintains three interrelated documents: the product plan describes the goals and the boundaries, the technical design describes the data contracts and the implementation trade-offs, and the release notes describe what was actually delivered. When the implementation changes, update the status accordingly, to avoid treating the roadmap as a list of product features.

### Sources

- The project README and the 0.1.3 local source code: the existing capabilities, the collection scope, the migration, and the sync limitations.
- The layout, the CC Switch style, the multi-device aggregation, the application sharing choices, DeepSeek Harness, and the future module requirements confirmed in the current discussion.
- This document contains no market-size or competitor research; the user profiles, the resource budgets, and the development effort are product assumptions that have to be validated through testing and feedback.
