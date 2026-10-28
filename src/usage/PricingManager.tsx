import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Download,
  Plus,
  RefreshCw,
  Search,
  Check,
  Pencil,
  Trash2,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import type { ModelPricing } from "../types/usage";
import { isNonNegativeDecimalString } from "../types/usage";
import { usageApi } from "../lib/api/usage";
import { syncModelsDevPricing } from "../lib/modelsDevAutoSync";
import { fetchModelsDev } from "../lib/modelsDev";
import { flattenModels, toModelPricing } from "../lib/modelsDevPricing";
import { UsageDialog, friendlyError } from "./ui";

const blank: ModelPricing = {
  modelId: "",
  displayName: "",
  inputCostPerMillion: "0",
  outputCostPerMillion: "0",
  cacheReadCostPerMillion: "0",
  cacheCreationCostPerMillion: "0",
};
export function PricingManager({ close }: { close: () => void }) {
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [catalogSearch, setCatalogSearch] = useState("");
  const [editing, setEditing] = useState<ModelPricing | null>(null);
  const [catalog, setCatalog] = useState(false);
  const [message, setMessage] = useState("");
  const prices = useQuery({
    queryKey: ["usage", "prices"],
    queryFn: () => usageCall<ModelPricing[]>("pricing_list"),
  });
  const config = useQuery({
    queryKey: ["usage", "price-config"],
    queryFn: usageApi.getModelsDevSyncConfig,
  });
  const remote = useQuery({
    queryKey: ["usage", "models-dev-catalog"],
    queryFn: fetchModelsDev,
    enabled: catalog,
    staleTime: 3600000,
  });
  const entries = useMemo(
    () =>
      remote.data
        ? flattenModels(remote.data).filter((x) =>
            (x.modelName + " " + x.modelId + " " + x.providerName)
              .toLowerCase()
              .includes(catalogSearch.toLowerCase()),
          )
        : [],
    [remote.data, catalogSearch],
  );
  const changed = async () => {
    await qc.invalidateQueries({ queryKey: ["usage"] });
  };
  const save = useMutation({
    mutationFn: (value: ModelPricing) => usageCall("pricing_update", value),
    onSuccess: async () => {
      setEditing(null);
      setMessage("模型价格已保存；缺失费用的历史记录已尝试补算。");
      await changed();
    },
  });
  const updateConfig = useMutation({
    mutationFn: usageApi.saveModelsDevSyncConfig,
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["usage", "price-config"] });
    },
  });
  const sync = useMutation({
    mutationFn: () => syncModelsDevPricing(config.data, true),
    onSuccess: async (r) => {
      setMessage(`已同步 ${r.imported} 个模型，更新 ${r.changed} 项价格。`);
      await changed();
    },
  });
  const importPrice = useMutation({
    mutationFn: async (key: string) => {
      const entry =
        remote.data && flattenModels(remote.data).find((x) => x.key === key);
      if (!entry) throw new Error("未找到所选模型");
      await usageApi.updateModelPricingBatch(toModelPricing([entry]));
      if (config.data)
        await usageApi.saveModelsDevSyncConfig({
          ...config.data.config,
          selectedModelKeys: [
            ...new Set([...config.data.config.selectedModelKeys, key]),
          ],
        });
    },
    onSuccess: async () => {
      setMessage("已导入定价并加入后续同步范围。");
      setCatalog(false);
      await changed();
    },
  });
  const remove = useMutation({
    mutationFn: (modelId: string) => usageCall("pricing_delete", { modelId }),
    onSuccess: changed,
  });
  const error =
    prices.error ||
    config.error ||
    remote.error ||
    save.error ||
    updateConfig.error ||
    sync.error ||
    importPrice.error ||
    remove.error;
  const rows =
    prices.data?.filter((p) =>
      (p.modelId + " " + p.displayName)
        .toLowerCase()
        .includes(search.toLowerCase()),
    ) ?? [];
  return (
    <UsageDialog title="模型价格管理" close={close} wide>
      <div className="price-sync-header">
        <div>
          <strong>models.dev 自动同步</strong>
          <p>启动时检查，距离上次成功同步满 6 小时才再次拉取。</p>
        </div>
        <button
          className="button"
          onClick={() => sync.mutate()}
          disabled={sync.isPending || !config.data}
        >
          <RefreshCw size={13} />
          {sync.isPending ? "同步中…" : "立即同步"}
        </button>
      </div>
      {config.data && (
        <div className="usage-switches">
          <label>
            <input
              type="checkbox"
              checked={config.data.config.autoSyncEnabled}
              disabled={updateConfig.isPending}
              onChange={(e) =>
                updateConfig.mutate({
                  ...config.data!.config,
                  autoSyncEnabled: e.target.checked,
                })
              }
            />
            自动同步价格
          </label>
          <label>
            <input
              type="checkbox"
              checked={config.data.config.includeCommonModels}
              disabled={updateConfig.isPending}
              onChange={(e) =>
                updateConfig.mutate({
                  ...config.data!.config,
                  includeCommonModels: e.target.checked,
                })
              }
            />
            包括常用模型
          </label>
          <span className="muted">
            {config.data.config.lastSyncAt
              ? "最近同步 " +
                new Date(config.data.config.lastSyncAt).toLocaleString()
              : "尚未自动同步"}
          </span>
        </div>
      )}
      {config.data?.config.lastSyncError && (
        <p className="usage-error">
          上次同步失败：{config.data.config.lastSyncError}
        </p>
      )}
      <div className="usage-note">
        单位：USD / 百万
        Token。手动价格若仍参与自动同步，后续同步可能覆盖它。已有非零历史费用不会整体重算。
      </div>
      {message && (
        <div className="usage-success" role="status">
          <Check size={14} />
          {message}
        </div>
      )}
      {error && (
        <p className="usage-error" role="alert">
          {friendlyError(error)}
        </p>
      )}
      <div className="filter-row">
        <label className="search">
          <Search size={13} />
          <input
            placeholder="搜索模型定价"
            aria-label="搜索模型定价"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </label>
        <div className="filters">
          <button className="button" onClick={() => setCatalog(!catalog)}>
            <Download size={13} />从 models.dev 导入
          </button>
          <button
            className="button primary"
            onClick={() => setEditing({ ...blank })}
          >
            <Plus size={13} />
            新增价格
          </button>
        </div>
      </div>
      {catalog && (
        <section className="usage-editor">
          <h3>选择模型与价格来源</h3>
          <input
            className="usage-input"
            placeholder="搜索模型、厂商"
            aria-label="搜索 models.dev 模型"
            value={catalogSearch}
            onChange={(e) => setCatalogSearch(e.target.value)}
          />
          {remote.isLoading ? (
            <p className="muted">加载模型目录…</p>
          ) : (
            <div className="usage-catalog">
              {entries.slice(0, 40).map((e) => (
                <div key={e.key}>
                  <span>
                    <strong>{e.modelName}</strong>
                    <small>
                      {e.providerName} · {e.modelId}
                    </small>
                  </span>
                  <code>
                    {e.input} / {e.output}
                  </code>
                  <button
                    className="button"
                    disabled={importPrice.isPending}
                    onClick={() => importPrice.mutate(e.key)}
                  >
                    导入
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}
      {editing && (
        <section className="usage-editor">
          <h3>编辑模型价格</h3>
          <div className="usage-price-fields">
            {(
              [
                ["modelId", "模型 ID"],
                ["displayName", "显示名称"],
                ["inputCostPerMillion", "输入"],
                ["outputCostPerMillion", "输出"],
                ["cacheReadCostPerMillion", "缓存读取"],
                ["cacheCreationCostPerMillion", "缓存写入"],
              ] as [keyof ModelPricing, string][]
            ).map(([key, label]) => (
              <label key={key}>
                {label}
                <input
                  aria-label={label + "价格字段"}
                  value={editing[key]}
                  onChange={(e) =>
                    setEditing({ ...editing, [key]: e.target.value })
                  }
                />
              </label>
            ))}
          </div>
          <div className="dialog-actions">
            <button className="button" onClick={() => setEditing(null)}>
              取消编辑
            </button>
            <button
              className="button primary"
              disabled={save.isPending}
              onClick={() => {
                if (!editing.modelId.trim() || !editing.displayName.trim()) {
                  setMessage("请填写模型 ID 和显示名称。");
                  return;
                }
                if (
                  ![
                    editing.inputCostPerMillion,
                    editing.outputCostPerMillion,
                    editing.cacheReadCostPerMillion,
                    editing.cacheCreationCostPerMillion,
                  ].every(isNonNegativeDecimalString)
                ) {
                  setMessage("价格必须是非负十进制数。");
                  return;
                }
                save.mutate(editing);
              }}
            >
              保存价格
            </button>
          </div>
        </section>
      )}
      <div className="table-wrap usage-price-table">
        <table>
          <thead>
            <tr>
              <th>模型</th>
              <th className="numeric">输入</th>
              <th className="numeric">输出</th>
              <th className="numeric">缓存读取</th>
              <th className="numeric">缓存写入</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {rows.slice(0, 100).map((p) => (
              <tr key={p.modelId}>
                <td>
                  {p.displayName}
                  <small className="cell-sub">{p.modelId}</small>
                </td>
                <td className="numeric">{p.inputCostPerMillion}</td>
                <td className="numeric">{p.outputCostPerMillion}</td>
                <td className="numeric">{p.cacheReadCostPerMillion}</td>
                <td className="numeric">{p.cacheCreationCostPerMillion}</td>
                <td>
                  <span className="inline">
                    <button
                      className="icon-button"
                      aria-label={"编辑 " + p.modelId}
                      onClick={() => setEditing({ ...p })}
                    >
                      <Pencil size={12} />
                    </button>
                    <button
                      className="icon-button"
                      aria-label={"删除 " + p.modelId}
                      onClick={() => {
                        if (
                          window.confirm(
                            `删除 ${p.modelId} 的定价？已有请求与费用会保留。`,
                          )
                        )
                          remove.mutate(p.modelId);
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="hint">
        共 {rows.length} 个模型
        {rows.length > 100 ? "，显示前 100 个，请搜索缩小范围" : ""}
        。价格配置独立保存于 {config.data?.configPath ?? "观测台数据目录"}。
      </p>
    </UsageDialog>
  );
}
