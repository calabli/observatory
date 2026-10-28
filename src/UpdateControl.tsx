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
  const running = useRef(false);
  const inspect = async (manual: boolean) => {
    if (!window.__TAURI_INTERNALS__ || running.current) return;
    running.current = true;
    if (manual) setBusy(true);
    try {
      const result = await check();
      setUpdate(result);
      onAvailable(!!result);
      setMessage(
        result
          ? `发现新版本 ${result.version}`
          : manual
            ? "当前已是最新版本"
            : "",
      );
    } catch {
      if (manual) setMessage("检查更新失败，请检查网络后重试");
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
          onClick={async () => {
            if (
              !window.confirm(
                `安装 ${update.version} 后应用将重新启动，是否继续？`,
              )
            )
              return;
            setBusy(true);
            setMessage("正在下载并安装更新…");
            try {
              await update.downloadAndInstall();
              await relaunch();
            } catch {
              setMessage("更新失败，请检查网络后重试");
              setBusy(false);
            }
          }}
        >
          安装更新
        </button>
      )}
    </div>,
    target,
  );
}
