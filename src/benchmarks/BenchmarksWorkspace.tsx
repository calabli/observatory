import React, { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  Check,
  Cpu,
  Gauge,
  KeyRound,
  Layers,
  List,
  RefreshCw,
  Search,
  Sparkles,
  Trophy,
  X,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import type { Dashboard } from "../usage/types";
import { formatTokens } from "../usage/ui";
import {
  BENCHMARK_DIMENSIONS,
  MODEL_TIER_DEFS,
  blendedPrice1m,
  creatorColor,
  effectiveTps,
  effectiveTtftS,
  formatPrice1m,
  formatScore,
  formatSeconds,
  formatTps,
  getDimensionScore,
  groupModelFamilies,
  matchLocalUsage,
  parseModelVariant,
  type BenchmarkDimensionKey,
  type BenchmarkModel,
  type BenchmarkSnapshot,
  type BenchmarksState,
  type LocalModelMatch,
  type ModelFamilyGroup,
  type ModelFamilyVariant,
  type ModelTierKey,
} from "./benchmarksShared";
import {
  BenchmarkChartsPanel,
  type BenchmarkChartMode,
} from "./BenchmarkCharts";

type SortColumn =
  | "score"
  | "intelligence"
  | "engineering"
  | "price"
  | "tps"
  | "ttft"
  | "releaseDate"
  | "localRequests";

type TableViewMode = "grouped" | "flat";

export function BenchmarksWorkspace() {
  const qc = useQueryClient();

  const [dimension, setDimension] =
    useState<BenchmarkDimensionKey>("intelligenceIndex");
  const [creatorFilter, setCreatorFilter] = useState<string>("all");
  const [tierFilter, setTierFilter] = useState<"all" | ModelTierKey>("all");
  const [viewMode, setViewMode] = useState<TableViewMode>("grouped");
  const [onlyScored, setOnlyScored] = useState<boolean>(true);
  const [onlyLocal, setOnlyLocal] = useState<boolean>(false);
  const [searchText, setSearchText] = useState<string>("");
  const [chartMode, setChartMode] = useState<BenchmarkChartMode>("bar");
  const [sortCol, setSortCol] = useState<SortColumn>("score");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState<boolean>(false);
  const [keyInput, setKeyInput] = useState<string>("");
  const [tierInput, setTierInput] = useState<"free" | "pro">("free");
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const autoFetchedRef = useRef(false);

  const stateQuery = useQuery({
    queryKey: ["benchmarks", "state"],
    queryFn: () => usageCall<BenchmarksState>("benchmarks_get", {}),
  });

  // Load local model stats (all-time) to correlate benchmark models with real local usage
  const localDashQuery = useQuery({
    queryKey: ["benchmarks", "local-models"],
    queryFn: () =>
      usageCall<Dashboard>("dashboard", {
        multiDevice: false,
        page: 1,
        pageSize: 1,
      }),
    staleTime: 60_000,
  });

  const fetchMutation = useMutation({
    mutationFn: () => usageCall<BenchmarkSnapshot>("benchmarks_fetch", {}),
    onSuccess: (snap) => {
      qc.setQueryData<BenchmarksState | undefined>(
        ["benchmarks", "state"],
        (prev) =>
          prev
            ? { ...prev, snapshot: snap }
            : {
                keyConfigured: true,
                keyHint: null,
                tier: snap.tier,
                snapshot: snap,
              },
      );
      setStatusMessage(
        `已拉取 ${snap.models.length} 个模型（共 ${snap.totalPages} 页）`,
      );
    },
  });

  const saveConfigMutation = useMutation({
    mutationFn: (payload: {
      apiKey?: string;
      clearApiKey?: boolean;
      tier?: "free" | "pro";
    }) => usageCall<BenchmarksState>("benchmarks_config_save", payload),
    onSuccess: (next, vars) => {
      qc.setQueryData(["benchmarks", "state"], next);
      setKeyInput("");
      setSettingsOpen(false);
      if (vars.clearApiKey) {
        setStatusMessage("已清除 Artificial Analysis API Key");
      } else {
        setStatusMessage("配置已保存");
        if (next.keyConfigured && !next.snapshot) {
          fetchMutation.mutate();
        }
      }
    },
  });

  const state = stateQuery.data;
  const snapshot = state?.snapshot ?? null;
  const allModels = useMemo(() => snapshot?.models ?? [], [snapshot]);

  // Auto-fetch once when key is configured but no cached snapshot exists yet
  useEffect(() => {
    if (
      state?.keyConfigured &&
      !state.snapshot &&
      !autoFetchedRef.current &&
      !fetchMutation.isPending
    ) {
      autoFetchedRef.current = true;
      fetchMutation.mutate();
    }
  }, [state?.keyConfigured, state?.snapshot, fetchMutation]);

  useEffect(() => {
    if (state?.tier === "pro" || state?.tier === "free") {
      setTierInput(state.tier);
    }
  }, [state?.tier]);

  useEffect(() => {
    if (!statusMessage) return;
    const t = window.setTimeout(() => setStatusMessage(null), 4000);
    return () => window.clearTimeout(t);
  }, [statusMessage]);

  const localMatches = useMemo(
    () => matchLocalUsage(allModels, localDashQuery.data?.models ?? []),
    [allModels, localDashQuery.data?.models],
  );

  // Dimensions that actually have non-null data in this snapshot
  const availableDimensions = useMemo(() => {
    if (allModels.length === 0) return BENCHMARK_DIMENSIONS.slice(0, 2);
    const list = BENCHMARK_DIMENSIONS.filter((d) =>
      allModels.some((m) => getDimensionScore(m, d.key) != null),
    );
    return list.length > 0 ? list : [BENCHMARK_DIMENSIONS[0]];
  }, [allModels]);

  const activeDimDef =
    availableDimensions.find((d) => d.key === dimension) ??
    availableDimensions[0];
  const activeDimKey = activeDimDef.key;

  // All model families across the full snapshot (so every tier of a family is always known)
  const allFamilies = useMemo(
    () => groupModelFamilies(allModels, activeDimKey),
    [allModels, activeDimKey],
  );

  const familyByKey = useMemo(() => {
    const map = new Map<string, ModelFamilyGroup>();
    for (const f of allFamilies) map.set(f.familyKey, f);
    return map;
  }, [allFamilies]);

  // Family-level local match aggregation (if any tier in a family matched local usage)
  const familyLocalMatch = useMemo(() => {
    const map = new Map<string, LocalModelMatch>();
    for (const f of allFamilies) {
      let combined: LocalModelMatch | null = null;
      for (const v of f.variants) {
        const hit = localMatches.get(v.model.id);
        if (!hit) continue;
        if (!combined) {
          combined = { ...hit, matchedLocalNames: [...hit.matchedLocalNames] };
        } else {
          const prev: LocalModelMatch = combined;
          const totalReq: number = prev.requests + hit.requests;
          combined = {
            requests: totalReq,
            totalTokens: prev.totalTokens + hit.totalTokens,
            totalCostUsd: prev.totalCostUsd + hit.totalCostUsd,
            localSpeedTps:
              prev.localSpeedTps != null && hit.localSpeedTps != null
                ? (prev.localSpeedTps * prev.requests +
                    hit.localSpeedTps * hit.requests) /
                  totalReq
                : (prev.localSpeedTps ?? hit.localSpeedTps),
            localSpeedEstimated:
              prev.localSpeedEstimated && hit.localSpeedEstimated,
            localTtftS: prev.localTtftS ?? hit.localTtftS,
            matchedLocalNames: [
              ...new Set([...prev.matchedLocalNames, ...hit.matchedLocalNames]),
            ],
          };
        }
      }
      if (combined) map.set(f.familyKey, combined);
    }
    return map;
  }, [allFamilies, localMatches]);

  // Top creators by model count under current onlyScored setting
  const creatorOptions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const m of allModels) {
      if (onlyScored && getDimensionScore(m, activeDimKey) == null) continue;
      counts.set(m.creatorName, (counts.get(m.creatorName) ?? 0) + 1);
    }
    return [...counts.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 9)
      .map(([name, count]) => ({ name, count }));
  }, [allModels, onlyScored, activeDimKey]);

  // Available effort tiers with counts
  const tierOptions = useMemo(() => {
    const counts = new Map<ModelTierKey, number>();
    for (const m of allModels) {
      if (onlyScored && getDimensionScore(m, activeDimKey) == null) continue;
      if (creatorFilter !== "all" && m.creatorName !== creatorFilter) continue;
      const tier = parseModelVariant(m).tierKey;
      counts.set(tier, (counts.get(tier) ?? 0) + 1);
    }
    return MODEL_TIER_DEFS.filter((d) => (counts.get(d.key) ?? 0) > 0).map(
      (d) => ({
        ...d,
        count: counts.get(d.key) ?? 0,
      }),
    );
  }, [allModels, onlyScored, activeDimKey, creatorFilter]);

  // Global rank map for individual variants on active dimension
  const globalRankMap = useMemo(() => {
    const scored = allModels
      .map((m) => ({ id: m.id, score: getDimensionScore(m, activeDimKey) }))
      .filter((x): x is { id: string; score: number } => x.score != null)
      .sort((a, b) => b.score - a.score);
    const map = new Map<string, number>();
    scored.forEach((item, idx) => map.set(item.id, idx + 1));
    return map;
  }, [allModels, activeDimKey]);

  // Global rank map for model families (by best score on active dimension)
  const familyRankMap = useMemo(() => {
    const scored = allFamilies
      .filter((f) => f.bestScore != null)
      .sort((a, b) => b.bestScore! - a.bestScore!);
    const map = new Map<string, number>();
    scored.forEach((f, idx) => map.set(f.familyKey, idx + 1));
    return map;
  }, [allFamilies]);

  const maxDimScore = useMemo(() => {
    let max = 0;
    for (const m of allModels) {
      const s = getDimensionScore(m, activeDimKey);
      if (s != null && s > max) max = s;
    }
    return max || 100;
  }, [allModels, activeDimKey]);

  // Summary KPIs
  const summary = useMemo(() => {
    const byIntel = [...allModels]
      .filter((m) => m.intelligenceIndex != null)
      .sort((a, b) => b.intelligenceIndex! - a.intelligenceIndex!);
    const topIntel = byIntel[0] ?? null;

    const byEng = [...allModels]
      .filter((m) => m.engineeringIndex != null)
      .sort((a, b) => b.engineeringIndex! - a.engineeringIndex!);
    const topEng = byEng[0] ?? null;

    const threshold = topIntel?.intelligenceIndex
      ? topIntel.intelligenceIndex - 15
      : 0;
    const valueCandidates = byIntel
      .filter((m) => {
        const p = blendedPrice1m(m);
        return (m.intelligenceIndex ?? 0) >= threshold && p != null && p > 0;
      })
      .sort(
        (a, b) =>
          b.intelligenceIndex! / blendedPrice1m(b)! -
          a.intelligenceIndex! / blendedPrice1m(a)!,
      );
    const topValue = valueCandidates[0] ?? null;

    const localMatchedModels = allModels
      .filter((m) => localMatches.has(m.id))
      .sort(
        (a, b) =>
          (b.intelligenceIndex ?? -1) - (a.intelligenceIndex ?? -1) ||
          (localMatches.get(b.id)?.requests ?? 0) -
            (localMatches.get(a.id)?.requests ?? 0),
      );

    return {
      topIntel,
      topEng,
      topValue,
      localCount: localMatchedModels.length,
      topLocal: localMatchedModels[0] ?? null,
    };
  }, [allModels, localMatches]);

  // Filtered flat list (used by scatter plot + flat view)
  const filteredModels = useMemo(() => {
    const q = searchText.trim().toLowerCase();
    const list = allModels.filter((m) => {
      if (onlyScored && getDimensionScore(m, activeDimKey) == null)
        return false;
      if (creatorFilter !== "all" && m.creatorName !== creatorFilter)
        return false;
      if (tierFilter !== "all" && parseModelVariant(m).tierKey !== tierFilter)
        return false;
      if (onlyLocal && !localMatches.has(m.id)) return false;
      if (q) {
        const hay = `${m.name} ${m.slug} ${m.creatorName}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });

    const valOf = (m: BenchmarkModel): number | null => {
      switch (sortCol) {
        case "score":
          return getDimensionScore(m, activeDimKey);
        case "intelligence":
          return m.intelligenceIndex ?? null;
        case "engineering":
          return m.engineeringIndex ?? null;
        case "price":
          return blendedPrice1m(m);
        case "tps":
          return effectiveTps(m);
        case "ttft":
          return effectiveTtftS(m);
        case "localRequests":
          return localMatches.get(m.id)?.requests ?? null;
        case "releaseDate":
          return m.releaseDate ? Date.parse(m.releaseDate) || null : null;
      }
    };

    const mult = sortDir === "desc" ? -1 : 1;
    return [...list].sort((a, b) => {
      const va = valOf(a);
      const vb = valOf(b);
      if (va == null && vb == null) return a.name.localeCompare(b.name);
      if (va == null) return 1;
      if (vb == null) return -1;
      if (va !== vb) return (va - vb) * mult;
      return a.name.localeCompare(b.name);
    });
  }, [
    allModels,
    onlyScored,
    activeDimKey,
    creatorFilter,
    tierFilter,
    onlyLocal,
    localMatches,
    searchText,
    sortCol,
    sortDir,
  ]);

  // Filtered model families (used by grouped view — keeps ALL effort tiers visible per family!)
  const filteredFamilies = useMemo(() => {
    const q = searchText.trim().toLowerCase();
    const list = allFamilies.filter((f) => {
      if (creatorFilter !== "all" && f.creatorName !== creatorFilter)
        return false;
      if (onlyLocal && !familyLocalMatch.has(f.familyKey)) return false;
      if (tierFilter !== "all") {
        const matchingTier = f.variants.some(
          (v) =>
            v.parsed.tierKey === tierFilter && (!onlyScored || v.score != null),
        );
        if (!matchingTier) return false;
      } else if (onlyScored && f.bestScore == null) {
        return false;
      }
      if (q) {
        const hay =
          `${f.familyName} ${f.creatorName} ${f.variants.map((v) => v.model.slug).join(" ")}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });

    const activeVariantOf = (f: ModelFamilyGroup): ModelFamilyVariant => {
      if (tierFilter !== "all") {
        const hit = f.variants.find((v) => v.parsed.tierKey === tierFilter);
        if (hit) return hit;
      }
      return f.primary;
    };

    const valOf = (f: ModelFamilyGroup): number | null => {
      const rep = activeVariantOf(f);
      switch (sortCol) {
        case "score":
          return tierFilter !== "all" ? rep.score : f.bestScore;
        case "intelligence":
          return tierFilter !== "all"
            ? (rep.model.intelligenceIndex ?? null)
            : f.bestIntelligence;
        case "engineering":
          return tierFilter !== "all"
            ? (rep.model.engineeringIndex ?? null)
            : f.bestEngineering;
        case "price":
          return blendedPrice1m(rep.model);
        case "tps":
          return effectiveTps(rep.model);
        case "ttft":
          return effectiveTtftS(rep.model);
        case "localRequests":
          return familyLocalMatch.get(f.familyKey)?.requests ?? null;
        case "releaseDate":
          return f.releaseDate ? Date.parse(f.releaseDate) || null : null;
      }
    };

    const mult = sortDir === "desc" ? -1 : 1;
    return [...list].sort((a, b) => {
      const va = valOf(a);
      const vb = valOf(b);
      if (va == null && vb == null)
        return a.familyName.localeCompare(b.familyName);
      if (va == null) return 1;
      if (vb == null) return -1;
      if (va !== vb) return (va - vb) * mult;
      return a.familyName.localeCompare(b.familyName);
    });
  }, [
    allFamilies,
    creatorFilter,
    onlyLocal,
    familyLocalMatch,
    tierFilter,
    onlyScored,
    searchText,
    sortCol,
    sortDir,
  ]);

  const selectedModel = useMemo(
    () => allModels.find((m) => m.id === selectedId) ?? null,
    [allModels, selectedId],
  );

  const selectedFamily = useMemo(() => {
    if (!selectedModel) return null;
    const key = parseModelVariant(selectedModel).familyKey;
    return familyByKey.get(key) ?? null;
  }, [selectedModel, familyByKey]);

  const toggleSort = (col: SortColumn) => {
    if (sortCol === col) {
      setSortDir((d) => (d === "desc" ? "asc" : "desc"));
    } else {
      setSortCol(col);
      setSortDir(col === "price" || col === "ttft" ? "asc" : "desc");
    }
  };

  const renderSortIcon = (col: SortColumn) => {
    if (sortCol !== col) return null;
    return sortDir === "desc" ? <ArrowDown size={12} /> : <ArrowUp size={12} />;
  };

  const handleSaveSettings = (e: React.FormEvent) => {
    e.preventDefault();
    saveConfigMutation.mutate({
      apiKey: keyInput.trim() ? keyInput.trim() : undefined,
      tier: tierInput,
    });
  };

  const fetchedAtLabel = snapshot?.fetchedAt
    ? new Date(snapshot.fetchedAt).toLocaleString("zh-CN", {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      })
    : null;

  return (
    <div className="bm-page">
      {/* Top Action Bar */}
      <div className="bm-topbar">
        <div className="bm-meta-strip">
          <span className="bm-badge">
            Artificial Analysis
            {snapshot?.intelligenceIndexVersion
              ? ` · Index v${snapshot.intelligenceIndexVersion}`
              : ""}
          </span>
          {snapshot && (
            <span className="bm-meta-text">
              共 {allFamilies.length} 个模型家族 / {snapshot.models.length}{" "}
              个等级变体 · 更新于 {fetchedAtLabel}
            </span>
          )}
          {state?.keyConfigured && state.keyHint && (
            <span className="bm-meta-text bm-key-hint">
              Key: <code>{state.keyHint}</code> ({state.tier})
            </span>
          )}
          {statusMessage && (
            <span className="bm-status-ok">
              <Check size={13} />
              {statusMessage}
            </span>
          )}
        </div>

        <div className="bm-actions">
          <button
            type="button"
            className="markets-refresh-btn"
            onClick={() => setSettingsOpen((v) => !v)}
          >
            <KeyRound size={14} />
            {state?.keyConfigured ? "API Key 设置" : "配置 API Key"}
          </button>
          <button
            type="button"
            className="markets-refresh-btn bm-primary-btn"
            disabled={!state?.keyConfigured || fetchMutation.isPending}
            onClick={() => fetchMutation.mutate()}
          >
            <RefreshCw
              size={14}
              className={fetchMutation.isPending ? "bm-spin" : ""}
            />
            {fetchMutation.isPending ? "拉取中…" : "刷新榜单"}
          </button>
        </div>
      </div>

      {/* Inline API Key Configuration Drawer */}
      {(settingsOpen || (state && !state.keyConfigured && !snapshot)) && (
        <section className="panel bm-settings-panel">
          <div className="bm-settings-head">
            <div>
              <h3>Artificial Analysis API 配置</h3>
              <p>
                密钥仅保存在本机 SQLite 配置中，不会写入代码仓库或活动日志。可在{" "}
                <code>artificialanalysis.ai</code> 免费注册获取 Key。
              </p>
            </div>
            {state?.keyConfigured && (
              <button
                type="button"
                className="bm-icon-close"
                onClick={() => setSettingsOpen(false)}
                aria-label="关闭设置"
              >
                <X size={15} />
              </button>
            )}
          </div>
          <form className="bm-settings-form" onSubmit={handleSaveSettings}>
            <input
              type="password"
              className="bm-input"
              placeholder={
                state?.keyConfigured
                  ? `已配置 (${state.keyHint ?? "••••"})，留空保持不变，输入新 Key 覆盖`
                  : "输入 Artificial Analysis API Key（如 aa_...）"
              }
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
              autoComplete="off"
            />
            <select
              className="bm-select"
              value={tierInput}
              onChange={(e) =>
                setTierInput(e.target.value === "pro" ? "pro" : "free")
              }
              aria-label="接口套餐级别"
            >
              <option value="free">Free 免费层</option>
              <option value="pro">Pro 专业版（403 自动降级 Free）</option>
            </select>
            <button
              type="submit"
              className="markets-refresh-btn bm-primary-btn"
              disabled={saveConfigMutation.isPending}
            >
              保存配置
            </button>
            {state?.keyConfigured && (
              <button
                type="button"
                className="markets-refresh-btn bm-danger-btn"
                disabled={saveConfigMutation.isPending}
                onClick={() => saveConfigMutation.mutate({ clearApiKey: true })}
              >
                清除 Key
              </button>
            )}
          </form>
        </section>
      )}

      {/* Error Banner */}
      {(fetchMutation.isError || saveConfigMutation.isError) && (
        <div className="bm-error-banner" role="alert">
          {String(
            fetchMutation.error ?? saveConfigMutation.error ?? "请求失败",
          )}
        </div>
      )}

      {/* Loading skeleton when fetching first snapshot */}
      {!snapshot && fetchMutation.isPending && (
        <section className="panel bm-empty-state">
          <RefreshCw size={22} className="bm-spin" />
          <p>正在从 Artificial Analysis 全量拉取多页模型评比数据…</p>
        </section>
      )}

      {/* Main Content once snapshot exists */}
      {snapshot && (
        <>
          {/* 4 KPI Summary Cards */}
          <div className="bm-kpi-grid">
            <div
              className="bm-kpi-card"
              onClick={() =>
                summary.topIntel && setSelectedId(summary.topIntel.id)
              }
            >
              <div className="bm-kpi-label">
                <Trophy size={13} />
                综合智力榜首
              </div>
              <div className="bm-kpi-value">
                {formatScore(summary.topIntel?.intelligenceIndex)}
              </div>
              <div className="bm-kpi-sub">
                {summary.topIntel
                  ? `${summary.topIntel.name} · ${summary.topIntel.creatorName}`
                  : "暂无评分"}
              </div>
            </div>

            <div
              className="bm-kpi-card"
              onClick={() => summary.topEng && setSelectedId(summary.topEng.id)}
            >
              <div className="bm-kpi-label">
                <Cpu size={13} />
                工程能力榜首
              </div>
              <div className="bm-kpi-value">
                {formatScore(summary.topEng?.engineeringIndex)}
              </div>
              <div className="bm-kpi-sub">
                {summary.topEng
                  ? `${summary.topEng.name} · ${summary.topEng.creatorName}`
                  : "暂无评分"}
              </div>
            </div>

            <div
              className="bm-kpi-card"
              onClick={() =>
                summary.topValue && setSelectedId(summary.topValue.id)
              }
            >
              <div className="bm-kpi-label">
                <Sparkles size={13} />
                高性价比前沿之选
              </div>
              <div className="bm-kpi-value">
                {summary.topValue
                  ? `${formatScore(summary.topValue.intelligenceIndex)} / ${formatPrice1m(blendedPrice1m(summary.topValue))}`
                  : "—"}
              </div>
              <div className="bm-kpi-sub">
                {summary.topValue
                  ? `${summary.topValue.name} · ${summary.topValue.creatorName}`
                  : "暂无数据"}
              </div>
            </div>

            <div
              className="bm-kpi-card"
              onClick={() => {
                if (summary.localCount > 0) setOnlyLocal((v) => !v);
              }}
            >
              <div className="bm-kpi-label">
                <Gauge size={13} />
                本机已用模型对照
              </div>
              <div className="bm-kpi-value">{summary.localCount} 款</div>
              <div className="bm-kpi-sub">
                {summary.topLocal
                  ? `最高分：${summary.topLocal.name} (${formatScore(summary.topLocal.intelligenceIndex)})`
                  : "暂未匹配到本机调用记录"}
              </div>
            </div>
          </div>

          {/* Filter Bar */}
          <div className="bm-filter-bar">
            <div className="bm-filter-row">
              <div className="markets-pill-group bm-dim-pills">
                {availableDimensions.map((d) => (
                  <button
                    key={d.key}
                    type="button"
                    className={`markets-pill-btn ${activeDimKey === d.key ? "active" : ""}`}
                    onClick={() => {
                      setDimension(d.key);
                      setSortCol("score");
                      setSortDir("desc");
                    }}
                  >
                    {d.shortLabel}
                  </button>
                ))}
              </div>

              <div className="bm-filter-right">
                <label className="bm-toggle">
                  <input
                    type="checkbox"
                    checked={onlyScored}
                    onChange={(e) => setOnlyScored(e.target.checked)}
                  />
                  <span>仅看有评分</span>
                </label>
                <button
                  type="button"
                  className={`bm-chip-toggle ${onlyLocal ? "active" : ""}`}
                  onClick={() => setOnlyLocal((v) => !v)}
                >
                  仅看我用过的 ({summary.localCount})
                </button>
                <div className="bm-search-box">
                  <Search size={13} />
                  <input
                    type="text"
                    placeholder="搜索模型 / 厂商 / 等级…"
                    value={searchText}
                    onChange={(e) => setSearchText(e.target.value)}
                  />
                  {searchText && (
                    <button
                      type="button"
                      className="bm-search-clear"
                      onClick={() => setSearchText("")}
                      aria-label="清空搜索"
                    >
                      <X size={12} />
                    </button>
                  )}
                </div>
              </div>
            </div>

            {/* Creator Filter Row */}
            <div className="bm-creator-row">
              <span className="bm-row-label">厂商</span>
              <button
                type="button"
                className={`bm-creator-pill ${creatorFilter === "all" ? "active" : ""}`}
                onClick={() => setCreatorFilter("all")}
              >
                全部厂商
              </button>
              {creatorOptions.map((c) => (
                <button
                  key={c.name}
                  type="button"
                  className={`bm-creator-pill ${creatorFilter === c.name ? "active" : ""}`}
                  onClick={() =>
                    setCreatorFilter((cur) => (cur === c.name ? "all" : c.name))
                  }
                >
                  <i style={{ background: creatorColor(c.name) }} />
                  {c.name}
                  <span>{c.count}</span>
                </button>
              ))}
            </div>

            {/* Effort / Reasoning Tier Filter Row */}
            <div className="bm-creator-row">
              <span className="bm-row-label">等级</span>
              <button
                type="button"
                className={`bm-creator-pill ${tierFilter === "all" ? "active" : ""}`}
                onClick={() => setTierFilter("all")}
              >
                全部等级
              </button>
              {tierOptions.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  className={`bm-creator-pill bm-tier-filter-pill tier-${t.key} ${tierFilter === t.key ? "active" : ""}`}
                  onClick={() =>
                    setTierFilter((cur) => (cur === t.key ? "all" : t.key))
                  }
                >
                  {t.label}
                  <span>{t.count}</span>
                </button>
              ))}
            </div>
          </div>

          {/* Official Bar Chart & Pareto Scatter Plot */}
          <BenchmarkChartsPanel
            models={filteredModels}
            dimension={activeDimKey}
            dimensionLabel={activeDimDef.label}
            chartMode={chartMode}
            onChangeChartMode={setChartMode}
            selectedId={selectedId}
            onSelectModel={(id) =>
              setSelectedId((cur) => (cur === id ? null : id))
            }
            localMatches={localMatches}
          />

          {/* Selected Model Detail Card */}
          {selectedModel && (
            <ModelDetailPanel
              model={selectedModel}
              family={selectedFamily}
              rank={globalRankMap.get(selectedModel.id) ?? null}
              activeDimKey={activeDimKey}
              activeDimLabel={activeDimDef.shortLabel}
              localMatch={localMatches.get(selectedModel.id) ?? null}
              onSelectSibling={(id) => setSelectedId(id)}
              onClose={() => setSelectedId(null)}
            />
          )}

          {/* Leaderboard Table */}
          <section className="panel bm-table-panel">
            <div className="bm-table-head-bar">
              <div>
                <h3>
                  {activeDimDef.label}排行榜
                  <span>
                    {viewMode === "grouped"
                      ? `（${filteredFamilies.length} 个模型家族 · 展示全部等级变体）`
                      : `（${filteredModels.length} 条逐等级记录）`}
                  </span>
                </h3>
                <span className="bm-table-hint">
                  {viewMode === "grouped"
                    ? "点击任意等级胶囊（Max / Xhigh / High / Medium / Low 等）查看该等级详细指标与全等级对比"
                    : "每行对应一个独立推理等级，点击行查看全维度画像与同模型全等级对比"}
                </span>
              </div>

              <div className="markets-pill-group">
                <button
                  type="button"
                  className={`markets-pill-btn ${viewMode === "grouped" ? "active" : ""}`}
                  onClick={() => setViewMode("grouped")}
                >
                  <Layers size={12} />
                  按模型展示全等级
                </button>
                <button
                  type="button"
                  className={`markets-pill-btn ${viewMode === "flat" ? "active" : ""}`}
                  onClick={() => setViewMode("flat")}
                >
                  <List size={12} />
                  逐条平铺
                </button>
              </div>
            </div>

            <div className="bm-table-scroll">
              {viewMode === "grouped" ? (
                <table className="bm-table bm-table-grouped">
                  <thead>
                    <tr>
                      <th className="col-rank">#</th>
                      <th className="col-model">模型 / 厂商</th>
                      <th className="col-tiers">全部等级（点击切换详情）</th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("score")}
                      >
                        <span>
                          {tierFilter === "all"
                            ? `最高${activeDimDef.shortLabel}`
                            : `${MODEL_TIER_DEFS.find((d) => d.key === tierFilter)?.label} ${activeDimDef.shortLabel}`}{" "}
                          {renderSortIcon("score")}
                        </span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("price")}
                      >
                        <span>
                          输入 / 输出 ($/1M) {renderSortIcon("price")}
                        </span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("tps")}
                      >
                        <span>榜单速度 {renderSortIcon("tps")}</span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("localRequests")}
                      >
                        <span>本机实测 {renderSortIcon("localRequests")}</span>
                      </th>
                      <th
                        className="col-date sortable"
                        onClick={() => toggleSort("releaseDate")}
                      >
                        <span>发布日 {renderSortIcon("releaseDate")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {filteredFamilies.length === 0 ? (
                      <tr>
                        <td colSpan={8} className="bm-table-empty">
                          没有符合当前筛选条件的模型。
                        </td>
                      </tr>
                    ) : (
                      filteredFamilies.map((f) => {
                        const rep =
                          (tierFilter !== "all"
                            ? f.variants.find(
                                (v) => v.parsed.tierKey === tierFilter,
                              )
                            : null) ?? f.primary;
                        const rank = familyRankMap.get(f.familyKey);
                        const local = familyLocalMatch.get(f.familyKey);
                        const score =
                          tierFilter !== "all" ? rep.score : f.bestScore;
                        const tps = effectiveTps(rep.model);
                        const isFamilySelected = f.variants.some(
                          (v) => v.model.id === selectedId,
                        );
                        const pct =
                          score != null
                            ? Math.max(
                                4,
                                Math.min(100, (score / maxDimScore) * 100),
                              )
                            : 0;

                        return (
                          <tr
                            key={f.familyKey}
                            className={`${isFamilySelected ? "selected" : ""} ${local ? "has-local" : ""}`}
                            onClick={() =>
                              setSelectedId((cur) =>
                                cur === rep.model.id ? null : rep.model.id,
                              )
                            }
                          >
                            <td className="col-rank">
                              {rank != null ? (
                                <span
                                  className={`bm-rank-num ${rank <= 3 ? `top-${rank}` : ""}`}
                                >
                                  {rank}
                                </span>
                              ) : (
                                "—"
                              )}
                            </td>
                            <td className="col-model">
                              <div className="bm-model-name-row">
                                <i
                                  className="bm-creator-dot"
                                  style={{
                                    background: creatorColor(f.creatorName),
                                  }}
                                />
                                <span className="bm-model-name">
                                  {f.familyName}
                                </span>
                                {local && (
                                  <span className="bm-local-badge">
                                    本机已用
                                  </span>
                                )}
                              </div>
                              <div className="bm-model-sub">
                                {f.creatorName}
                                <span>·</span>
                                <span>{f.variants.length} 个等级</span>
                              </div>
                            </td>
                            <td
                              className="col-tiers"
                              onClick={(e) => e.stopPropagation()}
                            >
                              <div className="bm-tier-chips">
                                {f.variants.map((v) => {
                                  const isPicked = v.model.id === selectedId;
                                  const isFiltered =
                                    tierFilter !== "all" &&
                                    v.parsed.tierKey === tierFilter;
                                  return (
                                    <button
                                      key={v.model.id}
                                      type="button"
                                      className={`bm-tier-chip tier-${v.parsed.tierKey} ${v.score == null ? "unscored" : ""} ${isPicked ? "active" : ""} ${isFiltered ? "highlighted" : ""}`}
                                      title={`${v.model.name} · ${activeDimDef.shortLabel}: ${formatScore(v.score)} · 速度: ${formatTps(effectiveTps(v.model))}`}
                                      onClick={() =>
                                        setSelectedId((cur) =>
                                          cur === v.model.id
                                            ? null
                                            : v.model.id,
                                        )
                                      }
                                    >
                                      <em>{v.parsed.tierLabel}</em>
                                      <b>{formatScore(v.score)}</b>
                                    </button>
                                  );
                                })}
                              </div>
                            </td>
                            <td className="col-num col-score-bar">
                              {score != null ? (
                                <div className="bm-score-cell">
                                  <strong>{formatScore(score)}</strong>
                                  <div className="bm-bar-track">
                                    <div
                                      className="bm-bar-fill"
                                      style={{
                                        width: `${pct}%`,
                                        background: creatorColor(f.creatorName),
                                      }}
                                    />
                                  </div>
                                </div>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            <td className="col-num">
                              {rep.model.priceInput1m != null ||
                              rep.model.priceOutput1m != null ? (
                                <span className="bm-price-pair">
                                  {formatPrice1m(rep.model.priceInput1m)}
                                  <em>/</em>
                                  {formatPrice1m(rep.model.priceOutput1m)}
                                </span>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            <td className="col-num">{formatTps(tps)}</td>
                            <td className="col-num">
                              {local ? (
                                <div className="bm-local-cell">
                                  <span>
                                    {local.localSpeedTps != null
                                      ? `${local.localSpeedEstimated ? "≈" : ""}${Math.round(local.localSpeedTps)} tok/s`
                                      : `${local.requests} 次`}
                                  </span>
                                  <small>
                                    {local.requests} 次 · $
                                    {local.totalCostUsd.toFixed(2)}
                                  </small>
                                </div>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            <td className="col-date">{f.releaseDate ?? "—"}</td>
                          </tr>
                        );
                      })
                    )}
                  </tbody>
                </table>
              ) : (
                <table className="bm-table">
                  <thead>
                    <tr>
                      <th className="col-rank">#</th>
                      <th className="col-model">模型 / 等级 / 厂商</th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("score")}
                      >
                        <span>
                          {activeDimDef.shortLabel} {renderSortIcon("score")}
                        </span>
                      </th>
                      {activeDimKey !== "intelligenceIndex" && (
                        <th
                          className="col-num sortable"
                          onClick={() => toggleSort("intelligence")}
                        >
                          <span>综合智力 {renderSortIcon("intelligence")}</span>
                        </th>
                      )}
                      {activeDimKey !== "engineeringIndex" && (
                        <th
                          className="col-num sortable"
                          onClick={() => toggleSort("engineering")}
                        >
                          <span>工程 {renderSortIcon("engineering")}</span>
                        </th>
                      )}
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("price")}
                      >
                        <span>
                          输入 / 输出 ($/1M) {renderSortIcon("price")}
                        </span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("tps")}
                      >
                        <span>榜单速度 {renderSortIcon("tps")}</span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("ttft")}
                      >
                        <span>TTFT {renderSortIcon("ttft")}</span>
                      </th>
                      <th
                        className="col-num sortable"
                        onClick={() => toggleSort("localRequests")}
                      >
                        <span>本机实测 {renderSortIcon("localRequests")}</span>
                      </th>
                      <th
                        className="col-date sortable"
                        onClick={() => toggleSort("releaseDate")}
                      >
                        <span>发布日 {renderSortIcon("releaseDate")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {filteredModels.length === 0 ? (
                      <tr>
                        <td colSpan={10} className="bm-table-empty">
                          没有符合当前筛选条件的模型。
                        </td>
                      </tr>
                    ) : (
                      filteredModels.map((m) => {
                        const parsed = parseModelVariant(m);
                        const score = getDimensionScore(m, activeDimKey);
                        const rank = globalRankMap.get(m.id);
                        const local = localMatches.get(m.id);
                        const tps = effectiveTps(m);
                        const ttft = effectiveTtftS(m);
                        const isSelected = m.id === selectedId;
                        const pct =
                          score != null
                            ? Math.max(
                                4,
                                Math.min(100, (score / maxDimScore) * 100),
                              )
                            : 0;
                        return (
                          <tr
                            key={m.id}
                            className={`${isSelected ? "selected" : ""} ${local ? "has-local" : ""}`}
                            onClick={() =>
                              setSelectedId((cur) =>
                                cur === m.id ? null : m.id,
                              )
                            }
                          >
                            <td className="col-rank">
                              {rank != null ? (
                                <span
                                  className={`bm-rank-num ${rank <= 3 ? `top-${rank}` : ""}`}
                                >
                                  {rank}
                                </span>
                              ) : (
                                "—"
                              )}
                            </td>
                            <td className="col-model">
                              <div className="bm-model-name-row">
                                <i
                                  className="bm-creator-dot"
                                  style={{
                                    background: creatorColor(m.creatorName),
                                  }}
                                />
                                <span className="bm-model-name">
                                  {parsed.familyName}
                                </span>
                                <span
                                  className={`bm-tier-badge tier-${parsed.tierKey}`}
                                >
                                  {parsed.tierLabel}
                                </span>
                                {local && (
                                  <span className="bm-local-badge">
                                    本机已用
                                  </span>
                                )}
                              </div>
                              <div className="bm-model-sub">
                                {m.creatorName}
                                <span>·</span>
                                <code>{m.slug}</code>
                                {parsed.extraNote && (
                                  <>
                                    <span>·</span>
                                    <span>{parsed.extraNote}</span>
                                  </>
                                )}
                              </div>
                            </td>
                            <td className="col-num col-score-bar">
                              {score != null ? (
                                <div className="bm-score-cell">
                                  <strong>{formatScore(score)}</strong>
                                  <div className="bm-bar-track">
                                    <div
                                      className="bm-bar-fill"
                                      style={{
                                        width: `${pct}%`,
                                        background: creatorColor(m.creatorName),
                                      }}
                                    />
                                  </div>
                                </div>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            {activeDimKey !== "intelligenceIndex" && (
                              <td className="col-num">
                                {formatScore(m.intelligenceIndex)}
                              </td>
                            )}
                            {activeDimKey !== "engineeringIndex" && (
                              <td className="col-num">
                                {formatScore(m.engineeringIndex)}
                              </td>
                            )}
                            <td className="col-num">
                              {m.priceInput1m != null ||
                              m.priceOutput1m != null ? (
                                <span className="bm-price-pair">
                                  {formatPrice1m(m.priceInput1m)}
                                  <em>/</em>
                                  {formatPrice1m(m.priceOutput1m)}
                                </span>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            <td className="col-num">{formatTps(tps)}</td>
                            <td className="col-num">{formatSeconds(ttft)}</td>
                            <td className="col-num">
                              {local ? (
                                <div className="bm-local-cell">
                                  <span>
                                    {local.localSpeedTps != null
                                      ? `${local.localSpeedEstimated ? "≈" : ""}${Math.round(local.localSpeedTps)} tok/s`
                                      : `${local.requests} 次`}
                                  </span>
                                  <small>
                                    {local.requests} 次 · $
                                    {local.totalCostUsd.toFixed(2)}
                                  </small>
                                </div>
                              ) : (
                                <span className="bm-muted">—</span>
                              )}
                            </td>
                            <td className="col-date">{m.releaseDate ?? "—"}</td>
                          </tr>
                        );
                      })
                    )}
                  </tbody>
                </table>
              )}
            </div>
          </section>
        </>
      )}
    </div>
  );
}

function ModelDetailPanel({
  model,
  family,
  rank,
  activeDimKey,
  activeDimLabel,
  localMatch,
  onSelectSibling,
  onClose,
}: {
  model: BenchmarkModel;
  family: ModelFamilyGroup | null;
  rank: number | null;
  activeDimKey: BenchmarkDimensionKey;
  activeDimLabel: string;
  localMatch: LocalModelMatch | null;
  onSelectSibling: (id: string) => void;
  onClose: () => void;
}) {
  const parsed = parseModelVariant(model);
  const scoredDims = BENCHMARK_DIMENSIONS.map((d) => ({
    ...d,
    score: getDimensionScore(model, d.key),
  })).filter((d) => d.score != null);

  const benchmarkTps = effectiveTps(model);
  const speedDelta =
    localMatch?.localSpeedTps != null && benchmarkTps != null
      ? localMatch.localSpeedTps - benchmarkTps
      : null;

  const siblings = family?.variants ?? [];

  return (
    <section className="panel bm-detail-panel">
      <div className="bm-detail-head">
        <div>
          <div className="bm-detail-title-row">
            <i
              className="bm-creator-dot lg"
              style={{ background: creatorColor(model.creatorName) }}
            />
            <h3>{parsed.familyName}</h3>
            <span className={`bm-tier-badge lg tier-${parsed.tierKey}`}>
              {parsed.tierLabel}
            </span>
            {rank != null && (
              <span className="bm-rank-pill">
                {activeDimLabel} #{rank}
              </span>
            )}
            {localMatch && <span className="bm-local-badge">本机已用</span>}
          </div>
          <p className="bm-detail-sub">
            {model.creatorName} · <code>{model.slug}</code>
            {parsed.extraNote ? ` · ${parsed.extraNote}` : ""}
            {model.releaseDate ? ` · 发布于 ${model.releaseDate}` : ""}
          </p>
        </div>
        <button
          type="button"
          className="markets-drawer-close labeled"
          onClick={onClose}
          title="关闭详情"
        >
          <X size={13} />
          <span>关闭</span>
        </button>
      </div>

      {/* Sibling Effort Tiers Comparison Table */}
      {siblings.length >= 2 && (
        <div className="bm-siblings-box">
          <div className="bm-siblings-head">
            <h4>
              同模型全等级横向对比（共 {siblings.length} 个等级 · 点击行切换）
            </h4>
          </div>
          <div className="bm-table-scroll">
            <table className="bm-siblings-table">
              <thead>
                <tr>
                  <th>推理等级</th>
                  <th className="col-num">综合智力</th>
                  <th className="col-num">工程</th>
                  {activeDimKey !== "intelligenceIndex" &&
                    activeDimKey !== "engineeringIndex" && (
                      <th className="col-num">{activeDimLabel}</th>
                    )}
                  <th className="col-num">输入 / 输出 ($/1M)</th>
                  <th className="col-num">评测单任务</th>
                  <th className="col-num">速度</th>
                  <th className="col-num">TTFT</th>
                  <th className="col-num">端到端</th>
                </tr>
              </thead>
              <tbody>
                {siblings.map((s) => {
                  const isCurrent = s.model.id === model.id;
                  return (
                    <tr
                      key={s.model.id}
                      className={isCurrent ? "current" : ""}
                      onClick={() => onSelectSibling(s.model.id)}
                    >
                      <td>
                        <span
                          className={`bm-tier-badge tier-${s.parsed.tierKey}`}
                        >
                          {s.parsed.tierLabel}
                        </span>
                        {isCurrent && (
                          <span className="bm-current-tag">当前</span>
                        )}
                      </td>
                      <td className="col-num">
                        <strong>
                          {formatScore(s.model.intelligenceIndex)}
                        </strong>
                      </td>
                      <td className="col-num">
                        {formatScore(s.model.engineeringIndex)}
                      </td>
                      {activeDimKey !== "intelligenceIndex" &&
                        activeDimKey !== "engineeringIndex" && (
                          <td className="col-num">
                            {formatScore(
                              getDimensionScore(s.model, activeDimKey),
                            )}
                          </td>
                        )}
                      <td className="col-num">
                        {s.model.priceInput1m != null ||
                        s.model.priceOutput1m != null
                          ? `${formatPrice1m(s.model.priceInput1m)} / ${formatPrice1m(s.model.priceOutput1m)}`
                          : "—"}
                      </td>
                      <td className="col-num">
                        {s.model.evalCostPerTask != null
                          ? `$${s.model.evalCostPerTask.toFixed(2)}`
                          : "—"}
                      </td>
                      <td className="col-num">
                        {formatTps(effectiveTps(s.model))}
                      </td>
                      <td className="col-num">
                        {formatSeconds(effectiveTtftS(s.model))}
                      </td>
                      <td className="col-num">
                        {formatSeconds(s.model.medianE2eS)}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div className="bm-detail-grid">
        {/* Domain Scores */}
        <div className="bm-detail-card">
          <h4>各领域评比得分（{parsed.tierLabel}）</h4>
          {scoredDims.length === 0 ? (
            <p className="bm-muted">该等级在当前榜单版本下暂无分项得分。</p>
          ) : (
            <div className="bm-dim-bars">
              {scoredDims.map((d) => (
                <div key={d.key} className="bm-dim-bar-row">
                  <span className="bm-dim-bar-label">{d.shortLabel}</span>
                  <div className="bm-bar-track lg">
                    <div
                      className="bm-bar-fill"
                      style={{
                        width: `${Math.max(4, Math.min(100, d.score!))}%`,
                        background: creatorColor(model.creatorName),
                      }}
                    />
                  </div>
                  <strong>{formatScore(d.score)}</strong>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Pricing & Speed */}
        <div className="bm-detail-card">
          <h4>价格与性能指标</h4>
          <dl className="bm-kv-grid">
            <div>
              <dt>输入价格</dt>
              <dd>{formatPrice1m(model.priceInput1m)} / 1M</dd>
            </div>
            <div>
              <dt>输出价格</dt>
              <dd>{formatPrice1m(model.priceOutput1m)} / 1M</dd>
            </div>
            <div>
              <dt>缓存命中</dt>
              <dd>{formatPrice1m(model.priceCacheHit1m)} / 1M</dd>
            </div>
            <div>
              <dt>缓存写入</dt>
              <dd>{formatPrice1m(model.priceCacheWrite1m)} / 1M</dd>
            </div>
            <div>
              <dt>3:1 综合单价</dt>
              <dd>{formatPrice1m(blendedPrice1m(model))} / 1M</dd>
            </div>
            <div>
              <dt>评测单任务花费</dt>
              <dd>
                {model.evalCostPerTask != null
                  ? `$${model.evalCostPerTask.toFixed(3)}`
                  : "—"}
              </dd>
            </div>
            <div>
              <dt>平均吞吐 (v3)</dt>
              <dd>{formatTps(model.meanTps)}</dd>
            </div>
            <div>
              <dt>中位吞吐</dt>
              <dd>{formatTps(model.medianTps)}</dd>
            </div>
            <div>
              <dt>首字延迟 (TTFT)</dt>
              <dd>{formatSeconds(effectiveTtftS(model))}</dd>
            </div>
            <div>
              <dt>端到端中位耗时</dt>
              <dd>{formatSeconds(model.medianE2eS)}</dd>
            </div>
          </dl>
        </div>

        {/* Local Usage Comparison */}
        <div className="bm-detail-card">
          <h4>本机实测对照</h4>
          {localMatch ? (
            <>
              <dl className="bm-kv-grid">
                <div>
                  <dt>本机请求数</dt>
                  <dd>{localMatch.requests.toLocaleString()}</dd>
                </div>
                <div>
                  <dt>累计 Token</dt>
                  <dd>{formatTokens(localMatch.totalTokens)}</dd>
                </div>
                <div>
                  <dt>累计估算花费</dt>
                  <dd>${localMatch.totalCostUsd.toFixed(2)}</dd>
                </div>
                <div>
                  <dt>本机实测速度</dt>
                  <dd>
                    {localMatch.localSpeedTps != null
                      ? `${localMatch.localSpeedEstimated ? "≈ " : ""}${Math.round(localMatch.localSpeedTps)} tok/s`
                      : "样本不足"}
                  </dd>
                </div>
                <div>
                  <dt>对比榜单速度</dt>
                  <dd
                    className={
                      speedDelta == null
                        ? ""
                        : speedDelta >= 0
                          ? "bm-delta-pos"
                          : "bm-delta-neg"
                    }
                  >
                    {speedDelta != null
                      ? `${speedDelta >= 0 ? "+" : ""}${Math.round(speedDelta)} tok/s`
                      : "—"}
                  </dd>
                </div>
                <div>
                  <dt>本机首字中位</dt>
                  <dd>{formatSeconds(localMatch.localTtftS)}</dd>
                </div>
              </dl>
              <div className="bm-matched-ids">
                <span>匹配本机模型名：</span>
                {localMatch.matchedLocalNames.map((n) => (
                  <code key={n}>{n}</code>
                ))}
              </div>
            </>
          ) : (
            <p className="bm-muted">
              本机历史用量中暂未匹配到该模型记录。调用过对应模型后，这里会自动展示你的实测速度、延迟与花费对比。
            </p>
          )}
        </div>
      </div>
    </section>
  );
}
