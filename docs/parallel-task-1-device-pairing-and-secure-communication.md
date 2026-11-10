# Parallel Development Task 1: Device Pairing and Secure Communication

Status: Pending development | Baseline: Observatory v0.1.26 | Target platforms: Windows x64, macOS Apple Silicon

This task can be handed to a single developer to drive independently, in parallel with [Task 2: Multi-Device Scanning and UI](./parallel-task-2-multi-device-scan-and-ui.md). The overall product rules, migration constraints, and the complete release gate are in the [master plan](./multi-device-pairing-and-scan-plan.md). This document is a work breakdown and does not mean the feature is already implemented.

## 1. Goal

Establish a device relationship that both sides explicitly consent to, and protect both LAN usage reads and remote scans with device identity and authorization. Devices registered in older versions can no longer read automatically by default and must be re-paired. Transport is encrypted, device identity is verifiable, and a local revocation takes effect immediately.

**Delivery scope**: the security protocol ADR, device identity and pairing, authorization storage and revocation, authenticated encrypted transport, snapshot reads and background sync authorization, legacy version migration, and the related tests. Scan scheduling and the page are the responsibility of Task 2; this task provides the authenticated communication capability for it to call and does not implement its batch status UI.

## 2. Parallel Boundary and File Responsibilities

| Area | This task | Task 2 |
|---|---|---|
| Communication and authorization | Owns `native/src/lan_usage/server.rs`, `client.rs`, `identity.rs`, `storage.rs`, `mod.rs`, and may add security modules | Calls them through the agreed interfaces, using mocks first; does not modify these files directly |
| Database and projection | Owns the authorization migration, revocation, and the authorization check on snapshot stores, touching `native/src/database` and `lan_usage/projection.rs` where needed | Does not change the authorization database schema; verifies with fixtures |
| Local API | Owns all final route registration in `native/src/api.rs` and merges Task 2's registration requests according to the agreed interfaces | Puts the scan implementation in a separate new module; provides the list of API hooks it needs and does not edit `api.rs` directly |
| Frontend | Provides the interface contract and key-free response examples | Owns `src/usage/DeviceManager.tsx`, `UsageWorkspace.tsx`, the styles, and the frontend types |
| Tests | Owns the protocol, authorization, migration, and database tests; separate test files | Owns the scan and UI tests and the combined scenarios; separate test files |

If a change to a file owned by the other side is genuinely needed, first submit an interface change note and let that file's owner merge it. Both sides build on **their own branch or worktree** from the same baseline and must not edit in the same working directory at the same time; during development, do not commit real device identities, keys, private data, or run logs.

## 3. Prerequisite Contract: Publish to Task 2 First

Complete the ADR draft and the interface draft first so that Task 2 can start developing with mocks immediately; freeze the final contract only after the security review passes. Keep the operation names, parameters, states, error codes, time units, and retry semantics in sync, and provide JSON examples. A draft is not a production protocol, and unauthenticated interfaces must not be assumed to be shippable.

Minimum capabilities:

1. Discover candidate devices and show the device name, version, address, and protocol capabilities; a discovery result does not mean the device is trusted.
2. Start, cancel, and query pairing requests; a request expires after 5 minutes by default.
3. Both sides display and confirm the same verification code; the requested side can consent or reject; read or scan permissions are not granted without confirmation in both directions.
4. Query pending requests and device authorization state; the interfaces return only the non-sensitive fields the UI needs.
5. Unpair, immediately revoking the local permission, and report whether the peer has received the revocation notice.
6. Authenticated remote scan submission/query and snapshot read capabilities for Task 2 to call, plus unified identity verification and error types.
7. Support querying the list of devices usable after pairing, their enabled state, version incompatibility, and the time of the most recent successful snapshot.

**Initial state vocabulary, which both sides must unify when the ADR is frozen**: pairing `pending / approved / rejected / expired / revoked`; errors at least `unauthorized / incompatible / offline / timeout / protocol_error`. The pending **bidirectional confirmation details**, field names, and serialization format are all governed by the ADR. Do not treat temporary examples as finished interfaces.

## 4. Implementation Tasks

### A1. Threat Model and Design Review

- Define how mutual consent and the short verification code bind to the actual handshake, the device public key, and the authorization; use mature TLS and standard cryptographic libraries, and never design cryptographic algorithms in-house.
- Settle certificate/public key pinning, cross-platform identity key storage and rotation, duplicate identity detection after restoring an app backup, and the boundaries of unauthenticated discovery.
- Define the business idempotency key, the fresh authenticated identifier per request, replay protection, and the persistent deduplication window; credentials, usage, and status must not travel over plaintext HTTP, URL parameters, broadcasts, or logs.
- Specify offline revocation: the data authorization expires immediately; a separate authenticated channel that only permits revocation notices, together with a minimal revocation record, must be retryable, and repeated revocation by both sides must not deadlock. State plainly that an offline peer does not learn about it immediately.
- Submit to Task 2 the protocol version, state machine, error codes, interface types, timeout/retry rules, and security constraints. Real user traffic is not allowed before an independent security review passes.

### A2. Identity, Pairing, and Authorization

- Create the persistent identity and the request and authorization states; every permission that accesses local usage or triggers a local scan is verified against the peer identity.
- Implement initiation, bidirectional confirmation, rejection, cancellation, expiry, query, and restart recovery; handle duplicate requests and confirmations idempotently.
- On revocation the local machine immediately denies access, stops the regular sync to the peer, and excludes it from the aggregate; as the master plan requires, delete that peer's local cache while protecting local and other devices' data.
- Re-validate the authorization and the revocation generation inside the snapshot store transaction, preventing late data from being stored again after a revocation.

### A3. Encrypted Transport and Compatibility Migration

- Rework the snapshot pull and the server-side egress so that manual sync and background sync both go through the authorized, authenticated, encrypted channel; verify that the device identity in the snapshot matches the paired identity.
- Disable or explicitly reject the legacy unauthenticated `/api/lan/usage`; old devices and CC Switch must not silently fall back and should get a displayable version incompatibility notice.
- Migrate legacy registered devices to "needs re-pairing"; old snapshots must not be used in the aggregate without authorization. Do not change the local raw usage, the prices, or the shared sources configured on each device.
- Rate-limit pairing and scan commands and limit the input size; keep the diagnostic information minimal and redacted.

### A4. Integration Interface Delivery

- Provide two test devices/instances and reproducible steps in an isolated environment; deliver the local API and the authenticated transport adapter that Task 2 can call.
- Expose only the authorized execution and query capabilities for remote scans; the existing `sync`, which calls `sync_all`, must not be used as the remote execution entry point, to avoid cascading between devices.
- Receive Task 2's API registration list and wire it into `api.rs` on this task's branch; both sides check together that the final types and states are consistent.

## 5. Acceptance for This Task

- [ ] When one side has not confirmed, or when the request is rejected, expired, or the verification codes differ, usage cannot be read, scans cannot be queried, and scans cannot be triggered; an unknown third device is rejected as well.
- [ ] After both sides consent, authentication still works after a restart; an identical device name, a changed IP, and a forged certificate cannot substitute for the real identity.
- [ ] The legacy plaintext endpoint is rejected, packet capture leaks neither usage nor authentication secrets, and logs, errors, APIs, and discovery packets expose no private keys or tokens.
- [ ] A local revocation takes effect immediately; an offline revocation shows a pending notification and can be retried; simultaneous revocation by both sides and repeated operations are idempotent.
- [ ] When revocation races with an in-progress sync, a late snapshot cannot be written back or take part in the aggregate; the deletion affects only that device's copy.
- [ ] Startup and timed sync visit only authorized devices; the shared source settings take effect; a failure does not overwrite the most recent successful snapshot.
- [ ] After upgrading a copy of the v0.1.26 database, old devices need re-pairing and old snapshots are not added to the aggregate as authorized data; the original local records and prices are intact.
- [ ] Run the security, migration, and protocol negative tests against a temporary data directory; record the test commands and results; the independent security review passes.

## 6. Handoff and Definition of Done

Hand over to Task 2 the ADR, the final API/types, the error codes, the test doubles, the run instructions, and the known limitations. After a two-instance integration test, both sides jointly complete the Windows/Mac two-machine acceptance; passing this task alone does not mean the whole feature is releasable. Release happens only after every item on the master plan's security, scanning, migration, and release checklists has passed.
