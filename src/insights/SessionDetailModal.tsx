import {
  fmtClockMs,
  fmtDurationMs,
  fmtTokens,
  fmtUsd,
  type SessionTraceDetail,
  type TaskTrace,
} from "./insightsShared";
import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  Check,
  Copy,
  GitBranch,
  ListTree,
  Table2,
  Wrench,
  X,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { usageCall } from "../native/usageClient";
import { AppGlyph } from "../usage/AppGlyph";
import { formatRequestSpeed } from "../usage/requestSpeed";

export function TaskWaterfall({
  task,
  onHover,
  onLeave,
}: {
  task: TaskTrace;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [selectedSpanId, setSelectedSpanId] = useState<string | null>(null);

  useEffect(() => {
    setSelectedSpanId(null);
  }, [task.index]);

  const wallMs = Math.max(1, task.durationMs);
  const llmPct = Math.min(100, (task.llmMs / wallMs) * 100);
  const toolPct = Math.min(100 - llmPct, (task.toolMs / wallMs) * 100);
  const otherPct = Math.max(0, 100 - llmPct - toolPct);

  const selectedSpan =
    task.spans.find((s) => s.id === selectedSpanId) ?? task.spans[0] ?? null;

  const ticks = [0, 0.25, 0.5, 0.75, 1];

  return (
    <div className="insights-waterfall">
      <div className="insights-wf-summary">
        <div className="insights-wf-prompt-row">
          <div className="insights-wf-prompt">
            <span className="insights-badge neutral">任务 #{task.index}</span>
            <strong>{task.prompt ?? `任务段 #${task.index}`}</strong>
          </div>
          <div className="insights-wf-kpis">
            <span>总耗时 {fmtDurationMs(task.durationMs)}</span>
            <span>·</span>
            <span>{task.llmCalls} 次模型</span>
            {task.toolCalls > 0 && (
              <>
                <span>·</span>
                <span>{task.toolCalls} 次工具</span>
              </>
            )}
            {task.errorCount > 0 && (
              <>
                <span>·</span>
                <span className="insights-wf-err-text">
                  {task.errorCount} 异常
                </span>
              </>
            )}
            <span>·</span>
            <strong>{fmtUsd(task.totalCostUsd)}</strong>
          </div>
        </div>

        <div className="insights-wf-attr-row">
          <div className="insights-bar-track" style={{ height: 7, flex: 1 }}>
            <div
              style={{
                width: `${llmPct.toFixed(1)}%`,
                background: "var(--accent-violet)",
              }}
              title={`模型推理 ${fmtDurationMs(task.llmMs)}`}
            />
            <div
              style={{
                width: `${toolPct.toFixed(1)}%`,
                background: "var(--accent-cyan)",
              }}
              title={`工具执行 ${fmtDurationMs(task.toolMs)}`}
            />
            <div
              style={{
                width: `${otherPct.toFixed(1)}%`,
                background: "var(--border-subtle)",
              }}
              title="等待 / 间隔"
            />
          </div>
          <div className="insights-wf-legend">
            <span>
              <span
                className="insights-dot"
                style={{ background: "var(--accent-violet)" }}
              />
              模型 {fmtDurationMs(task.llmMs)} ({llmPct.toFixed(0)}%)
            </span>
            {task.toolMs > 0 && (
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-cyan)" }}
                />
                工具 {fmtDurationMs(task.toolMs)} ({toolPct.toFixed(0)}%)
              </span>
            )}
          </div>
        </div>

        {task.topTools.length > 0 && (
          <div className="insights-wf-tools">
            <Wrench size={12} />
            {task.topTools.map((tool) => (
              <span
                key={tool.name}
                className={`insights-wf-tool-chip${tool.errorCount > 0 ? " err" : ""}`}
              >
                <code>{tool.name}</code>
                <span>×{tool.count}</span>
                <span className="muted">{fmtDurationMs(tool.totalMs)}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="insights-wf-board">
        <div className="insights-wf-axis-row">
          <div className="insights-wf-label-col">调用节点（模型 / 工具）</div>
          <div className="insights-wf-track-col insights-wf-axis">
            {ticks.map((r) => (
              <span
                key={r}
                className="insights-wf-tick"
                style={{ left: `${r * 100}%` }}
              >
                {fmtDurationMs(wallMs * r)}
              </span>
            ))}
          </div>
        </div>

        <div className="insights-wf-rows">
          {task.spans.map((span) => {
            const leftPct = Math.min(99, (span.offsetMs / wallMs) * 100);
            const widthPct = Math.max(
              0.8,
              Math.min(100 - leftPct, (span.durationMs / wallMs) * 100),
            );
            const ttftRatio =
              span.kind === "llm" &&
              span.ttftMs != null &&
              span.ttftMs > 0 &&
              span.durationMs > 0
                ? Math.min(0.95, span.ttftMs / span.durationMs)
                : 0;
            const kindTag =
              span.status === "error"
                ? "ERR"
                : span.kind === "llm"
                  ? "LLM"
                  : span.kind === "subagent"
                    ? "AGENT"
                    : span.kind === "gap"
                      ? "GAP"
                      : "TOOL";
            const barClass =
              span.status === "error"
                ? "err"
                : span.kind === "llm"
                  ? "llm"
                  : span.kind === "subagent"
                    ? "subagent"
                    : span.kind === "gap"
                      ? "gap"
                      : "tool";
            const isSelected = selectedSpan?.id === span.id;
            const labelInsideRight = leftPct + widthPct > 82;

            const tipLines = [
              ...(span.detail ? [`详情：${span.detail}`] : []),
              `耗时：${fmtDurationMs(span.durationMs)}（偏移 +${fmtDurationMs(span.offsetMs)}）`,
              ...(span.ttftMs != null && span.ttftMs > 0
                ? [`首字 TTFT：${fmtDurationMs(span.ttftMs)}`]
                : []),
              ...(span.kind === "llm"
                ? [
                    `Token：新输入 ${fmtTokens(span.inputTokens)} · 缓存读 ${fmtTokens(span.cacheReadTokens)} · 输出 ${fmtTokens(span.outputTokens)}`,
                    `费用：${fmtUsd(span.costUsd)}`,
                  ]
                : []),
              ...(span.errorMessage ? [`异常：${span.errorMessage}`] : []),
            ];

            return (
              <button
                key={span.id}
                type="button"
                className={`insights-wf-row${isSelected ? " active" : ""}${span.depth > 0 ? " child" : ""}`}
                onClick={() => setSelectedSpanId(span.id)}
                onMouseMove={(e) =>
                  onHover(e, `[${kindTag}] ${span.label}`, tipLines)
                }
                onMouseLeave={onLeave}
              >
                <div className="insights-wf-label-col">
                  {span.depth > 0 && (
                    <span className="insights-wf-tree">└</span>
                  )}
                  <span className={`insights-wf-tag ${barClass}`}>
                    {kindTag}
                  </span>
                  <span className="insights-wf-name" title={span.label}>
                    {span.label}
                  </span>
                  {span.detail && (
                    <span className="insights-wf-detail" title={span.detail}>
                      {span.detail}
                    </span>
                  )}
                </div>
                <div className="insights-wf-track-col">
                  <div
                    className={`insights-wf-bar ${barClass}`}
                    style={{
                      left: `${leftPct.toFixed(2)}%`,
                      width: `${widthPct.toFixed(2)}%`,
                    }}
                  >
                    {ttftRatio > 0 && (
                      <div
                        className="insights-wf-ttft"
                        style={{ width: `${(ttftRatio * 100).toFixed(1)}%` }}
                        title={`TTFT ${fmtDurationMs(span.ttftMs!)}`}
                      />
                    )}
                  </div>
                  <span
                    className={`insights-wf-dur${labelInsideRight ? " left" : ""}`}
                    style={
                      labelInsideRight
                        ? { right: `${(100 - leftPct + 0.8).toFixed(2)}%` }
                        : { left: `${(leftPct + widthPct + 0.8).toFixed(2)}%` }
                    }
                  >
                    {fmtDurationMs(span.durationMs)}
                    {span.costUsd > 0 ? ` · ${fmtUsd(span.costUsd)}` : ""}
                  </span>
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {selectedSpan && (
        <div className="insights-wf-inspector">
          <div className="insights-wf-insp-head">
            <span
              className={`insights-wf-tag ${selectedSpan.status === "error" ? "err" : selectedSpan.kind}`}
            >
              {selectedSpan.kind.toUpperCase()}
            </span>
            <strong>{selectedSpan.label}</strong>
            {selectedSpan.detail && (
              <span className="muted">{selectedSpan.detail}</span>
            )}
          </div>
          <div className="insights-wf-insp-meta">
            <span>
              起止 {fmtClockMs(selectedSpan.startMs)} →{" "}
              {fmtClockMs(selectedSpan.endMs)}
            </span>
            <span>
              偏移 +{fmtDurationMs(selectedSpan.offsetMs)} · 耗时{" "}
              <strong>{fmtDurationMs(selectedSpan.durationMs)}</strong>
            </span>
            {selectedSpan.ttftMs != null && selectedSpan.ttftMs > 0 && (
              <span>首字 TTFT {fmtDurationMs(selectedSpan.ttftMs)}</span>
            )}
            {selectedSpan.kind === "llm" && (
              <>
                <span>
                  新输入 {fmtTokens(selectedSpan.inputTokens)} · 缓存读{" "}
                  {fmtTokens(selectedSpan.cacheReadTokens)} · 输出{" "}
                  {fmtTokens(selectedSpan.outputTokens)}
                </span>
                <span>
                  费用 <strong>{fmtUsd(selectedSpan.costUsd)}</strong>
                </span>
              </>
            )}
            {selectedSpan.errorMessage && (
              <span className="insights-wf-err-text">
                异常：{selectedSpan.errorMessage}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function SessionDetailModal({
  sessionId,
  appType,
  appName,
  startDate,
  endDate,
  multiDevice,
  onClose,
}: {
  sessionId: string;
  appType: string;
  appName: string;
  startDate: number;
  endDate?: number;
  multiDevice: boolean;
  onClose: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const query = useQuery({
    queryKey: [
      "usage",
      "session-trace",
      sessionId,
      appType,
      startDate,
      endDate,
      multiDevice,
    ],
    queryFn: () =>
      usageCall<SessionTraceDetail>("session_trace", {
        sessionId,
        appType,
        startDate,
        endDate,
        multiDevice,
      }),
  });

  const [copied, setCopied] = useState(false);
  const [viewMode, setViewMode] = useState<"waterfall" | "turns">("waterfall");
  const [selectedTaskIdx, setSelectedTaskIdx] = useState<number | null>(null);
  const [ctxHoverIdx, setCtxHoverIdx] = useState<number | null>(null);
  const [modalTip, setModalTip] = useState<{
    x: number;
    y: number;
    title: string;
    lines: string[];
  } | null>(null);

  const showModalTip = (
    e: React.MouseEvent,
    title: string,
    lines: string[],
  ) => {
    setModalTip({
      x: Math.min(e.clientX + 14, window.innerWidth - 310),
      y: Math.min(e.clientY + 14, window.innerHeight - 160),
      title,
      lines,
    });
  };

  const detail = query.data;
  const turns = detail?.turns ?? [];
  const tasks = useMemo(() => detail?.tasks ?? [], [detail?.tasks]);
  const ctxSeries = turns.map(
    (t) => t.freshInput + t.cacheReadTokens + t.cacheCreationTokens,
  );
  const maxCtx = Math.max(1, ...ctxSeries);

  const defaultTaskIdx = useMemo(() => {
    if (tasks.length === 0) return null;
    let best = tasks[0];
    for (const t of tasks) {
      const score = (x: TaskTrace) =>
        x.toolCalls * 1000 + x.totalCostUsd * 1000 + x.durationMs / 1000;
      if (score(t) > score(best)) best = t;
    }
    return best.index;
  }, [tasks]);

  const activeTask =
    tasks.find((t) => t.index === (selectedTaskIdx ?? defaultTaskIdx)) ??
    tasks[0] ??
    null;

  const copyResume = () => {
    if (!detail?.resumeCommand) return;
    void navigator.clipboard?.writeText(detail.resumeCommand).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  };

  return (
    <div
      className="insights-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Agent 调用链与会话明细"
      onClick={onClose}
    >
      <div className="insights-modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="insights-modal-head">
          <div className="insights-modal-title-wrap">
            <div className="insights-modal-title">
              <AppGlyph app={appType} />
              <span>{detail?.title ?? appName}</span>
            </div>
            <div className="insights-modal-sub">
              <span>{appName}</span>
              <span>·</span>
              <code>{sessionId}</code>
              {detail?.projectDir && (
                <>
                  <span>·</span>
                  <span title={detail.projectDir}>{detail.projectDir}</span>
                </>
              )}
              {detail?.gitBranch && (
                <>
                  <span>·</span>
                  <span className="insights-branch-chip">
                    <GitBranch size={11} />
                    {detail.gitBranch}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="insights-modal-actions">
            {detail?.resumeCommand && (
              <button
                type="button"
                className="insights-copy-btn"
                onClick={copyResume}
                title={detail.resumeCommand}
              >
                {copied ? <Check size={13} /> : <Copy size={13} />}
                <span>{copied ? "已复制命令" : "复制恢复命令"}</span>
              </button>
            )}
            <button
              type="button"
              className="insights-modal-close"
              onClick={onClose}
              aria-label="关闭会话明细"
            >
              <X size={16} />
            </button>
          </div>
        </div>

        {query.isPending ? (
          <div className="insights-empty">正在解析调用链与会话明细…</div>
        ) : query.isError ? (
          <div className="insights-empty">读取失败：{String(query.error)}</div>
        ) : detail ? (
          <>
            <div className="insights-modal-bar">
              <div className="insights-modal-stats">
                <span className="insights-badge neutral">
                  {tasks.length} 个任务 · {detail.totalTurns} 轮调用
                </span>
                <span className="insights-badge neutral">
                  总费用 {fmtUsd(detail.totalCostUsd)}
                </span>
                <span className="insights-badge neutral">
                  总 Token {fmtTokens(detail.totalTokens)}
                </span>
                {ctxSeries.length > 0 && (
                  <span className="insights-badge neutral">
                    峰值上下文 {fmtTokens(maxCtx)}
                  </span>
                )}
                <span
                  className={`insights-badge ${detail.traceSource === "transcript" ? "good" : "neutral"}`}
                >
                  <Activity size={11} />{" "}
                  {detail.traceSource === "transcript"
                    ? "完整调用链（含工具）"
                    : "模型轮次瀑布"}
                </span>
              </div>

              <div className="insights-pill-group subtle">
                <button
                  type="button"
                  className={`insights-pill-btn${viewMode === "waterfall" ? " active" : ""}`}
                  onClick={() => setViewMode("waterfall")}
                >
                  <ListTree size={13} />
                  <span>调用链瀑布图</span>
                </button>
                <button
                  type="button"
                  className={`insights-pill-btn${viewMode === "turns" ? " active" : ""}`}
                  onClick={() => setViewMode("turns")}
                >
                  <Table2 size={13} />
                  <span>逐轮明细表</span>
                </button>
              </div>
            </div>

            {viewMode === "waterfall" ? (
              tasks.length === 0 ? (
                <div className="insights-empty">
                  当前时间范围内没有可展示的调用链任务
                </div>
              ) : (
                <div className="insights-wf-shell">
                  {tasks.length > 1 && (
                    <div className="insights-task-strip" role="tablist">
                      {tasks.map((t) => {
                        const active = activeTask?.index === t.index;
                        return (
                          <button
                            key={t.index}
                            type="button"
                            role="tab"
                            aria-selected={active}
                            className={`insights-task-pill${active ? " active" : ""}`}
                            onClick={() => setSelectedTaskIdx(t.index)}
                            title={t.prompt ?? `任务 #${t.index}`}
                          >
                            <span className="insights-task-pill-top">
                              <strong>#{t.index}</strong>
                              <span>{fmtDurationMs(t.durationMs)}</span>
                              {t.toolCalls > 0 && (
                                <span className="muted">
                                  {t.toolCalls} 工具
                                </span>
                              )}
                              {t.errorCount > 0 && (
                                <span className="insights-wf-err-text">!</span>
                              )}
                            </span>
                            <span className="insights-task-pill-prompt">
                              {t.prompt ?? `任务段 #${t.index}`}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  )}
                  {activeTask && (
                    <TaskWaterfall
                      task={activeTask}
                      onHover={showModalTip}
                      onLeave={() => setModalTip(null)}
                    />
                  )}
                </div>
              )
            ) : (
              <>
                {ctxSeries.length >= 2 && (
                  <div className="insights-modal-spark">
                    <div className="insights-card-sub">
                      上下文增长趋势（按轮次，悬浮查看单轮详情）
                    </div>
                    <svg
                      className="insights-svg"
                      viewBox="0 0 600 56"
                      style={{ height: 56 }}
                      onMouseLeave={() => {
                        setCtxHoverIdx(null);
                        setModalTip(null);
                      }}
                    >
                      <polyline
                        fill="none"
                        stroke="var(--accent-cyan)"
                        strokeWidth={2}
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        points={ctxSeries
                          .map(
                            (v, i) =>
                              `${((i / (ctxSeries.length - 1)) * 588 + 6).toFixed(1)},${(50 - (v / maxCtx) * 42).toFixed(1)}`,
                          )
                          .join(" ")}
                      />
                      {ctxHoverIdx != null && turns[ctxHoverIdx] && (
                        <>
                          <line
                            x1={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            y1={4}
                            x2={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            y2={52}
                            stroke="var(--accent-cyan)"
                            strokeWidth={1}
                            strokeDasharray="2 2"
                            opacity={0.7}
                          />
                          <circle
                            cx={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            cy={50 - (ctxSeries[ctxHoverIdx] / maxCtx) * 42}
                            r={3.5}
                            fill="var(--accent-cyan)"
                            stroke="var(--surface)"
                            strokeWidth={1.2}
                          />
                        </>
                      )}
                      {turns.map((t, i) => {
                        const cx = (i / (ctxSeries.length - 1)) * 588 + 6;
                        const band = 588 / Math.max(1, ctxSeries.length - 1);
                        return (
                          <rect
                            key={t.requestId}
                            x={Math.max(0, cx - band / 2)}
                            y={0}
                            width={band}
                            height={56}
                            fill="transparent"
                            style={{ cursor: "crosshair" }}
                            onMouseMove={(e) => {
                              setCtxHoverIdx(i);
                              showModalTip(
                                e,
                                `第 ${i + 1} 轮 · ${new Date(t.createdAt * 1000).toLocaleTimeString("zh-CN", { hour12: false })}`,
                                [
                                  `模型：${t.model}`,
                                  `上下文体积：${fmtTokens(ctxSeries[i])}（新输入 ${fmtTokens(t.freshInput)} · 缓存读 ${fmtTokens(t.cacheReadTokens)}）`,
                                  `输出：${fmtTokens(t.outputTokens)} · 费用：${fmtUsd(t.totalCostUsd)}`,
                                ],
                              );
                            }}
                          />
                        );
                      })}
                    </svg>
                  </div>
                )}

                <div className="insights-turns-wrap">
                  <table className="insights-turns-table">
                    <thead>
                      <tr>
                        <th>#</th>
                        <th>时间</th>
                        <th>模型</th>
                        <th className="num">新输入</th>
                        <th className="num">缓存读</th>
                        <th className="num">输出</th>
                        <th className="num">耗时</th>
                        <th className="num">速度</th>
                        <th className="num">费用</th>
                        <th className="num">状态</th>
                      </tr>
                    </thead>
                    <tbody>
                      {turns.map((t, idx) => (
                        <tr key={t.requestId}>
                          <td className="num">{idx + 1}</td>
                          <td>
                            {new Date(t.createdAt * 1000).toLocaleTimeString(
                              "zh-CN",
                              { hour12: false },
                            )}
                          </td>
                          <td className="model-cell" title={t.model}>
                            {t.model}
                          </td>
                          <td className="num">{fmtTokens(t.freshInput)}</td>
                          <td className="num">
                            {fmtTokens(t.cacheReadTokens)}
                          </td>
                          <td className="num">{fmtTokens(t.outputTokens)}</td>
                          <td className="num">
                            {t.latencyMs > 0
                              ? `${(t.latencyMs / 1000).toFixed(1)}s`
                              : "—"}
                          </td>
                          <td className="num">
                            {formatRequestSpeed({
                              outputTokens: t.outputTokens,
                              latencyMs: t.latencyMs,
                              firstTokenMs: t.firstTokenMs,
                              dataSource: t.dataSource,
                            })}
                          </td>
                          <td className="num">{fmtUsd(t.totalCostUsd)}</td>
                          <td className="num">
                            <span
                              className={`insights-status-code ${t.statusCode >= 200 && t.statusCode < 300 ? "ok" : "err"}`}
                            >
                              {t.statusCode || "ERR"}
                            </span>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}
          </>
        ) : null}
      </div>
      {modalTip && (
        <div
          className="insights-tooltip"
          style={{ left: modalTip.x, top: modalTip.y, zIndex: 95 }}
        >
          <strong>{modalTip.title}</strong>
          {modalTip.lines.map((line, i) => (
            <div key={i} style={{ color: "var(--text-secondary)" }}>
              {line}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
