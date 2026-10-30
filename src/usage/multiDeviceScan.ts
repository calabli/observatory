/** Task 2 contract. Transport and authorization are supplied by the pairing layer. */
export type ScanState =
  | "waiting"
  | "scanning"
  | "syncing"
  | "completed"
  | "partial_failure"
  | "failed"
  | "timed_out"
  | "cancelled"
  | "unknown"
  | "not_participating";
export type Target = {
  deviceId: string;
  deviceName: string;
  local: boolean;
  enabled: boolean;
  paired: boolean;
  lastSyncAt?: number | null;
};
export type DeviceScan = Target & {
  state: ScanState;
  error?: string;
  completedAt?: number;
  imported?: number;
  jobId?: string;
};
export type ScanBatch = {
  batchId: string;
  devices: DeviceScan[];
  startedAt: number;
  finishedAt?: number;
  status: "running" | "completed" | "incomplete";
};
export type ScanResult = { errors: string[]; imported?: number };
export interface ScanTransport {
  scanLocal(batchId: string): Promise<ScanResult>;
  startRemote(deviceId: string, batchId: string): Promise<{ jobId: string }>;
  waitRemote(deviceId: string, jobId: string): Promise<ScanResult>;
  syncRemote(deviceId: string): Promise<{ imported: number }>;
}

const active = new Set<ScanState>(["waiting", "scanning", "syncing"]);
export function batchStatus(devices: DeviceScan[]): ScanBatch["status"] {
  if (devices.some((d) => active.has(d.state))) return "running";
  if (
    devices.some(
      (d) =>
        (d.enabled && !d.paired && !d.local) ||
        (d.state !== "completed" && d.state !== "not_participating"),
    )
  )
    return "incomplete";
  return devices.some((d) => d.state === "completed")
    ? "completed"
    : "incomplete";
}

/** An in-memory coordinator for integration tests and a future authenticated adapter.
 * It must be hosted by the backend before enabling production remote scans. */
export class MultiDeviceScanCoordinator {
  private current: ScanBatch | null = null;
  private running = false;
  constructor(
    private transport: ScanTransport,
    private publish: (batch: ScanBatch) => void,
    private concurrency = 3,
  ) {}
  snapshot(): ScanBatch | null {
    return this.current ? structuredClone(this.current) : null;
  }
  private update(index: number, patch: Partial<DeviceScan>) {
    if (!this.current) return;
    Object.assign(this.current.devices[index], patch);
    this.current.status = batchStatus(this.current.devices);
    this.publish(structuredClone(this.current));
  }
  async start(batchId: string, targets: Target[]): Promise<ScanBatch> {
    if (this.running) throw new Error("已有扫描任务正在进行");
    if (
      !batchId ||
      !targets.some((t) => t.local) ||
      new Set(targets.map((t) => t.deviceId)).size !== targets.length
    )
      throw new Error("扫描目标无效");
    this.running = true;
    this.current = {
      batchId,
      startedAt: Date.now(),
      status: "running",
      devices: targets.map((t) => ({
        ...t,
        state:
          t.local || (t.enabled && t.paired) ? "waiting" : "not_participating",
      })),
    };
    this.publish(structuredClone(this.current));
    const indexes = this.current.devices
      .map((_, i) => i)
      .filter((i) => this.current!.devices[i].state === "waiting");
    let cursor = 0;
    const worker = async () => {
      while (cursor < indexes.length) {
        const index = indexes[cursor++];
        const target = this.current!.devices[index];
        try {
          this.update(index, { state: "scanning" });
          const result = target.local
            ? await this.transport.scanLocal(batchId)
            : await (async () => {
                const { jobId } = await this.transport.startRemote(
                  target.deviceId,
                  batchId,
                );
                this.update(index, { jobId });
                return this.transport.waitRemote(target.deviceId, jobId);
              })();
          let imported = result.imported;
          if (!target.local) {
            this.update(index, { state: "syncing" });
            imported = (await this.transport.syncRemote(target.deviceId))
              .imported;
          }
          this.update(index, {
            state: result.errors.length ? "partial_failure" : "completed",
            error: result.errors.join("；") || undefined,
            imported,
            completedAt: Date.now(),
          });
        } catch (error) {
          const code =
            typeof error === "object" && error !== null && "code" in error
              ? String(error.code)
              : "";
          this.update(index, {
            state: code === "timeout" ? "timed_out" : "failed",
            error: error instanceof Error ? error.message : "更新失败",
            completedAt: Date.now(),
          });
        }
      }
    };
    await Promise.all(
      Array.from(
        { length: Math.min(Math.max(1, this.concurrency), indexes.length) },
        worker,
      ),
    );
    this.current.finishedAt = Date.now();
    this.current.status = batchStatus(this.current.devices);
    this.publish(structuredClone(this.current));
    this.running = false;
    return structuredClone(this.current);
  }
}
