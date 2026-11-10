# Parallel Development Task 2: Multi-Device Scanning and UI

Status: Pending development | Baseline: Observatory v0.1.26 | Target platforms: Windows x64, macOS Apple Silicon

This task can be handed to a single developer to drive independently, in parallel with [Task 1: Device Pairing and Secure Communication](./parallel-task-1-device-pairing-and-secure-communication.md). The overall rules and the final release gate are in the [master plan](./multi-device-pairing-and-scan-plan.md). This document is a work breakdown and does not mean the feature is already implemented.

## 1. Goal

Clicking "Scan Update" on the multi-device aggregate page makes the local machine and this round's target devices each run a scan, and shows the waiting, scanning, syncing, and final status per device. It accurately distinguishes "All complete" from "Partial failure", and also provides the pairing request, consent/rejection, and device status UI.

**Delivery scope**: the local scan service abstraction, remote scan jobs and initiator-side batch coordination, per-device status, the pairing and device management UI, the automated tests, and the integration testing. This task works through Task 1's authentication and authorization interfaces; it must not open an unauthenticated service on its own or bypass the peer's consent.

## 2. Parallel Boundary and File Responsibilities

| Area | This task | Task 1 |
|---|---|---|
| Scanning and batches | Owns the new standalone scan/batch module and provides the list of hooks for `api.rs` | Owns the final route registration in `native/src/api.rs` and wires in this task's modules |
| UI | Owns `src/usage/DeviceManager.tsx`, `UsageWorkspace.tsx`, `src/usage/types.ts`, the matching styles, and the frontend tests | Only submits the contract, the sample data, and any necessary interface change proposals |
| Communication and authorization | Writes the adapter interfaces and mocks and calls the final authentication layer; does not change the communication, identity, storage, migration, and projection files owned by Task 1 | Owns `lan_usage/server.rs`, `client.rs`, `identity.rs`, `storage.rs`, `mod.rs`, and the database migration |
| Tests | Scan, batch, and UI tests use separate files and temporary data | Pairing, protocol, authorization, and migration tests use separate files |

When shared types or files are involved, negotiate first and let the owner of the corresponding file merge them according to the final contract. Both sides use their own branch or worktree and must not edit in the same working directory at the same time. The `scan_lock` currently belongs to `UsageBackend`: extracting and registering the scan service requires cooperation from both sides and must not be duplicated as a separate lock.

## 3. How to Start in Parallel and Dependencies

1. Immediately build the pairing, device list, and scan status UI on local fake data; define the scan status tests without calling the old unauthenticated remote interfaces.
2. Once Task 1 delivers the ADR draft, align the local mock interfaces with the pairing states, device list, authorization query, scan submit/query, snapshot pull, and error codes; do not treat the draft as the final protocol.
3. Scan execution and batch coordination can be developed against an injected mock transport; real remote integration testing depends on Task 1's security review and authentication layer.
4. Provide the list of operations and parameters that `api.rs` needs, to be registered by Task 1; after the merge, run the type check, the integration tests, and the real two-machine acceptance.

Initial vocabulary agreed by both sides: pairing `pending / approved / rejected / expired / revoked`; device scan `waiting / scanning / syncing / completed / partial_failure / failed / timed_out / cancelled / unknown`. The fields, whether the states need to be subdivided, and the serialization format are governed by the contract frozen by both sides, and the page must not infer authorization from temporary strings.

## 4. Implementation Tasks

### B1. Device Pairing and Management UI

- After discovering a device, a request can be started; show the peer's device name, protocol version, the permission description, the validity period, and the verification code on both ends; the user can consent, reject, or cancel.
- Distinguish waiting for the peer, waiting for local confirmation, paired, rejected, expired, revoked, and version incompatible; a rejection and a missing confirmation must not be treated as paired.
- When unpairing, explain that the local machine deletes that device's cache; while offline show "Unpaired locally, awaiting peer confirmation". Disabling and unpairing are different operations.
- The states have readable text and accessibility markers, support keyboard operation, light and dark modes, and narrow windows. Legacy devices get an upgrade and re-pairing prompt.

### B2. Scan Service and Remote Job

- Extract a shared service that scans only the local machine, reusing the existing `scan_lock`, the enabled sources, and their source errors; the UI, background, and remote scans share the same execution/lock mechanism.
- Create and query remote jobs protected by authentication; every job supports an idempotency key, an explicit terminal state, the timing, and the source errors. A remote scan must not call a sync entry point that would go on to trigger other devices.
- A retry with the same idempotency key returns the same job; compatible concurrent scans from different sources are merged into the current execution and share the result, and when they cannot be merged safely they are queued and their state is shown. A new request after the scan completes may start a new scan.
- A timeout does not mean the remote side has stopped: keep querying the existing job or show "Unknown status"; job records must be recoverable across restarts, otherwise they are explicitly marked as interrupted, and success must never be reported out of nowhere.

### B3. Multi-Device Batch Coordination

- Fix the targets at click time: the local machine plus the devices that are enabled and paired; disabled or unpaired devices are listed as "Not participating", and the scope is shown explicitly.
- Suggested limits: concurrency 3, connection timeout 10 seconds, single status query timeout 10 seconds, batch wait limit 5 minutes; the frozen contract is authoritative, and tests must be able to inject short timeouts.
- After the peer's scan reaches a terminal state, pull the snapshot through Task 1's authentication layer; only mark it complete after the identity is verified and the snapshot is stored successfully. A scan source error must be marked as a partial failure and must not become success just because the snapshot pull succeeded.
- Scan/sync failure, revocation, offline, and timeout each give their own reason; on failure the most recent successful snapshot is kept and its cache time is shown. The concurrency safety of stores and revocations is the responsibility of Task 1, and this task must not bypass it.
- Prevent duplicate starts while a batch is unfinished; support retrying failed devices and keep a traceable batch/job association. The state must be recoverable after the page is closed and reopened, and shown as unknown when it cannot be recovered.

### B4. Aggregate Status and Refresh

- Show the device name, the state, the completion time or error, and this round's sync result for each device; use appropriate semantics such as `role=status` for in-progress work and avoid distinguishing states by color alone.
- **All updates complete** is shown only when every fixed target scanned successfully, the remote snapshot was stored successfully, and there is no enabled but unpaired device. Otherwise show "Not all complete" with the matching reason; disabled devices do not count toward the targets.
- The local page, startup, and timed refresh keep their current behavior: local scans and authorized snapshot pulls only; only the manual button in the multi-device aggregate triggers remote scans.
- Refresh the matching statistics after a successful store; failed data is kept and marked as cached, and a refresh must not lose the state of a batch that is still running.

## 5. Acceptance for This Task

- [ ] With mocks, cover pairing waiting, consent, rejection, expiry, the legacy case, and revocation; the UI never shows keys, and pairing does not continue when the verification codes differ.
- [ ] Two devices each add a test record; after the click each scans once, and the status and completion time are shown per device; the aggregate shows the new records with no duplicates.
- [ ] Three devices succeed, go offline, and hit a source error respectively: each result is shown, the overall state shows not all complete, and the data of the successful devices can be refreshed.
- [ ] A scan that succeeds but whose snapshot pull or store fails shows a sync failure and does not report all complete; the most recent successful cache and its time are still recognizable.
- [ ] Disabled, unpaired, and legacy devices are displayed and counted correctly; when an enabled but unpaired device exists, all complete must not be shown.
- [ ] Retrying the same job, simultaneous starts from both ends, and competition with a background scan neither cascade nor deadlock; compatible executions are merged, and no second scan is started out of nowhere after a timeout.
- [ ] The state is recovered on refresh, when leaving the page, and when closing and restarting the app, and shown as unknown when it cannot be recovered; the local and timed refreshes do not scan remotely.
- [ ] The main operations are completable with the keyboard, light and dark modes and narrow windows are readable, and the frontend tests, type check, build, and Rust scan tests pass; record the commands and the results.

## 6. Integration Testing and Final Acceptance

First validate pairing, scanning, and the failure scenarios in two isolated instances, then verify on real Windows and Apple Silicon Mac hardware. Task 1 is responsible for the interface authentication, authorization, migration, and data safety; this task is responsible for the states, the scheduling, and the usability. Both sides accept together against Section 7 of the master plan, covering offline revocation, legacy migration, caching, the shared scope, timeouts, and concurrency.

At handoff, submit the API call list, the state machine, the mock data, the test commands, and the known limitations. Completing this task alone does not mean the feature is releasable; release happens only after both tasks are integrated, the overall acceptance passes, and the installer/auto-update checks pass.
