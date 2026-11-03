import type { BotAvatarType } from "./types";

/**
 * Known agents use a library body that still reads under the fur, plus the
 * brand color. Custom outlines collapse into the same plush blob.
 */
export type AgentMark = {
  id: string;
  type: BotAvatarType;
  color: string;
};

const MARKS: Array<AgentMark & { aliases: string[] }> = [
  { id: "claude", aliases: ["claude", "claudecode", "anthropic"], type: "star", color: "#D97757" },
  { id: "codex", aliases: ["codex", "openai", "chatgpt"], type: "flower", color: "#19C37D" },
  { id: "cursor", aliases: ["cursor", "cursoragent"], type: "square", color: "#F4F4F5" },
  { id: "gemini", aliases: ["gemini", "antigravity", "agy"], type: "triangle", color: "#8AB4F8" },
  { id: "grok", aliases: ["grok", "xai"], type: "hexagon", color: "#E4E4E7" },
  { id: "copilot", aliases: ["copilot", "githubcopilot"], type: "cat", color: "#EDEDED" },
  { id: "windsurf", aliases: ["windsurf", "codeium"], type: "cloud", color: "#5EEAD4" },
  { id: "continue", aliases: ["continue"], type: "pill", color: "#F5C16C" },
  { id: "cline", aliases: ["cline"], type: "ghost", color: "#7DD3FC" },
  { id: "roo", aliases: ["roo", "roocode"], type: "alien", color: "#E9D5FF" },
  { id: "aider", aliases: ["aider"], type: "pebble", color: "#86EFAC" },
  { id: "amp", aliases: ["amp"], type: "drop", color: "#FDBA74" },
  { id: "zed", aliases: ["zed"], type: "mech", color: "#93C5FD" },
  { id: "opencode", aliases: ["opencode"], type: "droid", color: "#CBD5E1" },
  { id: "goose", aliases: ["goose"], type: "blob", color: "#FDE68A" },
  { id: "junie", aliases: ["junie", "jetbrains"], type: "clover", color: "#FB923C" },
  { id: "kiro", aliases: ["kiro", "amazonq"], type: "puddle", color: "#FCD34D" },
  { id: "devin", aliases: ["devin"], type: "circle", color: "#A5B4FC" },
  { id: "augment", aliases: ["augment"], type: "hexagon", color: "#67E8F9" },
  { id: "cody", aliases: ["cody", "sourcegraph"], type: "droid", color: "#C4B5FD" },
  { id: "qwen", aliases: ["qwen", "qoder"], type: "drop", color: "#6EE7B7" },
  { id: "factory", aliases: ["factory", "droid"], type: "mech", color: "#D6D3D1" },
  { id: "trae", aliases: ["trae"], type: "triangle", color: "#FDA4AF" },
  { id: "crush", aliases: ["crush"], type: "pebble", color: "#F9A8D4" },
  { id: "pi", aliases: ["pi"], type: "circle", color: "#FDE047" },
  { id: "kimi", aliases: ["kimi", "moonshot", "moonshotai"], type: "pill", color: "#A5B4FC" },
];

/** The mark for a known agent label, or null so the caller keeps a stable hashed body. */
export function agentMarkFor(label: string): AgentMark | null {
  const tokens = label.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  if (tokens.length === 0) return null;
  const joined = tokens.join("");
  for (const mark of MARKS) {
    for (const alias of mark.aliases) {
      if (tokens.includes(alias) || joined === alias) return mark;
      if (alias.length >= 5 && joined.includes(alias)) return mark;
    }
  }
  return null;
}
