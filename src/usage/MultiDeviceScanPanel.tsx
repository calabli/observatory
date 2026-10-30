import { CheckCircle2, Circle, CircleAlert, LoaderCircle } from "lucide-react";
import type { DeviceScan, ScanBatch, ScanState } from "./multiDeviceScan";
import "./multiDeviceScan.css";

const labels: Record<ScanState, string> = {
  waiting: "等待中",
  scanning: "扫描中",
  syncing: "同步中",
  completed: "已完成",
  partial_failure: "部分失败",
  failed: "更新失败",
  timed_out: "等待超时",
  cancelled: "已取消",
  unknown: "状态未知",
  not_participating: "未参与",
};
const time = (value: number) =>
  new Date(value).toLocaleTimeString("zh-CN", { hour12: false });
function DeviceRow({ device }: { device: DeviceScan }) {
  const running = ["waiting", "scanning", "syncing"].includes(device.state);
  const Icon = running
    ? LoaderCircle
    : device.state === "completed"
      ? CheckCircle2
      : device.state === "not_participating"
        ? Circle
        : CircleAlert;
  const detail =
    device.error ||
    (device.state === "not_participating"
      ? device.enabled
        ? "需要先完成配对"
        : "设备已停用"
      : device.completedAt
        ? `完成于 ${time(device.completedAt)}`
        : "");
  return (
    <li className="multi-scan-device">
      <div className="multi-scan-device-info">
        <strong>
          {device.deviceName}
          {device.local ? "（本机）" : ""}
        </strong>
        {detail && <span>{detail}</span>}
        {device.state !== "completed" && device.lastSyncAt != null && (
          <span>
            最近成功同步：
            {new Date(device.lastSyncAt * 1000).toLocaleString("zh-CN")}
          </span>
        )}
      </div>
      <span className={`multi-scan-state multi-scan-state--${device.state}`}>
        <Icon size={15} aria-hidden="true" />
        {labels[device.state]}
      </span>
    </li>
  );
}
export function MultiDeviceScanPanel({ batch }: { batch: ScanBatch }) {
  const completed = batch.devices.filter((d) => d.state === "completed").length;
  const total = batch.devices.filter(
    (d) => d.state !== "not_participating",
  ).length;
  const heading =
    batch.status === "running"
      ? "正在更新设备"
      : batch.status === "completed"
        ? "全部更新完成"
        : "未全部完成";
  return (
    <section
      className="multi-scan-panel"
      role="status"
      aria-live="polite"
      aria-label="多设备更新状态"
    >
      <div className="multi-scan-head">
        <div>
          <strong>{heading}</strong>
          <p>
            参与本轮的设备 {total} 台 · 已完成 {completed} 台
          </p>
        </div>
        <span className="multi-scan-batch-time">
          开始于 {time(batch.startedAt)}
        </span>
      </div>
      <ul>
        {batch.devices.map((d) => (
          <DeviceRow key={d.deviceId} device={d} />
        ))}
      </ul>
      {batch.devices.some(
        (d) => d.state === "timed_out" || d.state === "unknown",
      ) && (
        <p className="multi-scan-hint">
          部分设备状态未知或等待超时，远端扫描可能仍在进行。
        </p>
      )}
    </section>
  );
}
