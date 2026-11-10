import { Terminal } from "lucide-react";
import codex from "../assets/openai.svg";
import claude from "../assets/claude.svg";
import deepseek from "../assets/deepseek.svg";
import zcode from "../assets/zcode.png";
import minimax from "../assets/minimax.svg";
import gemini from "../assets/gemini.svg";
import kimi from "../assets/kimi.svg";
import xai from "../assets/xai.svg";
import opencode from "../assets/opencode-logo-light.svg";
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
};
export function AppGlyph({ app }: { app: string }) {
  return (
    <span className="app-glyph" aria-hidden="true">
      {glyphs[app] ? <img src={glyphs[app]} alt="" /> : <Terminal size={14} />}
    </span>
  );
}
