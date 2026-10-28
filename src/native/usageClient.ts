let session: Promise<string> | null = null;
declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
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
