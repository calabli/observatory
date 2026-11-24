export interface TreemapNode<T> {
  id: string;
  weight: number;
  data: T;
}

export interface TreemapRect<T> {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  data: T;
}

function worstAspectRatio(row: number[], shortSide: number): number {
  if (row.length === 0 || shortSide <= 0) return Infinity;
  let sum = 0;
  let max = 0;
  let min = Infinity;
  for (const v of row) {
    sum += v;
    if (v > max) max = v;
    if (v < min) min = v;
  }
  if (sum <= 0 || min <= 0) return Infinity;
  const s2 = sum * sum;
  const w2 = shortSide * shortSide;
  return Math.max((w2 * max) / s2, s2 / (w2 * min));
}

/**
 * Pure Squarified Treemap layout (Bruls et al., 2000).
 * Returns rects in the same coordinate space as `(x, y, width, height)`.
 */
export function layoutSquarified<T>(
  nodes: TreemapNode<T>[],
  x: number,
  y: number,
  width: number,
  height: number,
): TreemapRect<T>[] {
  if (nodes.length === 0 || width <= 0 || height <= 0) return [];

  const sorted = [...nodes]
    .map((n) => ({ ...n, weight: Math.max(0.0001, n.weight) }))
    .sort((a, b) => b.weight - a.weight);

  const totalWeight = sorted.reduce((acc, n) => acc + n.weight, 0);
  const totalArea = width * height;
  const areas = sorted.map((n) => (n.weight / totalWeight) * totalArea);

  const out: TreemapRect<T>[] = [];
  let rx = x;
  let ry = y;
  let rw = width;
  let rh = height;
  let idx = 0;

  while (idx < sorted.length) {
    const horizontal = rw >= rh;
    const shortSide = Math.max(1e-6, horizontal ? rh : rw);
    const rowAreas: number[] = [];
    const rowIndices: number[] = [];

    while (idx < sorted.length) {
      const nextArea = areas[idx];
      if (rowAreas.length === 0) {
        rowAreas.push(nextArea);
        rowIndices.push(idx);
        idx++;
        continue;
      }
      const curWorst = worstAspectRatio(rowAreas, shortSide);
      const nextWorst = worstAspectRatio([...rowAreas, nextArea], shortSide);
      if (nextWorst <= curWorst) {
        rowAreas.push(nextArea);
        rowIndices.push(idx);
        idx++;
      } else {
        break;
      }
    }

    const rowSum = rowAreas.reduce((a, b) => a + b, 0);
    if (horizontal) {
      const stripW = Math.min(rw, rowSum / shortSide);
      let cy = ry;
      for (let i = 0; i < rowIndices.length; i++) {
        const item = sorted[rowIndices[i]];
        const ih =
          i === rowIndices.length - 1
            ? Math.max(0, ry + rh - cy)
            : rowSum > 0
              ? (rowAreas[i] / rowSum) * rh
              : 0;
        out.push({
          id: item.id,
          x: rx,
          y: cy,
          w: stripW,
          h: ih,
          data: item.data,
        });
        cy += ih;
      }
      rx += stripW;
      rw = Math.max(0, rw - stripW);
    } else {
      const stripH = Math.min(rh, rowSum / shortSide);
      let cx = rx;
      for (let i = 0; i < rowIndices.length; i++) {
        const item = sorted[rowIndices[i]];
        const iw =
          i === rowIndices.length - 1
            ? Math.max(0, rx + rw - cx)
            : rowSum > 0
              ? (rowAreas[i] / rowSum) * rw
              : 0;
        out.push({
          id: item.id,
          x: cx,
          y: ry,
          w: iw,
          h: stripH,
          data: item.data,
        });
        cx += iw;
      }
      ry += stripH;
      rh = Math.max(0, rh - stripH);
    }
  }

  return out;
}

/**
 * Finviz-style continuous heat color for percentage change, supporting
 * CN (`redUp`) and US (`greenUp`) color conventions.
 */
export function heatTileColor(
  changePercent: number | null | undefined,
  scheme: "cn" | "us",
): string {
  if (changePercent == null || !Number.isFinite(changePercent)) {
    return "#334155";
  }
  const abs = Math.abs(changePercent);
  if (abs < 0.05) return "#3f495a";

  const isRed =
    (scheme === "cn" && changePercent > 0) ||
    (scheme === "us" && changePercent < 0);

  if (isRed) {
    if (abs >= 4) return "#991b1b";
    if (abs >= 2.5) return "#b91c1c";
    if (abs >= 1.2) return "#dc2626";
    if (abs >= 0.4) return "#c24141";
    return "#8c4a4e";
  } else {
    if (abs >= 4) return "#166534";
    if (abs >= 2.5) return "#15803d";
    if (abs >= 1.2) return "#16a34a";
    if (abs >= 0.4) return "#2f8f5b";
    return "#3d6b56";
  }
}

/**
 * Color ramp for ATH drawdown (`fromAth`, <= 0%) or ATL rebound (`fromAtl`, >= 0%).
 * Respects CN (`redUp`) and US (`greenUp`) conventions.
 */
export function heatExtremeColor(
  pct: number | null | undefined,
  mode: "fromAth" | "fromAtl",
  scheme: "cn" | "us",
): string {
  if (pct == null || !Number.isFinite(pct)) return "#334155";
  if (mode === "fromAth") {
    const drop = Math.max(0, -pct);
    // Near ATH (< 5% off high): highlight in strong rise color
    if (drop < 5) {
      return scheme === "cn" ? "#b91c1c" : "#15803d";
    }
    if (drop < 12) return "#475569";
    // Deeper drawdown -> stronger fall color
    const fallRamp =
      scheme === "cn"
        ? ["#3d6b56", "#2f8f5b", "#16a34a", "#166534"]
        : ["#8c4a4e", "#c24141", "#dc2626", "#991b1b"];
    if (drop < 25) return fallRamp[0];
    if (drop < 40) return fallRamp[1];
    if (drop < 60) return fallRamp[2];
    return fallRamp[3];
  }
  // mode === "fromAtl": larger rebound -> stronger rise color (log-scaled thresholds)
  const rise = Math.max(0, pct);
  if (rise < 10) return "#3f495a";
  const riseRamp =
    scheme === "cn"
      ? ["#8c4a4e", "#c24141", "#dc2626", "#b91c1c", "#991b1b"]
      : ["#3d6b56", "#2f8f5b", "#16a34a", "#15803d", "#166534"];
  if (rise < 35) return riseRamp[0];
  if (rise < 100) return riseRamp[1];
  if (rise < 300) return riseRamp[2];
  if (rise < 1000) return riseRamp[3];
  return riseRamp[4];
}
