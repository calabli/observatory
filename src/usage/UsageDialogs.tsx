import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usageCall } from "../native/usageClient";
import {
  getFreshInputTokens,
  isNotBillableUsage,
  type RequestLog,
} from "../types/usage";
import { formatMoney, friendlyError, UsageDialog } from "./ui";
import { modelDisplayName } from "./modelDisplay";
import { metricValue, timeText } from "./usageShared";
import { formatRequestSpeed } from "./requestSpeed";
import { type NativeStatus } from "./types";
import { FileDown } from "lucide-react";

export function RequestDetail({
  id,
  multi,
  deviceIds,
  close,
}: {
  id: string;
  multi: boolean;
  deviceIds?: string[];
  close: () => void;
}) {
  const data = useQuery({
    queryKey: ["usage", "detail", multi, deviceIds, id],
    queryFn: () =>
      usageCall<RequestLog | null>("request_detail", {
        requestId: id,
        multiDevice: multi,
        deviceIds,
      }),
    retry: false,
  });
  const r = data.data;
  return (
    <UsageDialog title="请求详情" close={close} wide>
      {data.error ? (
        <p className="usage-error">{friendlyError(data.error)}</p>
      ) : !r ? (
        <p className="hint">
          {data.isLoading ? "正在读取详情…" : "明细不存在或已按日汇总"}
        </p>
      ) : (
        <>
          <div className="usage-detail-grid">
            {[
              ["应用", r.appType],
              ["供应商", r.providerName ?? r.providerId],
              [
                "响应模型",
                modelDisplayName(r.model) === r.model
                  ? r.model
                  : `${modelDisplayName(r.model)} (${r.model})`,
              ],
              ["请求模型", r.requestModel ?? "—"],
              ["计价模型", r.pricingModel ?? "未记录"],
              ["计费倍数", r.costMultiplier],
              ["状态码", r.statusCode],
              ["来源", r.dataSource ?? "proxy"],
              ["时间", timeText(r.createdAt)],
              [
                "请求耗时",
                r.latencyMs
                  ? (r.dataSource !== "proxy" ? "≈ " : "") +
                    (r.latencyMs / 1000).toFixed(3) +
                    "s"
                  : "—",
              ],
              [
                "首字时间",
                r.firstTokenMs != null
                  ? (r.dataSource !== "proxy" ? "≈ " : "") +
                    r.firstTokenMs +
                    "ms"
                  : "—",
              ],
              ["是否流式", r.isStreaming ? "是" : "否"],
              ["输出速度", formatRequestSpeed(r)],
            ].map(([label, value]) => (
              <div key={label}>
                <span>{label}</span>
                <strong>{value}</strong>
              </div>
            ))}
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>计费项</th>
                  <th className="numeric">Token</th>
                  <th className="numeric">金额 USD</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["新增输入", getFreshInputTokens(r), r.inputCostUsd],
                  ["输出", r.outputTokens, r.outputCostUsd],
                  ["缓存读取", r.cacheReadTokens, r.cacheReadCostUsd],
                  ["缓存写入", r.cacheCreationTokens, r.cacheCreationCostUsd],
                  ["合计", metricValue(r), r.totalCostUsd],
                ].map(([name, t, c]) => (
                  <tr key={name}>
                    <td>{name}</td>
                    <td className="numeric">{Number(t).toLocaleString()}</td>
                    <td className="numeric">{formatMoney(c, 6)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {isNotBillableUsage(r) && (
            <p className="hint">
              该模型没有公开单价，用量计入套餐，本身不产生费用。
            </p>
          )}
          {r.errorMessage && <p className="usage-error">{r.errorMessage}</p>}
          <p className="hint usage-request-id">请求标识：{r.requestId}</p>
          <p className="hint">
            ≈ 表示按日志时间戳估算的耗时，与代理记录的精确计时分开解释。
          </p>
        </>
      )}
    </UsageDialog>
  );
}

export function SourcesDialog({
  close,
  status,
  importPath,
  setImportPath,
  importBusy,
  importHistory,
  importError,
  importResult,
}: {
  importError?: unknown;
  importResult?: string;
  close: () => void;
  status?: NativeStatus;
  importPath: string;
  setImportPath: (v: string) => void;
  importBusy: boolean;
  importHistory: () => void;
}) {
  const qc = useQueryClient();
  const enabled =
    status?.sources.filter((a) => a.enabled).map((a) => a.id) ?? [];
  const save = useMutation({
    mutationFn: (selected: string[]) =>
      usageCall("sources_save", { enabled: selected }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["usage", "status"] });
    },
  });
  return (
    <UsageDialog title="数据来源与历史迁移" close={close} wide>
      <p className="hint">
        原生日志以只读方式读取，开关控制后续扫描，不删除已有记录。独立数据库：
        {status?.databasePath}
      </p>
      {save.error && <p className="usage-error">{friendlyError(save.error)}</p>}
      <div className="usage-section-title">
        <h3>应用采集范围</h3>
        <button
          className="text-button"
          disabled={save.isPending}
          onClick={() => save.mutate(status?.sources.map((a) => a.id) ?? [])}
        >
          全部开启
        </button>
      </div>
      <div className="usage-sources">
        {status?.sources.map((a) => (
          <label key={a.id}>
            <span>
              <strong>{a.name}</strong>
              <small>
                {a.nativeSupported
                  ? "原生日志采集"
                  : "兼容 CC Switch 历史与代理统计导入"}{" "}
                · {a.requests.toLocaleString()} 次历史请求
              </small>
            </span>
            <input
              className="toggle"
              type="checkbox"
              aria-label={"采集 " + a.name}
              disabled={save.isPending}
              checked={a.enabled}
              onChange={(e) =>
                save.mutate(
                  e.target.checked
                    ? [...enabled, a.id]
                    : enabled.filter((id) => id !== a.id),
                )
              }
            />
          </label>
        ))}
      </div>
      <div className="usage-editor">
        <h3>只读导入 CC Switch 历史用量</h3>
        {!!importError && (
          <p role="alert" className="usage-error">
            {friendlyError(importError)}
          </p>
        )}
        {importResult && (
          <p className="usage-success" role="status">
            {importResult}
          </p>
        )}
        <p className="hint">
          带入统计明细、日汇总、扫描游标和去重记录；供应商仅保留名称，不复制配置和密钥。重复导入不会重复累计。
        </p>
        <label className="field">
          CC Switch 数据库路径
          <input
            aria-label="CC Switch 数据库路径"
            value={importPath}
            onChange={(e) => setImportPath(e.target.value)}
          />
        </label>
        <div className="dialog-actions">
          <button
            className="button primary"
            disabled={importBusy}
            onClick={importHistory}
          >
            <FileDown size={13} />
            {importBusy ? "导入中…" : "导入历史用量"}
          </button>
        </div>
      </div>
      <p className="hint">
        Hermes、OpenClaw、Claude Desktop
        当前通过已有统计导入或对端共享接入；尚未提供独立日志采集器。
      </p>
    </UsageDialog>
  );
}
