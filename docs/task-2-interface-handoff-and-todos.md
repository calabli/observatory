# Task 2 Interface Handoff and To-Do List

The user has switched to a simple pairing code scheme. The current worktree wires up the pairing code, LAN sync, and the remote scan UI, and has passed the local simulation and two-instance tests; a real Windows/Mac two-machine test has not been done yet. LAN requests use plaintext HTTP, and the pairing code and credentials get no encryption protection, so they only suit trusted networks.

Pairing is now a single step that completes the two-way registration: both ends must have LAN sharing enabled and be able to reach each other, one side generates the pairing code and the other enters it. If the initiating device cannot be reached back, the pairing fails and the pairing code is not consumed.

## Existing Prototype

- `src/usage/multiDeviceScan.ts`: injects the local scan, the remote create/wait job, and the sync through `ScanTransport`; fixes the targets, limits the concurrency, and produces the per-device status. The state lives only in memory; the transport is responsible for authentication, job idempotency, timeouts, and validation.
- `src/usage/MultiDeviceScanPanel.tsx`: renders the device status, the aggregate, and the cache time. The time fields `startedAt` and `completedAt` use milliseconds; the existing `lastSyncAt` uses seconds.
- `src/usage/DeviceManager.tsx`: generates a one-time local pairing code, accepts the peer's pairing code, and manages the authorized devices.
- `tests/multiDeviceScan.test.mjs`: uses mocks to check success, source errors, the unpaired case, a sync failure, the concurrency limit, and duplicate starts.
- `native/src/api.rs`: extracts `scan_local`, which scans only the local machine, reuses the same lock as the background scan, and does not perform remote sync.
- `native/src/scan_jobs.rs`: an in-memory job manager that supports lookup by requester and idempotency key, merging concurrent jobs, and failure states; it is already wired to the pairing authorization and the local scan. The jobs disappear when the process restarts and the query then shows the status as unknown; historical jobs are cleaned up after one day.

## Must Complete Before Integration Testing

1. Finish the end-to-end test on real Windows and Apple Silicon Mac hardware, including LAN discovery, the firewall, pairing, reading, scanning, device disconnects, and revocation.
2. Improve the job retention and cleanup, the restart recovery, and the individual retry of failed devices. Today the jobs are lost after the service restarts and the frontend marks the unfinished jobs with an unknown status.
3. Clarify the offline revocation experience: when the connection is down and the remote side is not online, the user has to revoke manually in the peer's "devices allowed to access this machine"; there is currently no automatic retry.
4. Re-examine the security acceptance requirements of the master plan against the simplified scheme; this implementation does not meet the encrypted communication and certificate authentication requirements, so acceptance cannot be claimed against the original plan's security bar.
