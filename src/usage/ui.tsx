import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";

// Sparklines, detail tables, stats tables and detail views all call this on a
// hot path: constructing an Intl formatter is the most expensive object
// allocation there, so cache them by precision.
const MONEY_FORMATS = new Map<number, Intl.NumberFormat>();
const moneyFormatter = (digits: number) => {
  let fmt = MONEY_FORMATS.get(digits);
  if (!fmt) {
    fmt = new Intl.NumberFormat("en-US", {
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    });
    MONEY_FORMATS.set(digits, fmt);
  }
  return fmt;
};
const TIME_LABEL_FORMAT = new Intl.DateTimeFormat("zh-CN", { hour12: false });

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
  "$" + moneyFormatter(digits).format(Number(value || 0));
export const friendlyError = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
export const timeLabel = (seconds?: number) =>
  seconds ? TIME_LABEL_FORMAT.format(new Date(seconds * 1000)) : "尚未同步";
