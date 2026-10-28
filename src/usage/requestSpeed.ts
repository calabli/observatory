type SpeedInput = {
  outputTokens: number;
  latencyMs: number;
  firstTokenMs?: number | null;
  dataSource?: string | null;
};

/** 与 CC Switch 用量统计的速度门槛和来源口径一致。 */
export function requestSpeed(
  log: SpeedInput,
): { value: number; estimated: boolean } | null {
  const { outputTokens, latencyMs, firstTokenMs } = log;
  if (!Number.isFinite(outputTokens) || !Number.isFinite(latencyMs))
    return null;
  const estimated = !!log.dataSource && log.dataSource !== "proxy";
  if (estimated) {
    if (outputTokens < 200 || latencyMs < 1000) return null;
    return { value: (outputTokens * 1000) / latencyMs, estimated: true };
  }
  if (
    firstTokenMs == null ||
    !Number.isFinite(firstTokenMs) ||
    firstTokenMs < 0
  )
    return null;
  const generationMs = latencyMs - firstTokenMs;
  if (outputTokens < 100 || generationMs < 100) return null;
  return { value: (outputTokens * 1000) / generationMs, estimated: false };
}

export function formatRequestSpeed(log: SpeedInput): string {
  const result = requestSpeed(log);
  return result
    ? `${result.estimated ? "≈ " : ""}${result.value.toFixed(1)} Token/s`
    : "—";
}
