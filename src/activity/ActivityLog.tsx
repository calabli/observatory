import { Fragment, useEffect, useState } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight } from "lucide-react";
import { usageCall } from "../native/usageClient";
import "./activity.css";

type Kind = "scan" | "api_in" | "sync" | "action";
type Entry = {
  id: number;
  ts: number;
  kind: Kind;
  title: string;
  status: "ok" | "error" | "skipped";
  durationMs: number | null;
  summary: Record<string, unknown>;
  hasDetail: boolean;
};
type ScanFile = {
  source: string;
  path: string;
  part: string | null;
  isNew: boolean;
  sizeBytes: number | null;
  modifiedMs: number | null;
  fromByte: number | null;
  toByte: number | null;
  bytesRead: number | null;
  fromLine: number | null;
  toLine: number;
  rows: number;
  tokens: number;
};
type ScanSource = {
  id: string;
  name: string;
  durationMs: number;
  filesScanned: number;
  filesChanged: number;
  imported: number;
  newRows: number;
  tokens: number;
  unattributedRows: number;
  errors: number;
  fileLevel: boolean;
};
type ScanDetail = {
  sources: ScanSource[];
  files: ScanFile[];
  errors: string[];
};

const KINDS: { id: Kind | ""; label: string }[] = [
  { id: "", label: "全部" },
  { id: "scan", label: "文件扫描" },
  { id: "api_in", label: "外部调用" },
  { id: "sync", label: "设备同步" },
  { id: "action", label: "操作" },
];
const KIND_LABEL: Record<Kind, string> = {
  scan: "文件扫描",
  api_in: "外部调用",
  sync: "设备同步",
  action: "操作",
};
const STATUS: Record<Entry["status"], [string, string]> = {
  ok: ["成功", "good"],
  error: ["失败", "warning"],
  skipped: ["无变化", ""],
};
const PAGE = 100;

const num = (v: unknown) =>
  typeof v === "number" ? v.toLocaleString("zh-CN") : "—";
function bytes(v: unknown) {
  if (typeof v !== "number") return "—";
  if (v >= 1048576) return `${(v / 1048576).toFixed(2)} MB`;
  if (v >= 1024) return `${(v / 1024).toFixed(1)} KB`;
  return `${v} B`;
}
function duration(v: number | null | undefined) {
  if (v === null || v === undefined) return "—";
  return v >= 1000 ? `${(v / 1000).toFixed(2)} s` : `${v} ms`;
}
const TIME = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});
const time = (ms: number | null | undefined) =>
  ms ? TIME.format(new Date(ms)) : "—";
const str = (v: unknown) =>
  v === null || v === undefined || v === "" ? "—" : String(v);

/** One-line summary for the list. */
function brief(entry: Entry) {
  const s = entry.summary;
  switch (entry.kind) {
    case "scan":
      return `${str(s.trigger)} · ${num(s.filesChanged)} 个文件有新内容 · 新增 ${num(s.newRows)} 条 · ${num(s.tokens)} Token${s.errors ? ` · ${num(s.errors)} 个错误` : ""}`;
    case "api_in":
      return `${str(s.method)} ${str(s.path)} · ${str(s.caller)} · HTTP ${str(s.httpStatus)}${typeof s.responseBytes === "number" ? ` · ${bytes(s.responseBytes)}` : ""}`;
    case "sync":
      if (s.error) return `失败：${str(s.error)}`;
      if (s.skipped) return "对端内容未变化，跳过下载";
      return `下载 ${bytes(s.downloadedBytes)} · ${num(s.events)} 条明细 · 写入 ${num(s.writtenRows)} 行`;
    case "action":
      return s.error ? `失败：${str(s.error)}` : str(s.op);
  }
}

/** Routine, no-change entries: the peer's per-round fingerprint query and the syncs that match that fingerprint. */
const routine = (entry: Entry) =>
  (entry.kind === "sync" && entry.status === "skipped") ||
  (entry.kind === "api_in" &&
    entry.summary.path === "/api/lan/usage/fingerprint" &&
    entry.status === "ok");

function KeyValues({ rows }: { rows: [string, React.ReactNode][] }) {
  return (
    <dl className="activity-kv">
      {rows.map(([k, v]) => (
        <Fragment key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

function ScanDetailView({ id }: { id: number }) {
  const detail = useQuery({
    queryKey: ["activity", "detail", id],
    queryFn: () => usageCall<ScanDetail | null>("activity_detail", { id }),
    staleTime: Infinity,
  });
  if (detail.isLoading) return <p className="hint">读取明细…</p>;
  if (detail.error)
    return <p className="usage-error">明细读取失败：{String(detail.error)}</p>;
  const d = detail.data;
  if (!d) return <p className="hint">没有明细。</p>;
  const names = Object.fromEntries(d.sources.map((s) => [s.id, s.name]));
  const noFileLevel = d.sources.filter((s) => !s.fileLevel);
  return (
    <div className="activity-detail-body">
      <h4>各来源</h4>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>来源</th>
              <th className="numeric">扫描文件</th>
              <th className="numeric">有新内容</th>
              <th className="numeric">新增条数</th>
              <th className="numeric">Token</th>
              <th className="numeric">耗时</th>
              <th className="numeric">错误</th>
            </tr>
          </thead>
          <tbody>
            {d.sources.map((s) => (
              <tr key={s.id}>
                <td>{s.name}</td>
                <td className="numeric">{num(s.filesScanned)}</td>
                <td className="numeric">
                  {s.fileLevel ? num(s.filesChanged) : "—"}
                </td>
                <td className="numeric">{num(s.newRows)}</td>
                <td className="numeric">{num(s.tokens)}</td>
                <td className="numeric">{duration(s.durationMs)}</td>
                <td className="numeric">{s.errors ? num(s.errors) : ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {noFileLevel.length > 0 && (
        <p className="hint">
          {noFileLevel.map((s) => s.name).join("、")}{" "}
          不记录逐文件的读取进度，只显示来源汇总。
        </p>
      )}
      <h4>读取的文件（{d.files.length}）</h4>
      {d.files.length === 0 ? (
        <p className="hint">这次扫描没有文件出现新内容。</p>
      ) : (
        <div className="table-wrap">
          <table className="activity-files">
            <thead>
              <tr>
                <th>文件</th>
                <th>来源</th>
                <th className="numeric">文件大小</th>
                <th>修改时间</th>
                <th className="numeric">本次读取</th>
                <th className="numeric">读取位置</th>
                <th className="numeric">新增条数</th>
                <th className="numeric">Token</th>
              </tr>
            </thead>
            <tbody>
              {d.files.map((f) => (
                <tr key={`${f.path}|${f.part ?? ""}`}>
                  <td className="activity-path">
                    <span>{f.path}</span>
                    {(f.part || f.isNew) && (
                      <span className="cell-sub">
                        {f.part ? `会话 ${f.part}` : ""}
                        {f.part && f.isNew ? " · " : ""}
                        {f.isNew ? "首次读取" : ""}
                      </span>
                    )}
                  </td>
                  <td>{names[f.source] ?? f.source}</td>
                  <td className="numeric">{bytes(f.sizeBytes)}</td>
                  <td className="muted">{time(f.modifiedMs)}</td>
                  <td className="numeric">
                    {f.bytesRead !== null
                      ? bytes(f.bytesRead)
                      : f.fromLine !== null && f.toLine > f.fromLine
                        ? `${num(f.toLine - f.fromLine)} 行`
                        : "—"}
                  </td>
                  <td className="numeric muted">
                    {f.toByte !== null
                      ? `${num(f.fromByte ?? 0)} → ${num(f.toByte)}`
                      : f.toLine
                        ? `第 ${num(f.toLine)} 行`
                        : "—"}
                  </td>
                  <td className="numeric">{f.rows ? num(f.rows) : ""}</td>
                  <td className="numeric">{f.tokens ? num(f.tokens) : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {d.sources.some((s) => s.unattributedRows > 0) && (
        <p className="hint">
          部分新增记录无法对应到具体文件（如来源本身是数据库），已计入上方来源汇总。
        </p>
      )}
      {d.errors.length > 0 && (
        <>
          <h4>错误（{d.errors.length}）</h4>
          <ul className="activity-errors">
            {d.errors.map((e, i) => (
              <li key={i}>{e}</li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

function EntryDetail({ entry }: { entry: Entry }) {
  const s = entry.summary;
  switch (entry.kind) {
    case "scan":
      return <ScanDetailView id={entry.id} />;
    case "api_in":
      return (
        <KeyValues
          rows={[
            ["请求", <code key="r">{`${str(s.method)} ${str(s.path)}`}</code>],
            ["查询参数", <code key="q">{str(s.query)}</code>],
            ["调用方", str(s.caller)],
            ["来源地址", str(s.remote)],
            ["状态码", str(s.httpStatus)],
            ["响应大小", bytes(s.responseBytes)],
            ["耗时", duration(entry.durationMs)],
          ]}
        />
      );
    case "sync":
      return (
        <KeyValues
          rows={[
            ["设备", `${str(s.deviceName)}（${str(s.deviceId)}）`],
            ["地址", s.host ? `${str(s.host)}:${str(s.port)}` : "—"],
            [
              "结果",
              s.error
                ? `失败：${str(s.error)}`
                : s.skipped
                  ? "对端内容指纹与上次一致，跳过下载"
                  : "已下载并写入",
            ],
            ["下载大小", bytes(s.downloadedBytes)],
            ["明细 / 日汇总", `${num(s.events)} / ${num(s.daily)}`],
            ["写入行数", num(s.writtenRows)],
            ["耗时", duration(entry.durationMs)],
          ]}
        />
      );
    case "action":
      return (
        <KeyValues
          rows={[
            ["命令", <code key="op">{str(s.op)}</code>],
            [
              "参数",
              <pre key="a">{JSON.stringify(s.args ?? {}, null, 2)}</pre>,
            ],
            [
              s.error ? "错误" : "结果",
              s.error ? (
                str(s.error)
              ) : (
                <pre key="res">{JSON.stringify(s.result ?? {}, null, 2)}</pre>
              ),
            ],
            ["耗时", duration(entry.durationMs)],
          ]}
        />
      );
  }
}

export function ActivityLog() {
  const [kind, setKind] = useState<Kind | "">("");
  const [hideRoutine, setHideRoutine] = useState(true);
  const [open, setOpen] = useState<Set<number>>(() => new Set());
  const [visible, setVisible] = useState(
    () => document.visibilityState === "visible",
  );
  useEffect(() => {
    const onChange = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  const list = useInfiniteQuery({
    queryKey: ["activity", "list", kind],
    queryFn: ({ pageParam }) =>
      usageCall<Entry[]>("activity_list", {
        kind: kind || undefined,
        beforeId: pageParam,
        limit: PAGE,
      }),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) =>
      last.length === PAGE ? last[last.length - 1].id : undefined,
    // Poll for new records every 5 seconds while the page is visible; stop
    // when it is hidden.
    refetchInterval: visible ? 5000 : false,
  });
  const all = list.data?.pages.flat() ?? [];
  const rows = hideRoutine ? all.filter((e) => !routine(e)) : all;
  const hidden = all.length - rows.length;
  const toggle = (id: number) =>
    setOpen((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <section className="panel activity-log">
      <div className="panel-head">
        <div>
          <h2>活动记录</h2>
          <p>
            本机扫描读取的文件、外部接口调用、设备同步与关键操作；保留 7
            天，只存本机。点一行查看细节。
          </p>
        </div>
        <div className="panel-actions">
          <label className="activity-routine">
            <input
              type="checkbox"
              checked={hideRoutine}
              onChange={(e) => setHideRoutine(e.target.checked)}
            />
            隐藏无变化的例行同步
          </label>
          <div className="segmented" role="group" aria-label="按类型筛选">
            {KINDS.map((k) => (
              <button
                key={k.id || "all"}
                aria-pressed={kind === k.id}
                onClick={() => {
                  setKind(k.id);
                  setOpen(new Set());
                }}
              >
                {k.label}
              </button>
            ))}
          </div>
        </div>
      </div>
      {list.error && (
        <p className="usage-error" role="alert">
          活动记录读取失败：{String(list.error)}
        </p>
      )}
      <div className="table-wrap">
        <table className="activity-table">
          <thead>
            <tr>
              <th aria-label="展开" />
              <th>时间</th>
              <th>类型</th>
              <th>事件</th>
              <th>状态</th>
              <th className="numeric">耗时</th>
              <th>摘要</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((entry) => {
              const expanded = open.has(entry.id);
              const [statusText, tone] = STATUS[entry.status] ?? [
                entry.status,
                "",
              ];
              return (
                <Fragment key={entry.id}>
                  <tr
                    className={"activity-row" + (expanded ? " expanded" : "")}
                    onClick={() => toggle(entry.id)}
                  >
                    <td>
                      <button
                        className="icon-button activity-toggle"
                        aria-expanded={expanded}
                        aria-label={expanded ? "收起细节" : "展开细节"}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggle(entry.id);
                        }}
                      >
                        {expanded ? (
                          <ChevronDown size={13} />
                        ) : (
                          <ChevronRight size={13} />
                        )}
                      </button>
                    </td>
                    <td className="muted">{time(entry.ts)}</td>
                    <td>
                      <span className="badge">{KIND_LABEL[entry.kind]}</span>
                    </td>
                    <td>{entry.title}</td>
                    <td>
                      <span className={`badge ${tone}`}>{statusText}</span>
                    </td>
                    <td className="numeric">{duration(entry.durationMs)}</td>
                    <td className="activity-brief">{brief(entry)}</td>
                  </tr>
                  {expanded && (
                    <tr className="activity-detail">
                      <td colSpan={7}>
                        <EntryDetail entry={entry} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
      {!list.isLoading && rows.length === 0 && (
        <p className="hint activity-empty">
          {all.length === 0
            ? "还没有活动记录。扫描、同步、接口调用和设置修改都会出现在这里。"
            : "当前筛选下没有记录。"}
        </p>
      )}
      <div className="activity-foot">
        <span className="hint">
          已显示 {rows.length} 条
          {hidden > 0 ? `，隐藏 ${hidden} 条例行同步` : ""}
        </span>
        {list.hasNextPage && (
          <button
            className="button"
            onClick={() => void list.fetchNextPage()}
            disabled={list.isFetchingNextPage}
          >
            {list.isFetchingNextPage ? "读取中…" : "加载更早的记录"}
          </button>
        )}
      </div>
    </section>
  );
}
