import {
  appColor,
  type DailyCostPoint,
  type DailyTokenMix,
  fmtTokens,
  fmtUsd,
  type ModelStability,
  projectBase,
  type ProjectCostRow,
  type SessionBubble,
  type StabMode,
  type StatusHealth,
} from "./insightsShared";
import { useMemo, useState } from "react";
import { FolderGit2 } from "lucide-react";
import { AppGlyph } from "../usage/AppGlyph";

export function ForecastSparkline({
  dailyCosts,
  projected,
  budget,
  onHover,
  onLeave,
}: {
  dailyCosts: DailyCostPoint[];
  projected: number;
  budget: number;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const cumulative = useMemo(() => {
    let acc = 0;
    return dailyCosts.map((d) => {
      acc += d.cost;
      return acc;
    });
  }, [dailyCosts]);

  const actualEnd = cumulative[cumulative.length - 1] ?? 0;
  const maxY = Math.max(actualEnd, projected, budget, 1) * 1.15;
  const yOf = (v: number) => 44 - (v / maxY) * 38;
  const splitX = 180;
  const xOf = (i: number) =>
    cumulative.length >= 2 ? (i / (cumulative.length - 1)) * splitX : splitX;
  const pts =
    cumulative.length >= 2
      ? cumulative
          .map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`)
          .join(" ")
      : `0,${yOf(0).toFixed(1)} ${splitX},${yOf(actualEnd).toFixed(1)}`;
  const budgetY = yOf(budget);
  const actualY = yOf(actualEnd);
  const projY = yOf(projected);
  const bandW =
    dailyCosts.length > 0 ? splitX / Math.max(1, dailyCosts.length) : splitX;

  return (
    <svg
      className="insights-svg"
      viewBox="0 0 280 50"
      style={{ height: 48 }}
      onMouseLeave={() => {
        setHoverIdx(null);
        onLeave();
      }}
    >
      <line
        x1={0}
        y1={budgetY}
        x2={280}
        y2={budgetY}
        stroke="var(--accent-red)"
        strokeWidth={1}
        strokeDasharray="3 3"
        opacity={0.6}
      />
      <polyline
        fill="none"
        stroke="var(--accent-violet)"
        strokeWidth={2.2}
        strokeLinecap="round"
        strokeLinejoin="round"
        points={pts}
      />
      <line
        x1={splitX}
        y1={actualY}
        x2={274}
        y2={projY}
        stroke="var(--accent-violet)"
        strokeWidth={2}
        strokeDasharray="4 3"
        opacity={0.7}
      />
      {hoverIdx != null && dailyCosts[hoverIdx] && (
        <>
          <line
            x1={xOf(hoverIdx)}
            y1={4}
            x2={xOf(hoverIdx)}
            y2={46}
            stroke="var(--accent-violet)"
            strokeWidth={1}
            strokeDasharray="2 2"
            opacity={0.55}
          />
          <circle
            cx={xOf(hoverIdx)}
            cy={yOf(cumulative[hoverIdx])}
            r={3.5}
            fill="var(--accent-violet)"
            stroke="var(--surface)"
            strokeWidth={1.2}
          />
        </>
      )}
      <circle cx={splitX} cy={actualY} r={3.5} fill="var(--accent-violet)" />
      <circle
        cx={274}
        cy={projY}
        r={3.5}
        fill={projected > budget ? "var(--accent-red)" : "var(--accent-green)"}
      />
      {dailyCosts.map((d, i) => (
        <rect
          key={d.date}
          x={Math.max(0, xOf(i) - bandW / 2)}
          y={0}
          width={bandW}
          height={50}
          fill="transparent"
          style={{ cursor: "crosshair" }}
          onMouseMove={(e) => {
            setHoverIdx(i);
            onHover(e, d.date, [
              `当日费用：${fmtUsd(d.cost)}`,
              `范围累计：${fmtUsd(cumulative[i])}`,
            ]);
          }}
        />
      ))}
      <rect
        x={splitX + 2}
        y={0}
        width={280 - splitX - 2}
        height={50}
        fill="transparent"
        style={{ cursor: "crosshair" }}
        onMouseMove={(e) => {
          setHoverIdx(null);
          onHover(e, "30 天预测趋势", [
            `当前累计：${fmtUsd(actualEnd)}`,
            `30 天预测：${fmtUsd(projected)}`,
            `月度预算：${fmtUsd(budget)}（${projected > budget ? "预计超支" : "预算内"}）`,
          ]);
        }}
      />
    </svg>
  );
}

export function SessionScatter({
  sessions,
  sourceName,
  onHover,
  onLeave,
  onSelect,
}: {
  sessions: SessionBubble[];
  sourceName: Map<string, string>;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
  onSelect: (session: SessionBubble) => void;
}) {
  const scatterSessions = sessions.slice(0, 40);
  if (scatterSessions.length === 0) {
    return (
      <div className="insights-empty">当前筛选范围内暂无会话级明细记录</div>
    );
  }
  const W = 560;
  const H = 220;
  const padL = 48;
  const padR = 18;
  const padT = 14;
  const padB = 28;
  const maxTurns = Math.max(10, ...scatterSessions.map((s) => s.turns)) * 1.1;
  const maxCost =
    Math.max(0.1, ...scatterSessions.map((s) => s.costUsd)) * 1.15;
  const maxCtx = Math.max(
    10_000,
    ...scatterSessions.map((s) => s.avgContextTokens),
  );
  const xOf = (t: number) => padL + (t / maxTurns) * (W - padL - padR);
  const yOf = (c: number) => H - padB - (c / maxCost) * (H - padT - padB);

  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((p) => maxCost * p);
  const xTicks = [0, 0.25, 0.5, 0.75, 1].map((p) => Math.round(maxTurns * p));

  return (
    <svg
      className="insights-svg"
      viewBox={`0 0 ${W} ${H}`}
      style={{ height: 220 }}
    >
      {yTicks.map((v, i) => {
        const y = yOf(v);
        return (
          <g key={i}>
            <line
              x1={padL}
              y1={y}
              x2={W - padR}
              y2={y}
              stroke="var(--line)"
              strokeDasharray="3 3"
              opacity={0.7}
            />
            <text
              x={padL - 8}
              y={y + 4}
              textAnchor="end"
              fill="var(--muted)"
              fontSize={11}
            >
              {fmtUsd(v)}
            </text>
          </g>
        );
      })}
      {xTicks.map((v, i) => (
        <text
          key={i}
          x={xOf(v)}
          y={H - 8}
          textAnchor="middle"
          fill="var(--muted)"
          fontSize={11}
        >
          {v} 轮
        </text>
      ))}
      {scatterSessions.map((s) => {
        const cx = xOf(s.turns);
        const cy = yOf(s.costUsd);
        const r = 5 + (s.avgContextTokens / maxCtx) * 12;
        const col = appColor(s.appType);
        const label = sourceName.get(s.appType) ?? s.appType;
        const shortId =
          s.sessionId.length > 18
            ? `${s.sessionId.slice(0, 8)}…${s.sessionId.slice(-6)}`
            : s.sessionId;
        const heading = s.title ? s.title : `${label} · ${shortId}`;
        const proj = projectBase(s.projectDir);
        return (
          <circle
            key={`${s.appType}:${s.sessionId}`}
            cx={cx}
            cy={cy}
            r={r}
            fill={col}
            fillOpacity={0.55}
            stroke={col}
            strokeWidth={1.5}
            style={{ cursor: "pointer" }}
            onClick={() => onSelect(s)}
            onMouseMove={(e) =>
              onHover(e, heading, [
                `${label}${proj ? ` · ${proj}` : ""} · ${s.turns} 轮`,
                `费用 ${fmtUsd(s.costUsd)} · 平均上下文 ${fmtTokens(s.avgContextTokens)}`,
                "点击查看逐轮明细",
              ])
            }
            onMouseLeave={onLeave}
          />
        );
      })}
    </svg>
  );
}

export function StabilityDumbbell({
  items,
  mode,
  onHover,
  onLeave,
}: {
  items: ModelStability[];
  mode: StabMode;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const rows = items
    .map((m) => {
      const lo = mode === "ttft" ? m.ttftP50Ms : m.speedP10;
      const hi = mode === "ttft" ? m.ttftP95Ms : m.speedP50;
      const est = mode === "ttft" ? m.ttftEstimated : m.speedEstimated;
      return { raw: m, model: m.model, lo, hi, est };
    })
    .filter(
      (
        r,
      ): r is {
        raw: ModelStability;
        model: string;
        lo: number;
        hi: number;
        est: boolean;
      } => r.lo != null && r.hi != null,
    );

  if (rows.length === 0) {
    return (
      <div className="insights-empty">
        当前范围暂无足够的{mode === "ttft" ? "首字耗时" : "输出速度"}样本
      </div>
    );
  }

  const maxVal = Math.max(1, ...rows.map((r) => Math.max(r.lo, r.hi))) * 1.1;
  const unit = mode === "ttft" ? "ms" : "tok/s";
  const palette = [
    "var(--accent-violet)",
    "var(--accent-cyan)",
    "var(--accent-pink)",
    "var(--accent-orange)",
    "var(--accent-blue)",
    "var(--accent-green)",
  ];

  return (
    <div className="insights-dumbbell-list">
      {rows.map((r, i) => {
        const minV = Math.min(r.lo, r.hi);
        const maxV = Math.max(r.lo, r.hi);
        const leftPct = (minV / maxVal) * 100;
        const widthPct = Math.max(1.5, ((maxV - minV) / maxVal) * 100);
        const color = palette[i % palette.length];
        const fmt = (v: number) =>
          mode === "ttft" ? String(Math.round(v)) : v.toFixed(1);
        const tipLines = [`样本数：${r.raw.requests.toLocaleString()} 次`];
        if (r.raw.ttftP50Ms != null && r.raw.ttftP95Ms != null) {
          tipLines.push(
            `首字延迟：P50 ${Math.round(r.raw.ttftP50Ms)}ms · P95 ${Math.round(r.raw.ttftP95Ms)}ms${r.raw.ttftEstimated ? "（估算）" : ""}`,
          );
        }
        if (r.raw.speedP10 != null && r.raw.speedP50 != null) {
          tipLines.push(
            `输出速度：P10 ${r.raw.speedP10.toFixed(1)} · P50 ${r.raw.speedP50.toFixed(1)} tok/s${r.raw.speedEstimated ? "（估算）" : ""}`,
          );
        }
        return (
          <div
            key={r.model}
            className="insights-dumbbell-item"
            onMouseMove={(e) => onHover(e, r.model, tipLines)}
            onMouseLeave={onLeave}
          >
            <div className="insights-dumbbell-head">
              <span className="insights-dumbbell-model" title={r.model}>
                {r.model}
              </span>
              <span className="insights-dumbbell-val">
                {r.est ? "≈ " : ""}
                {fmt(minV)} – {fmt(maxV)} {unit}
              </span>
            </div>
            <div className="insights-dumbbell-track">
              <div className="insights-dumbbell-rail" />
              <div
                className="insights-dumbbell-span"
                style={{
                  left: `${leftPct}%`,
                  width: `${widthPct}%`,
                  background: color,
                }}
              />
              <div
                className="insights-dumbbell-node"
                style={{ left: `${leftPct}%`, background: color }}
              />
              <div
                className="insights-dumbbell-node"
                style={{ left: `${leftPct + widthPct}%`, background: color }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function TokenMixArea({
  points,
  onHover,
  onLeave,
}: {
  points: DailyTokenMix[];
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const valid = points.filter(
    (p) => p.freshInput + p.output + p.cacheCreation + p.cacheRead > 0,
  );
  if (valid.length === 0) {
    return <div className="insights-empty">当前范围暂无 Token 结构数据</div>;
  }

  const W = 640;
  const H = 165;
  const padL = 34;
  const padR = 12;
  const padT = 10;
  const padB = 24;
  const xOf = (i: number) =>
    valid.length === 1
      ? (padL + W - padR) / 2
      : padL + (i / (valid.length - 1)) * (W - padL - padR);
  const yOf = (p: number) => H - padB - (p / 100) * (H - padT - padB);

  const normalized = valid.map((p) => {
    const sum = p.freshInput + p.output + p.cacheCreation + p.cacheRead;
    return [
      (p.cacheRead / sum) * 100,
      (p.cacheCreation / sum) * 100,
      (p.freshInput / sum) * 100,
      (p.output / sum) * 100,
    ];
  });

  const colors = [
    "var(--accent-cyan)",
    "var(--accent-orange)",
    "var(--accent-violet)",
    "var(--accent-pink)",
  ];

  const base = new Array(valid.length).fill(0);
  const polygons = colors.map((color, layer) => {
    const top = base.map((b, i) => b + normalized[i][layer]);
    const pts =
      valid.length === 1
        ? `${padL},${yOf(top[0])} ${W - padR},${yOf(top[0])} ${W - padR},${yOf(base[0])} ${padL},${yOf(base[0])}`
        : `${top.map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`).join(" ")} ${base
            .map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`)
            .reverse()
            .join(" ")}`;
    for (let i = 0; i < valid.length; i++) base[i] = top[i];
    return { color, pts };
  });

  return (
    <svg
      className="insights-svg"
      viewBox={`0 0 ${W} ${H}`}
      style={{ height: 165 }}
      onMouseLeave={() => {
        setHoverIdx(null);
        onLeave();
      }}
    >
      {[0, 50, 100].map((p) => (
        <g key={p}>
          <line
            x1={padL}
            y1={yOf(p)}
            x2={W - padR}
            y2={yOf(p)}
            stroke="var(--line)"
            strokeDasharray="3 3"
            opacity={0.6}
          />
          <text
            x={padL - 6}
            y={yOf(p) + 4}
            textAnchor="end"
            fill="var(--muted)"
            fontSize={11}
          >
            {p}%
          </text>
        </g>
      ))}
      {polygons.map((poly, idx) => (
        <polygon key={idx} points={poly.pts} fill={poly.color} opacity={0.75} />
      ))}
      {hoverIdx != null && valid[hoverIdx] && (
        <line
          x1={xOf(hoverIdx)}
          y1={padT}
          x2={xOf(hoverIdx)}
          y2={H - padB}
          stroke="var(--text)"
          strokeWidth={1.2}
          strokeDasharray="3 2"
          opacity={0.55}
        />
      )}
      {valid.map((p, i) => {
        const showLabel =
          i === 0 ||
          i === valid.length - 1 ||
          i % Math.max(1, Math.ceil(valid.length / 6)) === 0;
        const pcts = normalized[i];
        const sum = p.freshInput + p.output + p.cacheCreation + p.cacheRead;
        return (
          <g key={p.date}>
            {showLabel && (
              <text
                x={xOf(i)}
                y={H - 6}
                textAnchor="middle"
                fill="var(--muted)"
                fontSize={11}
              >
                {p.date.slice(5)}
              </text>
            )}
            <rect
              x={xOf(i) - (W - padL - padR) / Math.max(2, valid.length * 2)}
              y={padT}
              width={(W - padL - padR) / Math.max(1, valid.length)}
              height={H - padT - padB}
              fill="transparent"
              style={{ cursor: "crosshair" }}
              onMouseMove={(e) => {
                setHoverIdx(i);
                onHover(e, p.date, [
                  `总消耗 ${fmtTokens(sum)}`,
                  `缓存读取 ${pcts[0].toFixed(1)}% (${fmtTokens(p.cacheRead)})`,
                  `缓存写入 ${pcts[1].toFixed(1)}% (${fmtTokens(p.cacheCreation)})`,
                  `新输入 ${pcts[2].toFixed(1)}% (${fmtTokens(p.freshInput)})`,
                  `输出 ${pcts[3].toFixed(1)}% (${fmtTokens(p.output)})`,
                ]);
              }}
            />
          </g>
        );
      })}
    </svg>
  );
}

export function StatusPanel({
  status,
  onHover,
  onLeave,
}: {
  status?: StatusHealth;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const total = status?.total ?? 0;
  if (total === 0) {
    return <div className="insights-empty">当前范围暂无状态码明细</div>;
  }
  const items = [
    {
      label: "2xx 正常完成",
      count: status!.ok2xx,
      color: "var(--accent-green)",
    },
    {
      label: "429 限流重试",
      count: status!.rateLimit429,
      color: "var(--accent-orange)",
    },
    {
      label: "5xx 服务端异常",
      count: status!.server5xx,
      color: "var(--accent-red)",
    },
    {
      label: "其他异常 / 中断",
      count: status!.otherError,
      color: "var(--muted)",
    },
  ];
  const okRate = (status!.ok2xx / total) * 100;
  const diag =
    status!.rateLimit429 > 0 && status!.peak429Hour != null
      ? `共记录 ${status!.rateLimit429} 次 429 限流，高发于 ${String(status!.peak429Hour).padStart(2, "0")}:00 时段。`
      : status!.server5xx > 0
        ? `共出现 ${status!.server5xx} 次 5xx 服务端错误，整体成功率 ${okRate.toFixed(2)}%。`
        : `整体成功率 ${okRate.toFixed(2)}%，当前范围无 429 限流或 5xx 异常。`;

  return (
    <>
      <div className="insights-bar-track">
        {items.map((it) => (
          <div
            key={it.label}
            style={{
              width: `${(it.count / total) * 100}%`,
              background: it.color,
              cursor: it.count > 0 ? "pointer" : "default",
            }}
            onMouseMove={(e) =>
              it.count > 0 &&
              onHover(e, it.label, [
                `请求数：${it.count.toLocaleString()} 次`,
                `占比：${((it.count / total) * 100).toFixed(2)}%（共 ${total.toLocaleString()} 次）`,
              ])
            }
            onMouseLeave={onLeave}
          />
        ))}
      </div>
      <div className="insights-status-rows">
        {items.map((it) => (
          <div key={it.label} className="insights-status-row">
            <span style={{ color: "var(--text-secondary)" }}>
              <span className="insights-dot" style={{ background: it.color }} />
              {it.label}
            </span>
            <span>
              <strong>{it.count.toLocaleString()}</strong>{" "}
              <span style={{ color: "var(--muted)" }}>
                ({((it.count / total) * 100).toFixed(1)}%)
              </span>
            </span>
          </div>
        ))}
      </div>
      <div className="insights-diag-box">
        <strong>自动诊断：</strong>
        {diag}
      </div>
    </>
  );
}

export function ProjectRanking({
  projects,
  selectedDir,
  onSelect,
  onHover,
  onLeave,
}: {
  projects: ProjectCostRow[];
  selectedDir: string | null;
  onSelect: (dir: string) => void;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const peak = Math.max(0.01, ...projects.map((p) => p.costUsd));
  const totalCost = projects.reduce((acc, p) => acc + p.costUsd, 0);
  return (
    <div className="insights-project-list">
      {projects.map((p) => {
        const active = selectedDir === p.projectDir;
        const share = totalCost > 0 ? (p.costUsd / totalCost) * 100 : 0;
        return (
          <button
            key={p.projectDir}
            type="button"
            className={`insights-project-row${active ? " active" : ""}`}
            onClick={() => onSelect(p.projectDir)}
            onMouseMove={(e) =>
              onHover(e, p.projectName, [
                `路径：${p.projectDir}`,
                `费用：${fmtUsd(p.costUsd)}（占比 ${share.toFixed(1)}%）`,
                `规模：${p.sessions} 会话 · ${p.requests} 次请求 · ${fmtTokens(p.totalTokens)} Token`,
                "点击可按此项目筛选下方会话",
              ])
            }
            onMouseLeave={onLeave}
          >
            <div className="insights-project-head">
              <span className="insights-project-name">
                <FolderGit2 size={14} />
                <span>{p.projectName}</span>
                <span className="insights-project-apps">
                  {p.apps.map((id) => (
                    <AppGlyph key={id} app={id} />
                  ))}
                </span>
              </span>
              <span className="insights-project-stats">
                <span>
                  {p.sessions} 会话 · {p.requests} 次 ·{" "}
                  {fmtTokens(p.totalTokens)}
                </span>
                <strong>{fmtUsd(p.costUsd)}</strong>
              </span>
            </div>
            <div className="insights-bar-track" style={{ height: 6 }}>
              <div
                style={{
                  width: `${Math.max(2, (p.costUsd / peak) * 100)}%`,
                  background: active
                    ? "var(--accent-pink)"
                    : "var(--accent-violet)",
                }}
              />
            </div>
          </button>
        );
      })}
    </div>
  );
}
