import { Terminal } from "lucide-react";
import codex from "../assets/codex.svg";
import claude from "../assets/claude.svg";
import deepseek from "../assets/deepseek.svg";
import zcode from "../assets/zcode.png";
import minimax from "../assets/minimax.svg";
import gemini from "../assets/gemini.svg";
import kimi from "../assets/kimi.svg";
import xai from "../assets/xai.svg";
import opencode from "../assets/opencode-logo-light.svg";
import antigravity from "../assets/antigravity.svg";
import openclaw from "../assets/openclaw.svg";
import hermes from "../assets/hermes.svg";
import pi from "../assets/pi.svg";
const glyphs: Record<string, string> = {
  codex,
  claude,
  "claude-desktop": claude,
  dsh: deepseek,
  zcode,
  mcode: minimax,
  gemini,
  kimi,
  grokbuild: xai,
  opencode,
  agy: antigravity,
  openclaw,
  hermes,
  pi,
};
/**
 * Icons drawn dark on a transparent background. An <img> SVG cannot follow
 * the text color, so these are inverted in dark mode (see widget.css).
 */
const darkInk = new Set(["kimi", "grokbuild", "opencode", "hermes", "pi"]);
export function AppGlyph({ app }: { app: string }) {
  return (
    <span className="app-glyph" aria-hidden="true">
      {glyphs[app] ? (
        <img
          src={glyphs[app]}
          alt=""
          className={darkInk.has(app) ? "dark-ink" : undefined}
        />
      ) : (
        <Terminal size={14} />
      )}
    </span>
  );
}
