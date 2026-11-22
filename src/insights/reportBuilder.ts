import { formatSubscriptionPrice } from "../subscriptions/format";

export interface ReportCacheSavings {
  actualCostUsd: number;
  cacheReadCostUsd: number;
  savedUsd: number;
  cacheHitRate: number;
  totalRequests: number;
  avgContextTokens: number;
}

export interface ReportDailyCostPoint {
  date: string;
  cost: number;
}

export interface ReportCostForecast {
  currentCostUsd: number;
  todayCostUsd: number;
  previousCostUsd: number;
  deltaPct: number | null;
  elapsedDays: number;
  dailyAvgUsd: number;
  projected30dUsd: number;
  dailyCosts: ReportDailyCostPoint[];
}

export interface ReportRhythmCell {
  weekday: number;
  hour: number;
  requests: number;
  costUsd: number;
  speedOutputTokens: number;
  speedDurationMs: number;
}

export interface ReportSessionBubble {
  sessionId: string;
  appType: string;
  title: string | null;
  projectDir: string | null;
  turns: number;
  costUsd: number;
  avgContextTokens: number;
  totalTokens: number;
  outputTokens: number;
  lastAt: number;
}

export interface ReportProjectCostRow {
  projectDir: string;
  projectName: string;
  costUsd: number;
  requests: number;
  sessions: number;
  totalTokens: number;
  apps: string[];
}

export interface ReportModelStability {
  model: string;
  requests: number;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  ttftEstimated: boolean;
  speedP10: number | null;
  speedP50: number | null;
  speedEstimated: boolean;
}

export interface ReportDailyTokenMix {
  date: string;
  freshInput: number;
  output: number;
  cacheCreation: number;
  cacheRead: number;
}

export interface ReportStatusHealth {
  total: number;
  ok2xx: number;
  rateLimit429: number;
  server5xx: number;
  otherError: number;
  peak429Hour: number | null;
}

export interface ReportSubscriptionRoi {
  id: string;
  name: string;
  appType: string;
  price: number;
  currency: string;
  billing: string;
  customIntervalDays?: number | null;
  rangeApiCostUsd: number;
  cycleApiCostUsd: number;
  cycleRequests: number;
  usdRoiMultiple: number | null;
}

export interface ReportInsightsData {
  cacheSavings: ReportCacheSavings;
  forecast: ReportCostForecast;
  rhythm: ReportRhythmCell[];
  sessions: ReportSessionBubble[];
  projects: ReportProjectCostRow[];
  stability: ReportModelStability[];
  tokenMix: ReportDailyTokenMix[];
  status: ReportStatusHealth;
  subscriptions: ReportSubscriptionRoi[];
}

export interface ReportOptions {
  data: ReportInsightsData;
  rangeLabel: string;
  scopeLabel: string;
  sourceName: Map<string, string>;
  budget: number;
  dailyBudget: number;
}

const WEEKDAYS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

function fmtUsd(v: number): string {
  if (!Number.isFinite(v) || v <= 0) return "$0.00";
  if (v < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(Math.round(n));
}

function esc(raw: string): string {
  return raw
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function tipAttrs(title: string, lines: string[]): string {
  return `data-tip-t="${esc(title)}" data-tip-b="${esc(lines.join("\n"))}"`;
}

function projectBasename(dir: string | null): string {
  if (!dir) return "";
  const parts = dir.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || dir;
}

function sumTokenMix(tokenMix: ReportDailyTokenMix[]) {
  let freshInput = 0;
  let output = 0;
  let cacheCreation = 0;
  let cacheRead = 0;
  for (const d of tokenMix) {
    freshInput += d.freshInput;
    output += d.output;
    cacheCreation += d.cacheCreation;
    cacheRead += d.cacheRead;
  }
  return {
    freshInput,
    output,
    cacheCreation,
    cacheRead,
    total: freshInput + output + cacheCreation + cacheRead,
  };
}

export function buildMarkdownReport(opts: ReportOptions): string {
  const { data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget } =
    opts;
  const cs = data.cacheSavings;
  const fc = data.forecast;
  const st = data.status;
  const tok = sumTokenMix(data.tokenMix);
  const fullCost = cs.actualCostUsd + cs.savedUsd;
  const savedPct =
    fullCost > 0 ? Math.round((cs.savedUsd / fullCost) * 100) : 0;
  const nowStr = new Date().toLocaleDateString("zh-CN");

  let peakCell: ReportRhythmCell | null = null;
  for (const c of data.rhythm) {
    if (!peakCell || c.costUsd > peakCell.costUsd) peakCell = c;
  }

  const lines: string[] = [
    `# Observatory AI 编程复盘报告（${rangeLabel}）`,
    "",
    `- **统计范围**：${scopeLabel}`,
    `- **生成日期**：${nowStr}`,
    "",
    "## 核心指标",
    "",
    "| 指标 | 数值 |",
    "| --- | --- |",
    `| 预估实付费用 | ${fmtUsd(cs.actualCostUsd)} |`,
    `| 今日费用 / 日预算 | ${fmtUsd(fc.todayCostUsd)} / ${fmtUsd(dailyBudget)} |`,
    `| 总 Token 消耗 | **${fmtTokens(tok.total)}**（新输入 ${fmtTokens(tok.freshInput)} · 输出 ${fmtTokens(tok.output)} · 缓存读 ${fmtTokens(tok.cacheRead)} · 缓存写 ${fmtTokens(tok.cacheCreation)}） |`,
    `| 缓存省下金额 | ${fmtUsd(cs.savedUsd)}（省 ${savedPct}%，命中率 ${(cs.cacheHitRate * 100).toFixed(1)}%） |`,
    `| 总请求次数 | ${cs.totalRequests.toLocaleString()} 次 |`,
    `| 平均上下文体积 | ${fmtTokens(cs.avgContextTokens)} tokens/次 |`,
    `| 有效日均 / 30 天预测 | ${fmtUsd(fc.dailyAvgUsd)}/天 · 预测 ${fmtUsd(fc.projected30dUsd)}（月预算 ${fmtUsd(budget)}） |`,
    `| 成功率 | ${st.total > 0 ? `${((st.ok2xx / st.total) * 100).toFixed(2)}%` : "—"}（429 限流 ${st.rateLimit429} 次，5xx ${st.server5xx} 次） |`,
  ];

  if (peakCell) {
    lines.push(
      `| 消费高峰时段 | ${WEEKDAYS[peakCell.weekday]} ${String(peakCell.hour).padStart(2, "0")}:00（${fmtUsd(peakCell.costUsd)}，${peakCell.requests} 次） |`,
    );
  }

  if (data.subscriptions.length > 0) {
    lines.push("", "## 订阅回本仪", "");
    lines.push("| 订阅 | 账单价 | 本周期等效 API 价值 | 回本倍数 |");
    lines.push("| --- | --- | ---: | ---: |");
    for (const sub of data.subscriptions) {
      const priceStr = formatSubscriptionPrice(
        sub.price,
        sub.currency,
        sub.billing,
        sub.customIntervalDays,
      );
      const roiStr =
        sub.usdRoiMultiple != null
          ? `${sub.usdRoiMultiple.toFixed(1)}×`
          : "原币种";
      lines.push(
        `| ${sub.name.replace(/\|/g, "/")} | ${priceStr} | ${fmtUsd(sub.cycleApiCostUsd)} | ${roiStr} |`,
      );
    }
  }

  if (data.projects.length > 0) {
    lines.push("", "## 项目 / 仓库费用排行 Top 8", "");
    lines.push("| 项目 | 会话数 | 请求数 | Token | 费用 |");
    lines.push("| --- | ---: | ---: | ---: | ---: |");
    for (const p of data.projects.slice(0, 8)) {
      lines.push(
        `| ${p.projectName.replace(/\|/g, "/")} | ${p.sessions} | ${p.requests} | ${fmtTokens(p.totalTokens)} | ${fmtUsd(p.costUsd)} |`,
      );
    }
  }

  if (data.sessions.length > 0) {
    lines.push("", "## 最贵会话 Top 8", "");
    lines.push("| 会话 | 项目 | 平台 | 轮次 | 平均上下文 | 费用 |");
    lines.push("| --- | --- | --- | ---: | ---: | ---: |");
    for (const s of data.sessions.slice(0, 8)) {
      const name = (s.title ?? s.sessionId.slice(0, 14)).replace(/\|/g, "/");
      const proj = (projectBasename(s.projectDir) || "—").replace(/\|/g, "/");
      const appLabel = sourceName.get(s.appType) ?? s.appType;
      lines.push(
        `| ${name} | ${proj} | ${appLabel} | ${s.turns} | ${fmtTokens(s.avgContextTokens)} | ${fmtUsd(s.costUsd)} |`,
      );
    }
  }

  return lines.join("\n");
}

function buildDailyCostSvg(
  dailyCosts: ReportDailyCostPoint[],
  dailyAvg: number,
  dailyBudget: number,
): string {
  if (dailyCosts.length === 0) {
    return `<div class="empty">当前时间范围内暂无按日费用明细</div>`;
  }
  const W = 680;
  const H = 190;
  const padL = 46;
  const padR = 18;
  const padT = 18;
  const padB = 28;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;

  const maxVal = Math.max(
    0.01,
    dailyAvg * 1.15,
    ...dailyCosts.map((d) => d.cost),
  );
  const showBudgetLine = dailyBudget > 0 && dailyBudget <= maxVal * 1.4;
  const scaleMax = showBudgetLine
    ? Math.max(maxVal, dailyBudget * 1.08)
    : maxVal;

  const barStep = plotW / Math.max(1, dailyCosts.length);
  const barW = Math.max(4, Math.min(28, barStep * 0.62));
  const yOf = (v: number) =>
    padT + plotH - (Math.min(v, scaleMax) / scaleMax) * plotH;

  const gridYs = [0, 0.5, 1].map((r) => {
    const val = scaleMax * r;
    const y = yOf(val);
    return `
      <line x1="${padL}" y1="${y.toFixed(1)}" x2="${W - padR}" y2="${y.toFixed(1)}" stroke="#e5e7eb" stroke-dasharray="${r === 0 ? "" : "3 3"}" />
      <text x="${padL - 8}" y="${(y + 4).toFixed(1)}" text-anchor="end" font-size="10" fill="#6b7280">${esc(fmtUsd(val))}</text>`;
  });

  let peakIdx = 0;
  dailyCosts.forEach((d, i) => {
    if (d.cost > dailyCosts[peakIdx].cost) peakIdx = i;
  });

  const labelStep = Math.max(1, Math.ceil(dailyCosts.length / 7));
  const bars = dailyCosts.map((d, i) => {
    const cx = padL + i * barStep + barStep / 2;
    const x = cx - barW / 2;
    const y = yOf(d.cost);
    const h = Math.max(1.5, padT + plotH - y);
    const isPeak = i === peakIdx && d.cost > 0;
    const color = isPeak ? "#7c3aed" : "#8b5cf6";
    const xLabel =
      i % labelStep === 0 || i === dailyCosts.length - 1
        ? `<text x="${cx.toFixed(1)}" y="${H - 8}" text-anchor="middle" font-size="10" fill="#6b7280">${esc(d.date.slice(5))}</text>`
        : "";
    const peakTag = isPeak
      ? `<text x="${cx.toFixed(1)}" y="${Math.max(11, y - 5).toFixed(1)}" text-anchor="middle" font-size="10" font-weight="700" fill="#6d28d9">${esc(fmtUsd(d.cost))}</text>`
      : "";
    const vsAvg =
      dailyAvg > 0
        ? `较日均 ${d.cost >= dailyAvg ? "+" : ""}${(((d.cost - dailyAvg) / dailyAvg) * 100).toFixed(0)}%`
        : "—";
    const tip = tipAttrs(d.date, [
      `当日费用：${fmtUsd(d.cost)}`,
      `有效日均：${fmtUsd(dailyAvg)}（${vsAvg}）`,
      ...(dailyBudget > 0
        ? [`占日预算：${((d.cost / dailyBudget) * 100).toFixed(0)}%`]
        : []),
    ]);
    return `
      <rect x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${barW.toFixed(1)}" height="${h.toFixed(1)}" rx="3" fill="${color}" opacity="${isPeak ? "1" : "0.78"}" />
      <rect class="hover-band" x="${(padL + i * barStep).toFixed(1)}" y="${padT}" width="${barStep.toFixed(1)}" height="${plotH}" ${tip}>
        <title>${esc(d.date)}: ${esc(fmtUsd(d.cost))}</title>
      </rect>
      ${peakTag}
      ${xLabel}`;
  });

  const avgY = yOf(dailyAvg);
  const avgLine = `
    <line x1="${padL}" y1="${avgY.toFixed(1)}" x2="${W - padR}" y2="${avgY.toFixed(1)}" stroke="#0891b2" stroke-width="1.4" stroke-dasharray="4 3" />
    <text x="${W - padR}" y="${Math.max(11, avgY - 4).toFixed(1)}" text-anchor="end" font-size="10" fill="#0891b2">日均 ${esc(fmtUsd(dailyAvg))}</text>`;

  const budgetLine = showBudgetLine
    ? `<line x1="${padL}" y1="${yOf(dailyBudget).toFixed(1)}" x2="${W - padR}" y2="${yOf(dailyBudget).toFixed(1)}" stroke="#ef4444" stroke-width="1.2" stroke-dasharray="3 3" />
       <text x="${padL + 4}" y="${Math.max(11, yOf(dailyBudget) - 4).toFixed(1)}" font-size="10" fill="#ef4444">日预算 ${esc(fmtUsd(dailyBudget))}</text>`
    : "";

  return `<svg viewBox="0 0 ${W} ${H}" class="svg-chart" role="img" aria-label="每日费用趋势">
    ${gridYs.join("")}
    ${bars.join("")}
    ${avgLine}
    ${budgetLine}
  </svg>`;
}

function buildTokenMixSvg(tokenMix: ReportDailyTokenMix[]): string {
  if (tokenMix.length === 0) {
    return `<div class="empty">当前时间范围内暂无 Token 结构明细</div>`;
  }
  const W = 680;
  const H = 185;
  const padL = 46;
  const padR = 16;
  const padT = 16;
  const padB = 28;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;
  const totals = tokenMix.map(
    (d) => d.freshInput + d.output + d.cacheCreation + d.cacheRead,
  );
  const maxTot = Math.max(1, ...totals);
  const step = plotW / Math.max(1, tokenMix.length);
  const barW = Math.max(4, Math.min(26, step * 0.62));
  const labelStep = Math.max(1, Math.ceil(tokenMix.length / 7));

  const grid = [0, 0.5, 1].map((r) => {
    const val = maxTot * r;
    const y = padT + plotH - r * plotH;
    return `
      <line x1="${padL}" y1="${y.toFixed(1)}" x2="${W - padR}" y2="${y.toFixed(1)}" stroke="#e5e7eb" stroke-dasharray="${r === 0 ? "" : "3 3"}" />
      <text x="${padL - 8}" y="${(y + 4).toFixed(1)}" text-anchor="end" font-size="10" fill="#6b7280">${esc(fmtTokens(val))}</text>`;
  });

  const bars = tokenMix.map((d, i) => {
    const cx = padL + i * step + step / 2;
    const x = cx - barW / 2;
    let curY = padT + plotH;
    const dayTotal = d.freshInput + d.output + d.cacheCreation + d.cacheRead;
    const segs: [number, string][] = [
      [d.cacheRead, "#10b981"],
      [d.cacheCreation, "#f59e0b"],
      [d.freshInput, "#06b6d4"],
      [d.output, "#8b5cf6"],
    ];
    const rects = segs
      .map(([v, color]) => {
        if (v <= 0) return "";
        const h = (v / maxTot) * plotH;
        curY -= h;
        return `<rect x="${x.toFixed(1)}" y="${curY.toFixed(1)}" width="${barW.toFixed(1)}" height="${Math.max(0.8, h).toFixed(1)}" fill="${color}" />`;
      })
      .join("");
    const xLabel =
      i % labelStep === 0 || i === tokenMix.length - 1
        ? `<text x="${cx.toFixed(1)}" y="${H - 8}" text-anchor="middle" font-size="10" fill="#6b7280">${esc(d.date.slice(5))}</text>`
        : "";
    const tip = tipAttrs(d.date, [
      `总消耗：${fmtTokens(dayTotal)}`,
      `缓存读取：${fmtTokens(d.cacheRead)}`,
      `缓存写入：${fmtTokens(d.cacheCreation)}`,
      `新输入：${fmtTokens(d.freshInput)}`,
      `模型输出：${fmtTokens(d.output)}`,
    ]);
    return `${rects}<rect class="hover-band" x="${(padL + i * step).toFixed(1)}" y="${padT}" width="${step.toFixed(1)}" height="${plotH}" ${tip}><title>${esc(d.date)}: ${esc(fmtTokens(dayTotal))}</title></rect>${xLabel}`;
  });

  return `<svg viewBox="0 0 ${W} ${H}" class="svg-chart" role="img" aria-label="每日 Token 结构">
    ${grid.join("")}
    ${bars.join("")}
  </svg>`;
}

function buildHeatmapSvg(rhythm: ReportRhythmCell[]): string {
  const map = new Map<string, ReportRhythmCell>();
  let maxCost = 0;
  for (const c of rhythm) {
    map.set(`${c.weekday}-${c.hour}`, c);
    if (c.costUsd > maxCost) maxCost = c.costUsd;
  }
  const cellW = 24;
  const cellH = 18;
  const gap = 3;
  const padL = 36;
  const padT = 20;
  const W = padL + 24 * (cellW + gap) + 6;
  const H = padT + 7 * (cellH + gap) + 8;

  const hours = Array.from({ length: 24 }, (_, h) =>
    h % 3 === 0
      ? `<text x="${padL + h * (cellW + gap) + cellW / 2}" y="13" text-anchor="middle" font-size="10" fill="#6b7280">${String(h).padStart(2, "0")}</text>`
      : "",
  ).join("");

  const rows: string[] = [];
  for (let d = 0; d < 7; d++) {
    const y = padT + d * (cellH + gap);
    rows.push(
      `<text x="${padL - 8}" y="${y + 13}" text-anchor="end" font-size="10" fill="#6b7280">${WEEKDAYS[d]}</text>`,
    );
    for (let h = 0; h < 24; h++) {
      const x = padL + h * (cellW + gap);
      const cell = map.get(`${d}-${h}`);
      const ratio = cell && maxCost > 0 ? cell.costUsd / maxCost : 0;
      const fill =
        ratio <= 0
          ? "#f3f4f6"
          : ratio < 0.25
            ? "#ddd6fe"
            : ratio < 0.55
              ? "#a78bfa"
              : ratio < 0.8
                ? "#7c3aed"
                : "#5b21b6";
      const slotLabel = `${WEEKDAYS[d]} ${String(h).padStart(2, "0")}:00–${String(h).padStart(2, "0")}:59`;
      const speedVal =
        cell && cell.speedDurationMs > 0
          ? (cell.speedOutputTokens * 1000) / cell.speedDurationMs
          : 0;
      const tip = tipAttrs(
        slotLabel,
        cell
          ? [
              `费用：${fmtUsd(cell.costUsd)}`,
              `请求：${cell.requests} 次`,
              ...(speedVal > 0
                ? [`输出速度：${speedVal.toFixed(1)} tok/s`]
                : []),
            ]
          : ["无调用记录"],
      );
      const title = cell
        ? `${slotLabel} · ${fmtUsd(cell.costUsd)} · ${cell.requests} 次`
        : `${slotLabel} · 无记录`;
      rows.push(
        `<rect class="hover-cell" x="${x}" y="${y}" width="${cellW}" height="${cellH}" rx="4" fill="${fill}" ${tip}><title>${esc(title)}</title></rect>`,
      );
    }
  }

  return `<svg viewBox="0 0 ${W} ${H}" class="svg-chart" role="img" aria-label="7x24 编程节律热力图">
    ${hours}
    ${rows.join("")}
  </svg>`;
}

function buildStabilitySvg(stability: ReportModelStability[]): string {
  const speedRows = stability
    .filter((s) => s.speedP50 != null && s.speedP10 != null)
    .slice(0, 6);
  const ttftRows = stability
    .filter((s) => s.ttftP50Ms != null && s.ttftP95Ms != null)
    .slice(0, 6);
  const useSpeed = speedRows.length >= ttftRows.length && speedRows.length > 0;
  const rows = useSpeed ? speedRows : ttftRows;
  if (rows.length === 0) {
    return `<div class="empty">当前范围暂无足够延迟或速度样本</div>`;
  }

  const maxVal = useSpeed
    ? Math.max(
        10,
        ...rows.map((r) => Math.max(r.speedP10 ?? 0, r.speedP50 ?? 0)),
      )
    : Math.max(
        500,
        ...rows.map((r) => Math.max(r.ttftP50Ms ?? 0, r.ttftP95Ms ?? 0)),
      );

  const items = rows
    .map((r) => {
      const a = useSpeed ? (r.speedP10 ?? 0) : (r.ttftP50Ms ?? 0);
      const b = useSpeed ? (r.speedP50 ?? 0) : (r.ttftP95Ms ?? 0);
      const leftPct = (Math.min(a, b) / maxVal) * 100;
      const rightPct = (Math.max(a, b) / maxVal) * 100;
      const summary = useSpeed
        ? `P10 ${a.toFixed(1)} · P50 ${b.toFixed(1)} tok/s`
        : `P50 ${(a / 1000).toFixed(2)}s · P95 ${(b / 1000).toFixed(2)}s`;
      const tipLines = [`样本数：${r.requests} 次`];
      if (r.speedP10 != null && r.speedP50 != null) {
        tipLines.push(
          `输出速度：P10 ${r.speedP10.toFixed(1)} · P50 ${r.speedP50.toFixed(1)} tok/s${r.speedEstimated ? "（估算）" : ""}`,
        );
      }
      if (r.ttftP50Ms != null && r.ttftP95Ms != null) {
        tipLines.push(
          `首字延迟：P50 ${(r.ttftP50Ms / 1000).toFixed(2)}s · P95 ${(r.ttftP95Ms / 1000).toFixed(2)}s${r.ttftEstimated ? "（估算）" : ""}`,
        );
      }
      return `
        <div class="stab-row hover-row" ${tipAttrs(r.model, tipLines)}>
          <div class="stab-head">
            <span class="mono truncate" title="${esc(r.model)}">${esc(r.model)}</span>
            <span class="muted">${esc(summary)}</span>
          </div>
          <div class="stab-track">
            <div class="stab-range" style="left:${leftPct.toFixed(1)}%;width:${Math.max(2, rightPct - leftPct).toFixed(1)}%"></div>
            <div class="stab-dot a" style="left:${((a / maxVal) * 100).toFixed(1)}%"></div>
            <div class="stab-dot b" style="left:${((b / maxVal) * 100).toFixed(1)}%"></div>
          </div>
        </div>`;
    })
    .join("");

  const subtitle = useSpeed
    ? "输出速度区间（P10 最慢 ~ P50 中位，tok/s）"
    : "首字延迟区间（P50 中位 ~ P95 尾部，秒）";
  return `<div class="sub-caption">${subtitle}</div><div class="stab-list">${items}</div>`;
}

export function buildHtmlReport(opts: ReportOptions): string {
  const { data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget } =
    opts;
  const cs = data.cacheSavings;
  const fc = data.forecast;
  const st = data.status;
  const tok = sumTokenMix(data.tokenMix);
  const fullCost = cs.actualCostUsd + cs.savedUsd;
  const savedPct =
    fullCost > 0 ? Math.round((cs.savedUsd / fullCost) * 100) : 0;
  const okRate = st.total > 0 ? ((st.ok2xx / st.total) * 100).toFixed(2) : "—";
  const nowStr = new Date().toLocaleString("zh-CN", { hour12: false });

  let peakCell: ReportRhythmCell | null = null;
  for (const c of data.rhythm) {
    if (!peakCell || c.costUsd > peakCell.costUsd) peakCell = c;
  }

  const budgetPct = budget > 0 ? (fc.projected30dUsd / budget) * 100 : 0;
  const overBudget = fc.projected30dUsd > budget;
  const topProject = data.projects[0];

  const summaryParts: string[] = [
    `本统计周期共发起 <strong>${cs.totalRequests.toLocaleString()} 次</strong> AI 请求，总消耗 <strong>${esc(fmtTokens(tok.total))} Token</strong>（新输入 ${esc(fmtTokens(tok.freshInput))} / 输出 ${esc(fmtTokens(tok.output))} / 缓存读 ${esc(fmtTokens(tok.cacheRead))}），预估实付 <strong>${esc(fmtUsd(cs.actualCostUsd))}</strong>。`,
    cs.savedUsd > 0
      ? `Prompt 缓存命中率达 <strong>${(cs.cacheHitRate * 100).toFixed(1)}%</strong>，共省下 <strong>${esc(fmtUsd(cs.savedUsd))}</strong>（约 ${savedPct}%）。`
      : `当前统计范围暂无显著缓存节省记录。`,
    `有效日均 <strong>${esc(fmtUsd(fc.dailyAvgUsd))}/天</strong>，按当前节奏 30 天预测 <strong>${esc(fmtUsd(fc.projected30dUsd))}</strong>（月预算 ${esc(fmtUsd(budget))}，${overBudget ? "预计超支" : "处于预算内"}）。`,
  ];
  if (peakCell) {
    summaryParts.push(
      `消费高峰集中在 <strong>${WEEKDAYS[peakCell.weekday]} ${String(peakCell.hour).padStart(2, "0")}:00</strong>（${esc(fmtUsd(peakCell.costUsd))}）。`,
    );
  }
  if (topProject) {
    summaryParts.push(
      `花费最高的项目是 <strong>${esc(topProject.projectName)}</strong>（${esc(fmtUsd(topProject.costUsd))}，${topProject.sessions} 个会话）。`,
    );
  }

  const projectMax = Math.max(0.01, ...data.projects.map((p) => p.costUsd));
  const projectTotal = data.projects.reduce((acc, p) => acc + p.costUsd, 0);
  const projectsHtml =
    data.projects.length === 0
      ? `<div class="empty">当前范围暂无可归因的本地项目目录</div>`
      : data.projects
          .slice(0, 10)
          .map((p) => {
            const share =
              projectTotal > 0 ? (p.costUsd / projectTotal) * 100 : 0;
            const tip = tipAttrs(p.projectName, [
              `路径：${p.projectDir}`,
              `费用：${fmtUsd(p.costUsd)}（占比 ${share.toFixed(1)}%）`,
              `规模：${p.sessions} 会话 · ${p.requests} 次请求 · ${fmtTokens(p.totalTokens)} Token`,
            ]);
            return `
            <div class="proj-row hover-row" ${tip}>
              <div class="proj-head">
                <div>
                  <strong>${esc(p.projectName)}</strong>
                  <span class="proj-path">${esc(p.projectDir)}</span>
                </div>
                <div class="proj-nums">
                  <span class="muted">${p.sessions} 会话 · ${p.requests} 次 · ${esc(fmtTokens(p.totalTokens))}</span>
                  <strong>${esc(fmtUsd(p.costUsd))}</strong>
                  <span class="pill">${share.toFixed(1)}%</span>
                </div>
              </div>
              <div class="bar-track"><div class="bar-fill violet" style="width:${Math.max(2, (p.costUsd / projectMax) * 100).toFixed(1)}%"></div></div>
            </div>`;
          })
          .join("");

  const subsHtml =
    data.subscriptions.length === 0
      ? ""
      : `<section class="card">
          <div class="card-head">
            <div>
              <h2>订阅回本仪</h2>
              <p class="sub">对比订阅账单价与等效 API 牌价（非美元币种保留原币种，不做估算汇率换算）</p>
            </div>
            <span class="pill">${data.subscriptions.length} 个生效订阅</span>
          </div>
          <div class="roi-grid">
            ${data.subscriptions
              .map((sub) => {
                const roi = sub.usdRoiMultiple;
                const priceStr = formatSubscriptionPrice(
                  sub.price,
                  sub.currency,
                  sub.billing,
                  sub.customIntervalDays,
                );
                const pct =
                  roi != null
                    ? Math.min(100, Math.max(4, (roi / Math.max(roi, 2)) * 100))
                    : 0;
                const marker =
                  roi != null ? Math.min(95, (1 / Math.max(roi, 2)) * 100) : 0;
                return `
                <div class="roi-item">
                  <div class="roi-top">
                    <strong>${esc(sub.name)}</strong>
                    <span class="badge ${roi != null ? (roi >= 1 ? "good" : "warn") : "neutral"}">${roi != null ? `${roi.toFixed(1)}× ${roi >= 1 ? "已回本" : "未回本"}` : esc(priceStr)}</span>
                  </div>
                  ${roi != null ? `<div class="roi-track"><div class="roi-fill" style="width:${pct.toFixed(1)}%"></div><div class="roi-mark" style="left:${marker.toFixed(1)}%"></div></div>` : ""}
                  <div class="roi-meta">
                    <span>本范围等效 <strong>${esc(fmtUsd(sub.rangeApiCostUsd))}</strong></span>
                    <span>本周期 <strong>${esc(fmtUsd(sub.cycleApiCostUsd))}</strong> (${sub.cycleRequests} 次)</span>
                  </div>
                </div>`;
              })
              .join("")}
          </div>
        </section>`;

  const statusItems = [
    { label: "2xx 成功", count: st.ok2xx, color: "#10b981" },
    { label: "429 限流", count: st.rateLimit429, color: "#f59e0b" },
    { label: "5xx 服务端错误", count: st.server5xx, color: "#ef4444" },
    { label: "其他异常", count: st.otherError, color: "#9ca3af" },
  ];
  const stTotal = Math.max(1, st.total);
  const stDiag =
    st.rateLimit429 > 0
      ? `检测到 ${st.rateLimit429} 次 429 限流${st.peak429Hour != null ? `（高发于 ${String(st.peak429Hour).padStart(2, "0")}:00 左右）` : ""}，整体成功率 ${okRate}%。`
      : st.server5xx > 0
        ? `出现 ${st.server5xx} 次 5xx 服务端错误，整体成功率 ${okRate}%。`
        : `整体成功率 ${okRate}%，当前范围无 429 限流或 5xx 异常。`;

  const sessionsHtml =
    data.sessions.length === 0
      ? `<div class="empty">当前范围暂无会话明细</div>`
      : `<table>
          <thead>
            <tr>
              <th>会话</th>
              <th>项目</th>
              <th>平台</th>
              <th class="num">轮次</th>
              <th class="num">平均上下文</th>
              <th class="num">总 Token</th>
              <th class="num">费用</th>
            </tr>
          </thead>
          <tbody>
            ${data.sessions
              .slice(0, 12)
              .map((s) => {
                const title = s.title || s.sessionId.slice(0, 16);
                const proj = projectBasename(s.projectDir) || "—";
                const appLabel = sourceName.get(s.appType) ?? s.appType;
                return `<tr>
                  <td><div class="sess-title">${esc(title)}</div><div class="sess-id mono">${esc(s.sessionId.slice(0, 18))}</div></td>
                  <td><span class="pill">${esc(proj)}</span></td>
                  <td>${esc(appLabel)}</td>
                  <td class="num">${s.turns}</td>
                  <td class="num">${esc(fmtTokens(s.avgContextTokens))}</td>
                  <td class="num">${esc(fmtTokens(s.totalTokens))}</td>
                  <td class="num"><strong>${esc(fmtUsd(s.costUsd))}</strong></td>
                </tr>`;
              })
              .join("")}
          </tbody>
        </table>`;

  return `<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<title>Observatory AI 编程复盘报告（${esc(rangeLabel)}）</title>
<style>
  :root {
    --bg: #f8fafc;
    --card: #ffffff;
    --ink: #0f172a;
    --muted: #64748b;
    --line: #e2e8f0;
    --violet: #7c3aed;
    --cyan: #0891b2;
    --green: #10b981;
    --amber: #f59e0b;
    --red: #ef4444;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    padding: 28px 20px 44px;
    background: var(--bg);
    color: var(--ink);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
    font-size: 13px;
    line-height: 1.55;
    -webkit-print-color-adjust: exact;
    print-color-adjust: exact;
  }
  .wrap { max-width: 980px; margin: 0 auto; display: flex; flex-direction: column; gap: 16px; }
  .hero {
    background: linear-gradient(135deg, #1e1b4b 0%, #312e81 52%, #0f172a 100%);
    color: #fff;
    border-radius: 16px;
    padding: 24px 26px;
    box-shadow: 0 10px 28px rgba(30, 27, 75, 0.16);
  }
  .hero-top { display: flex; justify-content: space-between; align-items: flex-start; gap: 12px; flex-wrap: wrap; }
  .brand { font-size: 11px; letter-spacing: 0.1em; text-transform: uppercase; color: #a5b4fc; font-weight: 700; }
  .hero h1 { margin: 6px 0 4px; font-size: 22px; font-weight: 700; letter-spacing: -0.01em; }
  .hero-meta { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 8px; }
  .hero-pill {
    display: inline-flex; align-items: center; padding: 3px 10px; border-radius: 999px;
    background: rgba(255,255,255,0.12); color: #e0e7ff; font-size: 11.5px;
  }
  .summary-box {
    margin-top: 16px; padding: 12px 14px; border-radius: 10px;
    background: rgba(255,255,255,0.09); border: 1px solid rgba(255,255,255,0.14);
    color: #f1f5f9; font-size: 13px; line-height: 1.65;
  }
  .summary-box strong { color: #ffffff; }
  .kpi-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
  .two-col { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
  .card {
    background: var(--card); border: 1px solid var(--line); border-radius: 14px;
    padding: 16px 18px; break-inside: avoid;
  }
  .card-head { display: flex; justify-content: space-between; align-items: flex-start; gap: 10px; margin-bottom: 12px; }
  .card h2 { margin: 0; font-size: 15px; font-weight: 700; }
  .sub { margin: 3px 0 0; color: var(--muted); font-size: 11.5px; }
  .kpi-label { font-size: 11.5px; color: var(--muted); }
  .kpi-val { font-size: 23px; font-weight: 700; margin: 6px 0 6px; letter-spacing: -0.02em; font-variant-numeric: tabular-nums; }
  .kpi-foot { font-size: 11.5px; color: var(--muted); display: flex; justify-content: space-between; gap: 6px; flex-wrap: wrap; }
  .badge {
    display: inline-flex; align-items: center; padding: 2px 8px; border-radius: 999px;
    font-size: 11px; font-weight: 600; white-space: nowrap;
  }
  .badge.good { background: #dcfce7; color: #166534; }
  .badge.warn { background: #fef3c7; color: #92400e; }
  .badge.danger { background: #fee2e2; color: #991b1b; }
  .badge.neutral { background: #f1f5f9; color: #475569; }
  .pill {
    display: inline-flex; padding: 2px 8px; border-radius: 999px;
    background: #f1f5f9; color: #475569; font-size: 11px; font-weight: 500;
  }
  .bar-track {
    height: 8px; border-radius: 999px; background: #f1f5f9; overflow: hidden;
    display: flex; gap: 1px; margin: 8px 0;
  }
  .bar-fill.violet { background: var(--violet); }
  .bar-fill.green { background: var(--green); }
  .bar-fill.cyan { background: var(--cyan); }
  .bar-fill.red { background: var(--red); }
  .svg-chart { width: 100%; height: auto; display: block; }
  .legend { display: flex; gap: 14px; flex-wrap: wrap; font-size: 11.5px; color: var(--muted); margin-top: 6px; }
  .dot { display: inline-block; width: 8px; height: 8px; border-radius: 999px; margin-right: 5px; vertical-align: middle; }
  .proj-row { padding: 8px 0; border-bottom: 1px solid #f1f5f9; }
  .proj-row:last-child { border-bottom: 0; }
  .proj-head { display: flex; justify-content: space-between; align-items: baseline; gap: 10px; flex-wrap: wrap; }
  .proj-path { font-size: 11px; color: var(--muted); margin-left: 8px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .proj-nums { display: flex; align-items: center; gap: 8px; font-variant-numeric: tabular-nums; }
  .roi-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(230px, 1fr)); gap: 10px; }
  .roi-item { padding: 12px; border-radius: 10px; background: #f8fafc; border: 1px solid var(--line); }
  .roi-top { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
  .roi-track { position: relative; height: 8px; border-radius: 999px; background: #e2e8f0; margin: 10px 0 8px; }
  .roi-fill { height: 100%; border-radius: 999px; background: linear-gradient(90deg, #7c3aed, #10b981); }
  .roi-mark { position: absolute; top: -3px; bottom: -3px; width: 2px; background: #0f172a; }
  .roi-meta { display: flex; justify-content: space-between; font-size: 11.5px; color: var(--muted); }
  .sub-caption { font-size: 11.5px; color: var(--muted); margin-bottom: 10px; }
  .stab-list { display: flex; flex-direction: column; gap: 10px; }
  .stab-head { display: flex; justify-content: space-between; gap: 8px; font-size: 12px; margin-bottom: 4px; }
  .stab-track { position: relative; height: 10px; background: #f1f5f9; border-radius: 999px; }
  .stab-range { position: absolute; top: 3px; height: 4px; border-radius: 999px; background: #c4b5fd; }
  .stab-dot { position: absolute; top: 1px; width: 8px; height: 8px; border-radius: 999px; transform: translateX(-50%); }
  .stab-dot.a { background: #0891b2; }
  .stab-dot.b { background: #7c3aed; }
  .status-list { display: flex; flex-direction: column; gap: 6px; margin-top: 10px; }
  .status-item { display: flex; justify-content: space-between; font-size: 12px; }
  .diag { margin-top: 12px; padding: 10px 12px; border-radius: 8px; background: #f8fafc; border: 1px solid var(--line); font-size: 12px; color: #334155; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  th { text-align: left; padding: 8px 8px; border-bottom: 1px solid var(--line); color: var(--muted); font-weight: 600; font-size: 11.5px; }
  td { padding: 8px 8px; border-bottom: 1px solid #f1f5f9; vertical-align: middle; }
  tr:last-child td { border-bottom: 0; }
  .num { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .sess-title { font-weight: 600; color: var(--ink); max-width: 300px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sess-id { font-size: 10.5px; color: var(--muted); }
  .mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .truncate { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 190px; }
  .muted { color: var(--muted); }
  .empty { padding: 24px; text-align: center; color: var(--muted); font-size: 12.5px; background: #f8fafc; border-radius: 10px; }
  footer { text-align: center; font-size: 11.5px; color: var(--muted); margin-top: 8px; }
  .hover-band { fill: transparent; cursor: crosshair; transition: fill 0.1s ease; }
  .hover-band:hover { fill: rgba(124, 58, 237, 0.08); }
  .hover-cell { cursor: crosshair; transition: opacity 0.1s ease; }
  .hover-cell:hover { stroke: #1e1b4b; stroke-width: 1.4; }
  .hover-row { border-radius: 8px; transition: background 0.12s ease; cursor: default; }
  .hover-row:hover { background: #f8fafc; }
  #obs-tip {
    position: fixed; z-index: 9999; pointer-events: none; display: none;
    max-width: 300px; padding: 8px 11px; border-radius: 10px;
    background: rgba(15, 23, 42, 0.94); color: #f8fafc;
    box-shadow: 0 10px 26px rgba(15, 23, 42, 0.28);
    font-size: 11.5px; line-height: 1.5; backdrop-filter: blur(6px);
  }
  #obs-tip .t-title { font-weight: 700; color: #ffffff; margin-bottom: 3px; word-break: break-word; }
  #obs-tip .t-line { color: #cbd5e1; white-space: pre-wrap; word-break: break-word; }
  @media (max-width: 760px) {
    .kpi-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .two-col { grid-template-columns: 1fr; }
  }
  @media print {
    body { background: #fff; padding: 0; }
    .card, .hero { box-shadow: none; }
    #obs-tip { display: none !important; }
  }
</style>
</head>
<body>
  <div class="wrap">
    <header class="hero">
      <div class="hero-top">
        <div>
          <div class="brand">Observatory · AI Usage Report</div>
          <h1>AI 编程用量与效能复盘报告（${esc(rangeLabel)}）</h1>
        </div>
        <div class="hero-meta">
          <span class="hero-pill">范围：${esc(scopeLabel)}</span>
          <span class="hero-pill">生成时间：${esc(nowStr)}</span>
        </div>
      </div>
      <div class="summary-box">${summaryParts.join(" ")}</div>
    </header>

    <section class="kpi-grid">
      <div class="card">
        <div class="kpi-label">预估实付费用</div>
        <div class="kpi-val" style="color:var(--violet)">${esc(fmtUsd(cs.actualCostUsd))}</div>
        <div class="kpi-foot">
          <span>今日 ${esc(fmtUsd(fc.todayCostUsd))}</span>
          ${fc.deltaPct != null ? `<span class="badge ${fc.deltaPct > 15 ? "warn" : "good"}">较上期 ${fc.deltaPct >= 0 ? "+" : ""}${fc.deltaPct.toFixed(1)}%</span>` : ""}
        </div>
      </div>
      <div class="card">
        <div class="kpi-label">Prompt 缓存省下</div>
        <div class="kpi-val" style="color:var(--green)">${esc(fmtUsd(cs.savedUsd))}</div>
        <div class="bar-track">
          <div class="bar-fill violet" style="width:${fullCost > 0 ? ((cs.actualCostUsd / fullCost) * 100).toFixed(1) : "0"}%"></div>
          <div class="bar-fill green" style="width:${fullCost > 0 ? ((cs.savedUsd / fullCost) * 100).toFixed(1) : "0"}%"></div>
        </div>
        <div class="kpi-foot">
          <span>命中率 ${(cs.cacheHitRate * 100).toFixed(1)}%</span>
          <span>省 ${savedPct}%</span>
        </div>
      </div>
      <div class="card">
        <div class="kpi-label">总 Token 消耗</div>
        <div class="kpi-val" style="color:var(--cyan)">${esc(fmtTokens(tok.total))}</div>
        <div class="bar-track">
          <div style="width:${tok.total > 0 ? ((tok.cacheRead / tok.total) * 100).toFixed(1) : "0"}%;background:#10b981"></div>
          <div style="width:${tok.total > 0 ? ((tok.cacheCreation / tok.total) * 100).toFixed(1) : "0"}%;background:#f59e0b"></div>
          <div style="width:${tok.total > 0 ? ((tok.freshInput / tok.total) * 100).toFixed(1) : "0"}%;background:#06b6d4"></div>
          <div style="width:${tok.total > 0 ? ((tok.output / tok.total) * 100).toFixed(1) : "0"}%;background:#8b5cf6"></div>
        </div>
        <div class="kpi-foot">
          <span>${cs.totalRequests.toLocaleString()} 次请求</span>
          <span>均上下文 ${esc(fmtTokens(cs.avgContextTokens))}</span>
        </div>
      </div>
      <div class="card">
        <div class="kpi-label">30 天预测 / 月预算 ${esc(fmtUsd(budget))}</div>
        <div class="kpi-val" style="color:${overBudget ? "var(--red)" : "var(--ink)"}">${esc(fmtUsd(fc.projected30dUsd))}</div>
        <div class="bar-track">
          <div class="bar-fill ${overBudget ? "red" : "cyan"}" style="width:${Math.min(100, budgetPct).toFixed(1)}%"></div>
        </div>
        <div class="kpi-foot">
          <span>日均 ${esc(fmtUsd(fc.dailyAvgUsd))}</span>
          <span class="badge ${overBudget ? "danger" : "good"}">${overBudget ? "预计超预算" : `${budgetPct.toFixed(0)}%`}</span>
        </div>
      </div>
    </section>

    <section class="two-col">
      <div class="card">
        <div class="card-head">
          <div>
            <h2>每日费用趋势</h2>
            <p class="sub">按本地日期统计估算费用，虚线为有效日均与日预算参考</p>
          </div>
          <span class="pill">有效 ${Math.max(1, Math.round(fc.elapsedDays))} 天</span>
        </div>
        ${buildDailyCostSvg(fc.dailyCosts, fc.dailyAvgUsd, dailyBudget)}
      </div>
      <div class="card">
        <div class="card-head">
          <div>
            <h2>每日 Token 结构</h2>
            <p class="sub">拆解缓存读取、缓存写入、新输入与模型输出占比</p>
          </div>
          <span class="pill">总计 ${esc(fmtTokens(tok.total))}</span>
        </div>
        ${buildTokenMixSvg(data.tokenMix)}
        <div class="legend">
          <span><i class="dot" style="background:#10b981"></i>缓存读取 <strong>${esc(fmtTokens(tok.cacheRead))}</strong></span>
          <span><i class="dot" style="background:#f59e0b"></i>缓存写入 <strong>${esc(fmtTokens(tok.cacheCreation))}</strong></span>
          <span><i class="dot" style="background:#06b6d4"></i>新输入 <strong>${esc(fmtTokens(tok.freshInput))}</strong></span>
          <span><i class="dot" style="background:#8b5cf6"></i>模型输出 <strong>${esc(fmtTokens(tok.output))}</strong></span>
        </div>
      </div>
    </section>

    ${subsHtml}

    <section class="card">
      <div class="card-head">
        <div>
          <h2>7×24 编程节律热力图</h2>
          <p class="sub">按星期与小时透视消费强度，深紫色代表高消费时段</p>
        </div>
        ${peakCell ? `<span class="pill">峰值：${WEEKDAYS[peakCell.weekday]} ${String(peakCell.hour).padStart(2, "0")}:00 (${esc(fmtUsd(peakCell.costUsd))})</span>` : ""}
      </div>
      ${buildHeatmapSvg(data.rhythm)}
    </section>

    <section class="card">
      <div class="card-head">
        <div>
          <h2>项目 / 代码仓库费用排行</h2>
          <p class="sub">按本地会话工作目录自动归因</p>
        </div>
        <span class="pill">Top ${Math.min(10, data.projects.length)} 项目</span>
      </div>
      ${projectsHtml}
    </section>

    <section class="two-col">
      <div class="card">
        <div class="card-head">
          <div>
            <h2>模型响应稳定性</h2>
            <p class="sub">对比主力模型的分布区间</p>
          </div>
        </div>
        ${buildStabilitySvg(data.stability)}
      </div>
      <div class="card">
        <div class="card-head">
          <div>
            <h2>请求健康度与状态码</h2>
            <p class="sub">统计 2xx 成功、429 限流与 5xx 服务端异常</p>
          </div>
          <span class="badge ${st.rateLimit429 > 0 || st.server5xx > 0 ? "warn" : "good"}">成功率 ${okRate}%</span>
        </div>
        <div class="bar-track" style="height:10px">
          ${statusItems
            .filter((it) => it.count > 0)
            .map(
              (it) =>
                `<div style="width:${((it.count / stTotal) * 100).toFixed(2)}%;background:${it.color};cursor:pointer" ${tipAttrs(it.label, [`请求数：${it.count.toLocaleString()} 次`, `占比：${((it.count / stTotal) * 100).toFixed(2)}%`])}></div>`,
            )
            .join("")}
        </div>
        <div class="status-list">
          ${statusItems
            .map(
              (it) => `<div class="status-item">
                <span><i class="dot" style="background:${it.color}"></i>${it.label}</span>
                <span><strong>${it.count.toLocaleString()}</strong> <span class="muted">(${((it.count / stTotal) * 100).toFixed(1)}%)</span></span>
              </div>`,
            )
            .join("")}
        </div>
        <div class="diag"><strong>自动诊断：</strong>${esc(stDiag)}</div>
      </div>
    </section>

    <section class="card">
      <div class="card-head">
        <div>
          <h2>最贵会话 Top ${Math.min(12, data.sessions.length)}</h2>
          <p class="sub">按费用降序排列，便于复盘高消耗长对话</p>
        </div>
      </div>
      ${sessionsHtml}
    </section>

    <footer>由 Observatory（观测台）本地离线生成 · 仅统计本地元数据，不包含对话正文 · 费用为基于本地价格表的估算值</footer>
  </div>
  <div id="obs-tip" role="tooltip"></div>
  <script>
    (function () {
      var tip = document.getElementById("obs-tip");
      if (!tip) return;
      var active = null;
      function show(el, x, y) {
        if (active !== el) {
          active = el;
          var title = el.getAttribute("data-tip-t") || "";
          var body = el.getAttribute("data-tip-b") || "";
          tip.textContent = "";
          if (title) {
            var h = document.createElement("div");
            h.className = "t-title";
            h.textContent = title;
            tip.appendChild(h);
          }
          if (body) {
            body.split("\\n").forEach(function (line) {
              var d = document.createElement("div");
              d.className = "t-line";
              d.textContent = line;
              tip.appendChild(d);
            });
          }
          tip.style.display = "block";
        }
        var w = tip.offsetWidth || 220;
        var h = tip.offsetHeight || 64;
        var left = x + 14 + w > window.innerWidth - 8 ? Math.max(8, x - w - 14) : x + 14;
        var top = y + 14 + h > window.innerHeight - 8 ? Math.max(8, y - h - 14) : y + 14;
        tip.style.left = left + "px";
        tip.style.top = top + "px";
      }
      function hide() {
        active = null;
        tip.style.display = "none";
      }
      document.addEventListener("pointermove", function (e) {
        var el = e.target && e.target.closest ? e.target.closest("[data-tip-t]") : null;
        if (el) show(el, e.clientX, e.clientY);
        else if (active) hide();
      });
      document.addEventListener("pointerleave", hide);
      window.addEventListener("scroll", hide, true);
    })();
  </script>
</body>
</html>`;
}
