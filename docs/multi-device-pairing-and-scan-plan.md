# Multi-Device Pairing and Scan Update Development Plan

Status: Pending development | Applies to: after v0.1.26 | Updated: 2026-10-30

## 1. Goals and Delivery Scope

Establish a trusted connection between Windows and Apple Silicon macOS through mutual confirmation. Only a device that the peer has authorized may read the peer's shared usage or trigger a scan on the peer's own machine. After the user manually clicks "Scan Update" on the multi-device aggregate page, each enabled device scans and syncs on its own, the UI shows the status in real time, and the result states clearly whether everything succeeded.

Delivery covers pairing, authorization and revocation, protected data sync, remote scanning, per-device and overall results, migration notices, testing, and release. Each device keeps its own configured shared-app scope; conversation bodies, API keys, and provider configuration are never shared.

**Non-goals**: public internet exposure, cloud accounts, remote control of other settings, deleting raw data on the remote side, and Intel Mac/Linux releases. Legacy CC Switch devices without the new protocol must not bypass authorization; supporting them requires a separate design, and this phase shows an incompatibility and upgrade notice.

## 2. Current Code Facts

- `native/src/lan_usage/server.rs`: listens on HTTP while LAN sharing is enabled; `/api/lan/info` and `/api/lan/usage` currently require no authentication.
- `native/src/lan_usage/client.rs`: pulls remote snapshots device by device; `sync_all` skips disabled devices but does not trigger a remote scan.
- `native/src/api.rs`: `sync` runs a scan under the local scan lock and then pulls each device; `lan_connect` registers by address and syncs immediately; the background timed sync also calls `sync_all`.
- `native/src/lan_usage/storage.rs` and `projection.rs`: store devices and remote snapshots and feed the aggregate projection. Disabling a device and revoking an authorization must be defined separately; changing only the button text is not enough.
- `src/usage/DeviceManager.tsx`: the current connection is a one-way registration; `UsageWorkspace.tsx`: the scan action currently returns only the final result, and the messaging centers on the local scan.
- `native/tests/usage_integration.rs` already covers the existing snapshot sync; the new protocol needs extended tests, and the old unauthenticated tests cannot serve as security acceptance.

These are the findings from inspecting the v0.1.26 source; specific functions and files may change as development proceeds.

## 3. Product Rules

### 3.1 Pairing and Authorization

1. A initiates a connection request; B shows the requesting device name, fingerprint verification details, the requested permissions, and the validity period, and the user can consent or reject. A confirms the same short verification code at the same time; only then is the pairing complete, which prevents impersonation by a forged device name alone.
2. Each device separately grants the peer permission to read **local** shared usage and to trigger a **local** scan. By default a complete pairing includes consent in both directions; a bidirectional authorization is established only after both sides explicitly confirm. A fixed permission set can be implemented first; fine-grained switches are not required.
3. Requests that are unconfirmed, rejected, expired, or revoked must not sync or trigger scans. Duplicate requests, application restarts, and repeated confirmations must have well-defined, idempotent results.
4. A request is valid for 5 minutes by default; the user can cancel it. The device name is for display only and is not an identity credential.
5. Either side may unpair: the local machine immediately rejects the revoked credential, stops sending usage and scan requests to that device, and excludes the device from the aggregate; when the peer is online it is notified of the revocation, and when offline the notification is queued for delivery and retried once the connection is restored. The revocation notification uses an authentication channel that is separate from the data authorization and only permits submitting a revocation; a minimal revocation record is kept until it is confirmed or expires, and repeated revocation by both sides must be idempotent. The exact mechanism is determined during the T1 review. Because the offline peer cannot be informed immediately, the status should read "Unpaired locally, awaiting peer confirmation". The unpaired side must be rejected when it tries to access again.
6. Cached remote copies are deleted by default on revocation; the local raw usage is unaffected. The user must see this behavior in the unpair confirmation dialog. Revocation must not delete local data or data belonging to other devices.

### 3.2 Scanning and Results

- Remote scans are triggered only by manually clicking "Scan Update" in the **multi-device aggregate**; the local page, startup, and timed refresh keep using local scans and authorized snapshot pulls, and timed refreshes must not frequently trigger scans on all remote devices.
- Each round fixes one target list: the local machine plus the remote devices that are enabled and paired at click time. Unpaired or disabled devices are listed separately as "Not participating" and are not counted as successes; the UI must explain the scope of the statistics. If an enabled but unpaired legacy device exists, the UI shows that re-pairing is required and must not claim that all devices were updated.
- Each device is shown as `waiting → scanning → syncing → completed/partial_failure/failed/timed_out/cancelled`; the local machine has no network sync phase. Show the device name, result, completion time, and a readable error. If a scan source reports an error the result is "Partial failure", and even a successfully pulled snapshot does not count as a full success.
- "All updates complete" may only be shown when every device in the target list has finished scanning and its remote snapshot has been stored successfully, **and there is no enabled but not yet paired device**. If any device has a partial failure, a timeout, is offline, is unauthorized, or is unavailable, show "Not all complete" and keep the individual reasons. A timeout is not a success; the remote side may still be scanning, so later queries must remain possible and the UI must not imply that it has stopped.
- Aggregate data is refreshed only after a successful store; failed devices keep showing the last successful cache with its cache time marked, and a stale snapshot must not be presented as this round's result. Repeated clicks must not start overlapping batches, retrying failed devices is allowed, and the device count, concurrency, and timeouts must all be bounded.

## 4. Technical Approach and Protocol Constraints

### 4.1 Security Design Gate (settled and reviewed before coding)

Use reviewed TLS and standard cryptographic libraries. Devices generate a persistent identity key, every connection verifies the certificate or public key fingerprint, and the pairing process binds both identities through mutual confirmation of a short verification code. All usage data, scan commands, status, and credentials travel only over authenticated and encrypted connections; credentials must never be exposed through plaintext HTTP, URL parameters, logs, or discovery broadcasts. Designing cryptographic algorithms in-house is forbidden.

The concrete TLS certificate generation and storage, the mutual authentication or equivalent scheme, the short-code algorithm, and the cross-platform key storage are specified by T1 in an ADR that goes through security review; a protocol that has not been reviewed must not carry production traffic. Persisted device keys must restrict local file access permissions and be verified on Windows/macOS. Discovery broadcasts and unauthenticated identity announcements only provide candidate addresses and grant no permission; an address change must not automatically change an already bound identity.

Authenticate per request, restrict operations and device identity, reject replays, rate-limit pairing requests and scans, limit input sizes, and keep credentials out of logs and error messages. The legacy plaintext `/api/lan/usage` must be disabled or explicitly rejected, with no switch left that bypasses consent; unauthenticated entry points must not reach scan results either. The transport must still handle certificate expiry, clock skew, identity duplication caused by restoring a device backup, and re-pairing after key rotation or loss.

### 4.2 Interface Contract (T1 owns the final version)

The following are logical operations; the final HTTP paths, fields, and versions are finalized by T1 and published to all tasks, so that parallel development does not each assume its own:

| Operation | Request | Response/Event | Key constraints |
|---|---|---|---|
| Start pairing | Candidate address, local identity and ephemeral handshake material | Request ID, validity period, verification codes for both sides | No usage data; rate limited; discovery only yields candidates |
| Query/confirm/reject | Request ID, user choice | `pending/approved/rejected/expired` | Authorization is issued only after both sides confirm; the confirmation leaks no credential |
| Revoke | Authenticated device and authorization ID | Local revocation result, remote notification status | Immediate local invalidation; offline keeps a notification retry state |
| Create scan | Authenticated device, `batchId`, idempotency key | Remote `jobId` and initial status | Reuses the local scan lock; must not cascade into remote scans |
| Query scan | `jobId` | Status, times, scan result and source errors | Checked against the authorization; history has a retention period |
| Pull snapshot | Authenticated device | Device identity, snapshot and shared scope | Identity is verified; a manual scan batch may only be pulled after the scan reaches a terminal state, while an ordinary timed sync can pull independently; a batch counts as complete only after a successful store |
| Query local batch | `batchId` | Fixed target list with per-device status and overall status | Local API; after a restart the state is recoverable or explicitly marked as interrupted |

`batchId` and `jobId` use unpredictable identifiers; a request timeout and retry use the same business idempotency key, but each transmission uses a new authenticated request identifier, so that replay protection and safe retries do not conflict. Idempotency records must be persisted and cover the longest batch wait and retry period, with the retention period determined by T1. The protocol carries a version and capabilities; older versions return an explicit "Not supported, upgrade required" and must not fall back to unauthenticated sync. Commands and snapshots must both be bound to the peer identity, and a snapshot must be verified as coming from the expected device.

### 4.3 Scheduling Recommendations

- A single machine uniformly reuses the existing `scan_lock`: the UI, background scans, and remote scans share it; a remote call scans only the local machine, and calling `sync`, which contains `sync_all`, is forbidden to avoid mutual cascading.
- The initiator maintains the batch state and the per-device state, and executes with bounded concurrency. Recommended defaults: concurrency 3, connection timeout 10 seconds, single status request 10 seconds, batch wait limit 5 minutes; the exact values are confirmed and testable in T1/T4. After a timeout the same job can still be queried safely, and a second job must not be generated.
- The local machine uses the `errors` field of the existing scan result to detect partial failures; the snapshot is pulled only after the remote side reports the scan complete, and the identity, shared scope, and successful store are verified. A network failure or the user leaving the page must not cancel the real work of other devices; when the UI comes back it should recover the state or show "Unknown status".
- A retry with the same idempotency key returns the same job; when different initiators request at the same time and a compatible local scan is already running, the request joins that scan and shares the result instead of starting another scan; a new request arriving after the scan completes may start a new scan. Background scans follow the same rule, and when a merge is not safe the work is explicitly queued and its status is shown. When revocation races with the snapshot store, the store transaction revalidates the authorization and the revocation generation, preventing a revoked device from restoring its cache or its place in the aggregate.
- Error types must at least distinguish unauthorized/revoked, peer rejection, version incompatibility, offline, scan error, sync error, timeout, and unknown status. Sensitive content must not appear in error displays.

## 5. Executable Tasks and Acceptance

All tasks stay on separate branches or worktrees and develop against the contract first; cross-task interface changes are coordinated by T1. The effort figures below are rough estimates in effective working days, covering development and this task's own verification, and excluding waiting for devices, certificates, review, and release builds. The actual duration depends on the protocol library and the two-machine environment.

### T0: Baseline and Test Environment (0.5–1 days)

- Content: confirm the main branch, version, and workspace; set up a Windows + Apple Silicon Mac two-machine environment on the same LAN plus separate-network and offline scenarios; record each side's IP, app version, and sharing settings; prepare test data without real keys and a backup of the old version.
- Deliverables: test environment notes, a reproducible old-version sync path, and automated baseline results.
- Acceptance: the existing tests pass; the current behavior of manual/timed sync, device disabling, caching, and source selection can each be explained; the user's real database is never overwritten.
- Dependencies: none. It can run in parallel with T1 and with T6's early test research; T6's formal protocol tests depend on the T1 contract.

### T1: Threat Model, Pairing Protocol, and Interface Contract (1–2 days)

- Content: settle the identity key, TLS trust and fingerprint comparison, mutual consent, pairing validity, revocation, idempotency, and the legacy rejection policy; produce the protocol version, error codes, storage design, state machine, and ADR.
- Deliverables: protocol document, sample messages/type definitions, sequence diagrams, and a security checklist; no real keys.
- Acceptance: an unknown device cannot read usage or trigger a scan; mismatched verification codes, replay, expiry, revocation, and device impersonation all have concrete handling; the offline revocation boundary and the legacy rejection are specified; an independent code/security review is obtained.
- Dependencies: none. The final interfaces of T2/T3/T4 depend on this task; their early research can run in parallel.

### T2: Local Identity, Pairing Requests, and Authorization Storage (2–4 days)

- Content: add identity/authorization modules and database migrations under `native/src/lan_usage`; implement requests, consent, rejection, expiry, cancellation, restart recovery, and authorization revocation; expose a local API for the UI; isolate credentials per device and close the legacy auto-trust path.
- Deliverables: the database migration, the authorization service and types, and the pairing state in the device list.
- Acceptance: no authorization is established unless both sides have explicitly consented; repeated operations are idempotent; local revocation takes effect immediately; authorizations and pending requests are correct after a restart; device keys never appear in `lan_status`, logs, or database exports; a failed migration rolls back without corrupting the original data.
- Dependencies: the T1 contract. It can run in parallel with T3's transport layer and T5's UI, with module boundaries agreed in advance.

### T3: Authenticated Encrypted Transport and Data Migration (2–4 days)

- Content: rework `server.rs`, `client.rs`, and `mod.rs` to provide the T1-based authenticated transport; enforce the authorization check on every snapshot request; remove unauthenticated usage reads; route background sync and manual sync through the same authorization; store data only after the device identity matches.
- Deliverables: a testable transport adapter, protocol version/capability detection, and a legacy incompatibility notice.
- Acceptance: access via the original HTTP usage endpoint and via an unknown device both fail; access fails when unpaired, with a forged certificate, or after revocation; data cannot be read in plaintext on the wire; the shared app scope takes effect; old devices and CC Switch cannot silently fall back; a network failure does not overwrite an existing snapshot.
- Dependencies: the T1 contract. It can run in parallel with T2 by mocking the identity/authorization interfaces in tests; integration requires T2.

### T4: Remote Scan Execution and Batch Coordination (2–4 days)

- Content: extract the local scan into a shared service that reuses the lock, source settings, and scan results in `api.rs`; implement remote jobs, idempotency, rate limiting, controlled concurrency, staged status, snapshot pulling, and local batch queries.
- Deliverables: the local scan service, the remote job interface, the batch status and error codes; the local page's timed refresh behavior is unchanged.
- Acceptance: two devices each scan exactly once with no cascading; lock contention, duplicate requests, crash/timeout, partial source failure, offline, and failed sync stores all produce the correct status; the batch returns "All complete" only when every target succeeded; closing and reopening the page recovers the state or marks it unknown.
- Dependencies: T1; it can start with mocks in parallel with T2/T3, while the end-to-end work depends on T2/T3.

### T5: Pairing and Scan Status UI (2–3 days)

- Content: `DeviceManager.tsx` supports requests, verification codes, consent/rejection, waiting/expired/revoked states, and legacy notices; `UsageWorkspace.tsx` shows the per-device status, progress, errors, and the final aggregate in the multi-device view; the local page behavior is unchanged.
- Deliverables: UI components, accessible status text and interactions, and controlled refresh and recovery logic. While waiting for the API, use mocks that match T1.
- Acceptance: pairing and rejection are completable with mouse or keyboard; both devices show the same verification code; an in-progress action cannot be triggered twice; the wording stays accurate for a mix of successful, failed, offline, legacy, and not-participating devices; light and dark themes and narrow windows do not truncate the content; closing the panel does not lose the real job status.
- Dependencies: the T1 contract; it can run in parallel with T2/T3/T4, while the final integration testing depends on the corresponding interfaces.

### T6: Test Design and Automation (2–3 days, in parallel with development)

- Content: add backend unit/integration tests, protocol negative tests, and frontend state and interaction tests based on the contract; set up a two-instance local test environment and a two-machine manual checklist.
- Acceptance: the scenarios in Section 7 are covered; the tests use a temporary data directory and must not connect to or delete real devices; they can be re-run reliably; every defect records reproduction steps, redacted logs, and the expected result.
- Dependencies: authoring can start against the T1 contract; the full test run depends on T2–T5.

### T7: Migration, Integration Testing, and Release (1–2 days, excluding cloud build wait)

- Content: implement the migration of existing devices and the accompanying user guidance; upgrade and pair the Windows/Mac machines; run the regression and release workflows and confirm the installer packages and the auto-update manifest.
- Acceptance: after the upgrade, old devices are untrusted by default and the UI clearly prompts for re-pairing; historical raw data is preserved and old snapshots are not added to the aggregate as authorized data; the Windows x64 installer and the Apple Silicon DMG are published only after the Section 7 release gate is passed; the auto-update signature and manifest are checked. The macOS notarization status is described as it actually stands.
- Dependencies: T0 and T2–T6 are complete and have passed review.

## 6. Parallel Development Plan

```text
T0 Environment setup ──────────────────────────────────────────────────────────────────┐
T1 Security protocol and contract ─┬─ T2 Pairing and authorization storage ──────────┤
                                   ├─ T3 Authenticated transport and migration ──────┤
                                   ├─ T4 Remote scan and batch coordination ─────────┤─ T7 Integration, migration and release
                                   ├─ T5 Device manager and scan status UI ──────────┤
                                   └─ T6 Test design and automation ─────────────────┘
```

Four development lines are recommended: ① T2 after T1; ② T3; ③ T4; ④ T5. Testing starts on T6 from T1 onward; if the team is small, one person completes T2/T3 sequentially while T4/T5 stay parallel. T4 uses mocks temporarily until the authorization and transport interfaces are done; T5 does not modify server-side files directly. `api.rs`, the shared types, and the migration scripts are high-conflict areas, so the interface owner coordinates the merges to avoid two people editing the same area at once.

The critical path is usually T1 → T2/T3 integration → T4 end-to-end → T7. A rough estimate for the parallel cycle is 7–12 effective working days, and 12–20 days for a single person; both need adjustment based on the protocol, the security review, and the results of debugging on two machines.

## 7. Acceptance Checklist

### 7.1 Pairing and Security

- [ ] A initiates and B rejects: no authorization, no usage, no scan; the UI shows the rejection.
- [ ] The verification codes differ, only one side confirms, or the request times out or is cancelled: no sync and no scan can run.
- [ ] After both sides confirm, restarting both ends keeps the authorization usable; an unpaired third device cannot read usage, query jobs, or start a scan.
- [ ] After changing the IP or port, rediscovery still verifies the original device identity; an impersonated name or a swapped certificate is rejected.
- [ ] After A revokes B, the local machine immediately rejects B; B receives the revocation while online, and while offline the local machine marks it as pending notification and retries; reconnecting requires a new confirmation.
- [ ] Revoking the authorization while a sync is in flight: a late snapshot must not be stored again or enter the aggregate; simultaneous revocation on both sides and repeated notifications must not restore permissions or deadlock the notification.
- [ ] The legacy plaintext endpoint cannot be read; packet capture confirms that usage and credentials are not readable in plaintext; logs, errors, and discovery packets contain no private keys or tokens.
- [ ] Request flooding, replay, repeated confirmation, and scan concurrency are all limited without affecting normal pairing; the error and re-pairing flow after a private key is corrupted or reset is clearly defined.

### 7.2 Multi-Device Scanning

- [ ] Two devices add different test records; after clicking the multi-device scan each scans once, the aggregate shows the new records from both sides with no duplicates, and each device's completion time plus the overall success count are shown.
- [ ] Three devices mixing success, offline, and a scan source error: each shows its real result, the overall state is "Not all complete", and data from successful devices refreshes normally.
- [ ] The remote scan succeeds but the snapshot pull or store fails: a sync failure is shown, the old cache and its time are kept, and "All complete" is not reported.
- [ ] A scan timeout, a dropped connection, closing and reopening the page, or an application restart: the job is recoverable or shows "Unknown status", and no second scan is created.
- [ ] A retry with the same idempotency key, simultaneous requests from both ends, and competition with the background scan: compatible in-progress scans are merged and share the result; there is no deadlock and no cascading, and a new request after the scan completes may start a new round.
- [ ] Disabled devices do not enter the target list; an unpaired legacy device shows that re-pairing is required and must not create the impression that everything completed; the configured shared app scope takes effect on both sides.
- [ ] Scans and auto-refresh on the local page do not trigger remote scans; timed pulls also visit only authorized devices.

### 7.3 Upgrade, Compatibility, and Release

- [ ] Upgrade a test database copied from the real v0.1.26 structure; registered devices become "needs re-pairing" and are no longer pulled without consent; the local raw usage and prices are unchanged.
- [ ] Verify that historical caches do not enter the aggregate while unauthorized; unpairing a device deletes only its own copy, and other devices' and the local data are unchanged.
- [ ] An old Observatory or CC Switch shows that the protocol is incompatible, with no silent downgrade; once both pairing sides have upgraded the connection can be re-established.
- [ ] Run the frontend tests, type check, and build, plus the Rust core tests and relevant integration tests; record the commands and results. Verify the firewall, sleep/wake, and service restart separately on both machines.
- [ ] The Windows x64 and Apple Silicon Mac installers, the update signature, and `latest.json` are usable; the release notes state that both ends must be upgraded and re-paired. Nothing is released before all security and data integrity items pass.

## 8. Open Questions for T1

1. Which mature TLS identity-binding and mutual authentication scheme to use, and how the short verification code is derived from both sides' handshake material so that it resists a man-in-the-middle.
2. Whether credentials live in the system secure store or in a local file with restricted permissions, and how to recover when permissions are insufficient or a migration fails.
3. How to resolve conflicts when both sides start pairing at the same time, when a device is restored from an old backup, or when duplicate identities appear.
4. How long job results and batch states are retained, and how to recognize after an application restart that a remote job is still running.
5. The boundaries of address discovery and certificate verification across subnets, hostnames, IPv6, or proxied environments; an IP address must not be relied on as identity.

These questions belong to the protocol design task and do not require the user to make a technical choice up front; the development owner should settle them and have them reviewed before implementation.
