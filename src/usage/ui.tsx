import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
export function UsageDialog({
  title,
  children,
  close,
  wide = false,
}: {
  title: string;
  children: ReactNode;
  close: () => void;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog
      ref={ref}
      className={"dialog usage-dialog " + (wide ? "wide" : "")}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
    >
      <div className="dialog-head">
        <h2>{title}</h2>
        <button className="icon-button" aria-label="关闭" onClick={close}>
          <X size={17} />
        </button>
      </div>
      {children}
    </dialog>
  );
}
export const formatTokens = (n: number) =>
  n >= 1e9
    ? (n / 1e9).toFixed(2) + "B"
    : n >= 1e6
      ? (n / 1e6).toFixed(2) + "M"
      : n >= 1000
        ? (n / 1000).toFixed(1) + "K"
        : String(n);
export const formatMoney = (value: string | number, digits = 2) =>
  "$" +
  Number(value || 0).toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
export const friendlyError = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
export const timeLabel = (seconds?: number) =>
  seconds
    ? new Date(seconds * 1000).toLocaleString("zh-CN", { hour12: false })
    : "尚未同步";
