let session: Promise<string> | null = null;
declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}
/** 结构化错误：保留稳定错误码与细节，供界面按类型分别处理。 */
export type UsageError = {
  code: string;
  message: string;
  details?: Record<string, unknown>;
};
const ENVELOPE = "observatoryError";
/**
 * 解开 Rust 侧的结构化错误信封。
 *
 * `usage_call` 的错误通道是纯字符串，所以带 `code` / `details` 的错误会被
 * 编码成 `{"observatoryError":{...}}` 传过来。普通错误消息按原样返回，
 * `code` 为空串。
 */
export function parseUsageError(error: unknown): UsageError {
  const raw = error instanceof Error ? error.message : String(error ?? "");
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && ENVELOPE in parsed) {
      const body = (parsed as Record<string, unknown>)[ENVELOPE];
      if (body && typeof body === "object") {
        const record = body as Record<string, unknown>;
        return {
          code: typeof record.code === "string" ? record.code : "",
          message: typeof record.error === "string" ? record.error : raw,
          details:
            record.details && typeof record.details === "object"
              ? (record.details as Record<string, unknown>)
              : undefined,
        };
      }
    }
  } catch {
    // 普通错误消息，不是结构化信封。
  }
  return { code: "", message: raw };
}
export async function usageCall<T>(op: string, args: unknown = {}): Promise<T> {
  if (window.__TAURI_INTERNALS__) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("usage_call", { operation: op, args });
  }
  session ??= fetch("/__observatory/session", {
    credentials: "same-origin",
    cache: "no-store",
  })
    .then(async (r) => {
      if (!r.ok)
        throw new Error("无法连接本地统计服务，请使用 npm run dev 启动应用");
      const v = await r.json();
      if (!v.token) throw new Error("本地统计会话无效");
      return v.token;
    })
    .catch((e) => {
      session = null;
      throw e;
    });
  const response = await fetch("/__observatory/usage", {
    method: "POST",
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      "X-Observatory-Token": await session,
    },
    body: JSON.stringify({ op, args }),
  });
  const data = await response
    .json()
    .catch(() => ({ error: "本地统计服务响应无效" }));
  if (!response.ok || data.error)
    throw new Error(data.error || `HTTP ${response.status}`);
  return data.result;
}
