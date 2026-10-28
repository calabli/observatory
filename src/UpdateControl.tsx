import { useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { createPortal } from "react-dom";

export function UpdateControl({
  target,
  onAvailable,
}: {
  target: HTMLElement | null;
  onAvailable: (available: boolean) => void;
}) {
  const [update, setUpdate] = useState<Update | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const running = useRef(false);
  const installing = useRef(false);
  const inspect = async (manual: boolean) => {
    if (!window.__TAURI_INTERNALS__ || installing.current) return;
    if (running.current) {
      if (manual) setMessage("正在检查更新，请稍候…");
      return;
    }
    running.current = true;
    setBusy(true);
    setMessage("正在检查更新，请稍候…");
    try {
      const result = await check({ timeout: 30000 });
      setUpdate(result);
      setConfirming(false);
      onAvailable(!!result);
      setMessage(
        result
          ? `发现新版本 ${result.version}`
          : manual
            ? "当前已是最新版本"
            : "",
      );
    } catch (error) {
      setMessage(`检查更新失败：${String(error)}`);
    } finally {
      running.current = false;
      setBusy(false);
    }
  };
  useEffect(() => {
    void inspect(false);
    const timer = setInterval(() => void inspect(false), 6 * 60 * 60 * 1000);
    return () => clearInterval(timer);
  }, []);
  if (!window.__TAURI_INTERNALS__ || !target) return null;
  return createPortal(
    <div className="update-control">
      <button
        className="button"
        disabled={busy}
        onClick={() => void inspect(true)}
      >
        检查更新
      </button>
      {message && <span role="status">{message}</span>}
      {update && (
        <button
          className="button primary"
          disabled={busy}
          onClick={() => setConfirming(true)}
        >
          安装更新
        </button>
      )}
      {update && confirming && (
        <div className="update-confirm">
          <span>安装 {update.version} 后应用将重新启动。</span>
          <button
            className="button"
            disabled={busy}
            onClick={() => setConfirming(false)}
          >
            取消
          </button>
          <button
            className="button primary"
            disabled={busy}
            onClick={async () => {
              installing.current = true;
              setBusy(true);
              setMessage("正在连接更新服务…");
              let received = 0;
              let total = 0;
              try {
                await update.downloadAndInstall(
                  (event) => {
                    if (event.event === "Started") {
                      total = event.data.contentLength ?? 0;
                      setMessage("正在下载更新…");
                    }
                    if (event.event === "Progress") {
                      received += event.data.chunkLength;
                      setMessage(
                        total > 0
                          ? `正在下载更新… ${Math.min(100, Math.round((received / total) * 100))}%`
                          : `正在下载更新… ${(received / 1048576).toFixed(1)} MB`,
                      );
                    }
                    if (event.event === "Finished")
                      setMessage("下载完成，正在安装更新…");
                  },
                  { timeout: 120000 },
                );
                setMessage("安装完成，正在重新启动…");
                await relaunch();
              } catch (error) {
                setMessage(`更新失败：${String(error)}`);
                installing.current = false;
                setBusy(false);
              }
            }}
          >
            确认安装
          </button>
        </div>
      )}
    </div>,
    target,
  );
}
