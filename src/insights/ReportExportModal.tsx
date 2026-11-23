import { type InsightsData } from "./insightsShared";
import { useEffect, useMemo, useState } from "react";
import { buildHtmlReport, buildMarkdownReport } from "./reportBuilder";
import { Check, Code2, Copy, Download, Eye, FileText, X } from "lucide-react";

export function ReportExportModal({
  data,
  rangeLabel,
  scopeLabel,
  sourceName,
  budget,
  dailyBudget,
  onClose,
}: {
  data: InsightsData;
  rangeLabel: string;
  scopeLabel: string;
  sourceName: Map<string, string>;
  budget: number;
  dailyBudget: number;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<"html" | "markdown">("html");
  const [copied, setCopied] = useState(false);

  const reportOpts = useMemo(
    () => ({ data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget }),
    [data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget],
  );
  const markdown = useMemo(() => buildMarkdownReport(reportOpts), [reportOpts]);
  const html = useMemo(() => buildHtmlReport(reportOpts), [reportOpts]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const copyCurrent = () => {
    const content = mode === "html" ? html : markdown;
    void navigator.clipboard?.writeText(content).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  };

  const downloadFile = (content: string, mime: string, ext: "html" | "md") => {
    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    const stamp = new Date().toISOString().slice(0, 10);
    a.href = url;
    a.download = `observatory-report-${stamp}.${ext}`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div
      className="insights-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="预览与导出复盘报告"
      onClick={onClose}
    >
      <div
        className="insights-modal wide report-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="insights-modal-head">
          <div className="insights-modal-title-wrap">
            <div className="insights-modal-title">
              <FileText size={16} />
              <span>AI 编程复盘报告（{rangeLabel}）</span>
            </div>
            <div className="insights-modal-sub">
              <span>{scopeLabel}</span>
              <span>·</span>
              <span>
                离线单文件 HTML 含内嵌 SVG 图表，支持浏览器打印存为 PDF
              </span>
            </div>
          </div>
          <div className="insights-modal-actions">
            <div className="insights-pill-group subtle">
              <button
                type="button"
                className={`insights-pill-btn${mode === "html" ? " active" : ""}`}
                onClick={() => setMode("html")}
              >
                <Eye size={13} />
                <span>图文预览</span>
              </button>
              <button
                type="button"
                className={`insights-pill-btn${mode === "markdown" ? " active" : ""}`}
                onClick={() => setMode("markdown")}
              >
                <Code2 size={13} />
                <span>Markdown</span>
              </button>
            </div>
            <button
              type="button"
              className="insights-copy-btn"
              onClick={copyCurrent}
            >
              {copied ? <Check size={13} /> : <Copy size={13} />}
              <span>
                {copied
                  ? "已复制"
                  : mode === "html"
                    ? "复制 HTML"
                    : "复制 Markdown"}
              </span>
            </button>
            <button
              type="button"
              className="insights-copy-btn primary"
              onClick={() =>
                downloadFile(html, "text/html;charset=utf-8", "html")
              }
            >
              <Download size={13} />
              <span>导出 HTML</span>
            </button>
            <button
              type="button"
              className="insights-copy-btn"
              onClick={() =>
                downloadFile(markdown, "text/markdown;charset=utf-8", "md")
              }
            >
              <Download size={13} />
              <span>导出 .md</span>
            </button>
            <button
              type="button"
              className="insights-modal-close"
              onClick={onClose}
              aria-label="关闭报告预览"
            >
              <X size={16} />
            </button>
          </div>
        </div>
        {mode === "html" ? (
          <iframe
            className="insights-report-frame"
            srcDoc={html}
            sandbox="allow-scripts"
            title="AI 编程复盘报告图文预览"
          />
        ) : (
          <pre className="insights-report-pre">{markdown}</pre>
        )}
      </div>
    </div>
  );
}
