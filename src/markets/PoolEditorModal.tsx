import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Check,
  FolderPlus,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import {
  MARKET_META,
  type ResolvedPool,
  type SearchHit,
  type WatchItem,
  MAX_POOL_SIZE,
  POOL_LIMIT_HINT,
} from "./marketsShared";

interface PoolEditorModalProps {
  mode: "edit" | "create";
  /** Null when editing the user's personal watchlist (`poolId === "watchlist"`). */
  pool: ResolvedPool | null;
  watchlistItems: WatchItem[];
  initialSector?: string;
  onClose: () => void;
  onAddCompany: (item: WatchItem) => void;
  onRemoveCompany: (symbol: string) => void;
  onChangeSector: (symbol: string, sector: string) => void;
  onRestoreRemoved: (item: WatchItem) => void;
  onResetPreset: () => void;
  onCreatePool: (name: string, description: string) => void;
  onRenamePool: (name: string, description: string) => void;
  onDeletePool: () => void;
}

const MAX_UNFILTERED_ROWS = 160;

export function PoolEditorModal({
  mode,
  pool,
  watchlistItems,
  initialSector,
  onClose,
  onAddCompany,
  onRemoveCompany,
  onChangeSector,
  onRestoreRemoved,
  onResetPreset,
  onCreatePool,
  onRenamePool,
  onDeletePool,
}: PoolEditorModalProps) {
  const isWatchlist = mode === "edit" && pool === null;
  const items = useMemo(
    () => (isWatchlist ? watchlistItems : (pool?.items ?? [])),
    [isWatchlist, watchlistItems, pool?.items],
  );

  const sectors = useMemo(() => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const it of items) {
      const s = it.sector?.trim();
      if (s && !seen.has(s)) {
        seen.add(s);
        out.push(s);
      }
    }
    return out;
  }, [items]);

  const [newPoolName, setNewPoolName] = useState("");
  const [newPoolDesc, setNewPoolDesc] = useState("");
  const [editName, setEditName] = useState(pool?.name ?? "");
  const [editDesc, setEditDesc] = useState(pool?.description ?? "");

  const [targetSector, setTargetSector] = useState(
    () =>
      (initialSector && initialSector !== "ALL" ? initialSector : "") ||
      sectors[0] ||
      "自定义",
  );
  const [searchText, setSearchText] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [filterSector, setFilterSector] = useState("ALL");
  const [showAllRows, setShowAllRows] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const searchWrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setEditName(pool?.name ?? "");
    setEditDesc(pool?.description ?? "");
    setConfirmDelete(false);
    setFilterText("");
    setFilterSector("ALL");
    setShowAllRows(false);
  }, [pool?.id, pool?.name, pool?.description]);

  useEffect(() => {
    if (!isWatchlist && sectors.length > 0 && !targetSector.trim()) {
      setTargetSector(sectors[0]);
    }
  }, [isWatchlist, sectors, targetSector]);

  useEffect(() => {
    const t = setTimeout(() => setDebouncedSearch(searchText.trim()), 250);
    return () => clearTimeout(t);
  }, [searchText]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (
        searchWrapRef.current &&
        !searchWrapRef.current.contains(e.target as Node)
      ) {
        setSearchOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, []);

  const searchQuery = useQuery({
    queryKey: ["markets", "search", debouncedSearch],
    queryFn: () =>
      usageCall<SearchHit[]>("markets_search", { query: debouncedSearch }),
    enabled: mode === "edit" && debouncedSearch.length >= 1,
    staleTime: 30_000,
    retry: false,
  });

  const existingSymbols = useMemo(
    () => new Set(items.map((i) => i.symbol.trim().toUpperCase())),
    [items],
  );

  const filteredItems = useMemo(() => {
    let list = items;
    if (!isWatchlist && filterSector !== "ALL") {
      list = list.filter((it) => (it.sector || "其他") === filterSector);
    }
    const q = filterText.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (it) =>
        it.symbol.toLowerCase().includes(q) ||
        it.name.toLowerCase().includes(q) ||
        (it.sector?.toLowerCase().includes(q) ?? false),
    );
  }, [items, isWatchlist, filterSector, filterText]);

  const visibleItems =
    showAllRows || filterText.trim() || filterSector !== "ALL"
      ? filteredItems
      : filteredItems.slice(0, MAX_UNFILTERED_ROWS);
  const hiddenCount = filteredItems.length - visibleItems.length;

  const atLimit = items.length >= MAX_POOL_SIZE;

  const handleAddHit = (hit: SearchHit) => {
    if (atLimit) return;
    const sector = isWatchlist
      ? undefined
      : targetSector.trim() || sectors[0] || "自定义";
    onAddCompany({
      symbol: hit.symbol,
      name: hit.name,
      market: hit.market,
      ...(sector ? { sector } : {}),
    });
    setSearchText("");
    setSearchOpen(false);
  };

  const handleCreateSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = newPoolName.trim();
    if (!trimmed) return;
    onCreatePool(trimmed, newPoolDesc.trim());
  };

  return (
    <div
      className="markets-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={mode === "create" ? "新建股票板块" : "编辑股票板块"}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="markets-modal">
        {mode === "create" ? (
          <>
            <div className="markets-modal-head">
              <div className="markets-modal-title">
                <FolderPlus size={16} />
                <h3>新建自定义板块</h3>
              </div>
              <button
                type="button"
                className="markets-drawer-close"
                aria-label="关闭"
                onClick={onClose}
              >
                <X size={14} />
              </button>
            </div>
            <form className="markets-create-form" onSubmit={handleCreateSubmit}>
              <label className="markets-field">
                <span>板块名称</span>
                <input
                  type="text"
                  value={newPoolName}
                  onChange={(e) => setNewPoolName(e.target.value)}
                  placeholder="例如：人形机器人、量子计算、核电算力…"
                  maxLength={32}
                  autoFocus
                  required
                />
              </label>
              <label className="markets-field">
                <span>简介（可选）</span>
                <input
                  type="text"
                  value={newPoolDesc}
                  onChange={(e) => setNewPoolDesc(e.target.value)}
                  placeholder="简单记录该板块关注的主题或选股逻辑"
                  maxLength={100}
                />
              </label>
              <div className="markets-modal-actions">
                <button
                  type="button"
                  className="markets-btn-secondary"
                  onClick={onClose}
                >
                  取消
                </button>
                <button
                  type="submit"
                  className="markets-btn-primary"
                  disabled={!newPoolName.trim()}
                >
                  创建并添加公司
                </button>
              </div>
            </form>
          </>
        ) : (
          <>
            <div className="markets-modal-head">
              <div className="markets-modal-title">
                <h3>编辑板块：{isWatchlist ? "我的自选" : pool?.name}</h3>
                <span className="markets-badge neutral">
                  共 {items.length} 家
                </span>
                {pool?.kind === "preset" && pool.edited && (
                  <span className="markets-badge edited">已自定义</span>
                )}
              </div>
              <div className="markets-modal-head-actions">
                {pool?.kind === "preset" && pool.edited && (
                  <button
                    type="button"
                    className="markets-btn-secondary"
                    onClick={onResetPreset}
                    title="撤销对此内置板块的所有增删改动"
                  >
                    <RotateCcw size={12} />
                    <span>恢复默认</span>
                  </button>
                )}
                {pool?.kind === "custom" &&
                  (confirmDelete ? (
                    <button
                      type="button"
                      className="markets-btn-danger"
                      onClick={onDeletePool}
                    >
                      确认删除板块
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="markets-btn-secondary danger-text"
                      onClick={() => setConfirmDelete(true)}
                    >
                      <Trash2 size={12} />
                      <span>删除板块</span>
                    </button>
                  ))}
                <button
                  type="button"
                  className="markets-drawer-close"
                  aria-label="关闭"
                  onClick={onClose}
                >
                  <X size={14} />
                </button>
              </div>
            </div>

            {pool?.kind === "custom" && (
              <div className="markets-custom-meta-row">
                <input
                  type="text"
                  value={editName}
                  onChange={(e) => setEditName(e.target.value)}
                  onBlur={() => {
                    const trimmed = editName.trim();
                    if (
                      trimmed &&
                      (trimmed !== pool.name ||
                        editDesc.trim() !== pool.description)
                    ) {
                      onRenamePool(trimmed, editDesc.trim());
                    }
                  }}
                  placeholder="板块名称"
                  aria-label="板块名称"
                  maxLength={32}
                />
                <input
                  type="text"
                  value={editDesc}
                  onChange={(e) => setEditDesc(e.target.value)}
                  onBlur={() => {
                    const trimmed = editName.trim() || pool.name;
                    if (
                      trimmed !== pool.name ||
                      editDesc.trim() !== pool.description
                    ) {
                      onRenamePool(trimmed, editDesc.trim());
                    }
                  }}
                  placeholder="板块简介（可选）"
                  aria-label="板块简介"
                  maxLength={100}
                />
              </div>
            )}

            {/* Add company row */}
            <div className="markets-editor-add-bar">
              <div className="markets-editor-search-wrap" ref={searchWrapRef}>
                <label className="markets-editor-search-box">
                  <Search size={13} />
                  <input
                    type="search"
                    value={searchText}
                    onFocus={() => setSearchOpen(true)}
                    onChange={(e) => {
                      setSearchText(e.target.value);
                      setSearchOpen(true);
                    }}
                    placeholder="搜索代码或名称添加到本板块，如 NVDA、2330、茅台、腾讯…"
                    aria-label="搜索公司添加到板块"
                  />
                </label>
                {searchOpen && debouncedSearch.length >= 1 && (
                  <div className="markets-search-dropdown left">
                    {atLimit ? (
                      <div className="markets-search-empty">
                        {POOL_LIMIT_HINT}
                      </div>
                    ) : searchQuery.isFetching ? (
                      <div className="markets-search-empty">搜索中…</div>
                    ) : (searchQuery.data ?? []).length === 0 ? (
                      <div className="markets-search-empty">
                        未找到匹配标的（可尝试完整代码如 2330.TW / 600519.SS）
                      </div>
                    ) : (
                      (searchQuery.data ?? []).map((hit) => {
                        const exists = existingSymbols.has(
                          hit.symbol.trim().toUpperCase(),
                        );
                        return (
                          <button
                            key={hit.symbol}
                            type="button"
                            className="markets-search-item"
                            onClick={() =>
                              exists
                                ? onRemoveCompany(hit.symbol)
                                : handleAddHit(hit)
                            }
                          >
                            <span className="markets-mkt-tag">
                              {hit.market}
                            </span>
                            <span className="markets-search-info">
                              <strong>{hit.name}</strong>
                              <small>
                                {hit.symbol}
                                {hit.exchange ? ` · ${hit.exchange}` : ""}
                              </small>
                            </span>
                            <span className="markets-add-tag">
                              {exists ? (
                                <>
                                  <Check size={12} /> 已在板块 · 点此移除
                                </>
                              ) : (
                                <>
                                  <Plus size={12} /> 加入
                                  {!isWatchlist && targetSector.trim()
                                    ? `（${targetSector.trim()}）`
                                    : ""}
                                </>
                              )}
                            </span>
                          </button>
                        );
                      })
                    )}
                  </div>
                )}
              </div>

              {!isWatchlist && (
                <label className="markets-sector-target">
                  <span>放入子行业</span>
                  <input
                    type="text"
                    list="markets-editor-sectors"
                    value={targetSector}
                    onChange={(e) => setTargetSector(e.target.value)}
                    placeholder="输入或选择子行业"
                    maxLength={24}
                  />
                  <datalist id="markets-editor-sectors">
                    {sectors.map((sec) => (
                      <option key={sec} value={sec} />
                    ))}
                  </datalist>
                </label>
              )}
            </div>

            {/* Filter existing companies in this pool */}
            <div className="markets-editor-filter-row">
              <label className="markets-pool-filter">
                <Search size={12} />
                <input
                  type="search"
                  value={filterText}
                  onChange={(e) => setFilterText(e.target.value)}
                  placeholder={`筛选当前板块内 ${items.length} 家公司…`}
                  aria-label="筛选板块内公司"
                />
              </label>
              {!isWatchlist && sectors.length > 0 && (
                <div className="markets-pill-group markets-editor-sector-filter">
                  <button
                    type="button"
                    className={`markets-pill-btn${filterSector === "ALL" ? " active" : ""}`}
                    onClick={() => setFilterSector("ALL")}
                  >
                    全部 ({items.length})
                  </button>
                  {sectors.map((sec) => (
                    <button
                      key={sec}
                      type="button"
                      className={`markets-pill-btn${filterSector === sec ? " active" : ""}`}
                      onClick={() => setFilterSector(sec)}
                    >
                      {sec}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Company table */}
            <div className="markets-editor-list">
              {visibleItems.length === 0 ? (
                <div className="markets-empty-cell">
                  {items.length === 0
                    ? "该板块还没有公司，在上方搜索代码或名称添加"
                    : "没有匹配的公司"}
                </div>
              ) : (
                <table className="markets-editor-table">
                  <thead>
                    <tr>
                      <th>公司</th>
                      {!isWatchlist && <th>所属子行业</th>}
                      <th className="num">操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visibleItems.map((it) => (
                      <tr key={it.symbol}>
                        <td>
                          <div className="markets-sym-cell">
                            <span className="markets-mkt-tag">
                              {MARKET_META[it.market]?.short ?? it.market}
                            </span>
                            <div className="markets-sym-text">
                              <strong>{it.name}</strong>
                              <span>{it.symbol}</span>
                            </div>
                          </div>
                        </td>
                        {!isWatchlist && (
                          <td>
                            <input
                              type="text"
                              className="markets-inline-sector"
                              list="markets-editor-sectors"
                              defaultValue={it.sector ?? ""}
                              key={`${it.symbol}|${it.sector ?? ""}`}
                              placeholder="子行业"
                              onBlur={(e) => {
                                const nextSec = e.target.value.trim();
                                if (nextSec && nextSec !== (it.sector ?? "")) {
                                  onChangeSector(it.symbol, nextSec);
                                }
                              }}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                  (e.target as HTMLInputElement).blur();
                                }
                              }}
                            />
                          </td>
                        )}
                        <td className="num">
                          <button
                            type="button"
                            className="markets-del-btn visible"
                            title={`从板块移除 ${it.name}`}
                            aria-label={`移除 ${it.symbol}`}
                            onClick={() => onRemoveCompany(it.symbol)}
                          >
                            <Trash2 size={13} />
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              {hiddenCount > 0 && (
                <div className="markets-editor-more">
                  <button
                    type="button"
                    className="markets-btn-secondary"
                    onClick={() => setShowAllRows(true)}
                  >
                    展开剩余 {hiddenCount} 家公司
                  </button>
                </div>
              )}
            </div>

            {/* Removed built-in companies restore strip */}
            {pool && pool.removedItems.length > 0 && (
              <div className="markets-removed-bar">
                <span className="markets-removed-label">
                  已从默认列表移除 ({pool.removedItems.length})：
                </span>
                <div className="markets-removed-chips">
                  {pool.removedItems.map((rem) => (
                    <button
                      key={rem.symbol}
                      type="button"
                      className="markets-removed-chip"
                      onClick={() => onRestoreRemoved(rem)}
                      title={`恢复 ${rem.name} (${rem.symbol}) 到板块`}
                    >
                      <span>
                        {rem.name} ({rem.symbol})
                      </span>
                      <Plus size={11} />
                    </button>
                  ))}
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
