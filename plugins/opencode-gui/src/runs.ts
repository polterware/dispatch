// How a Dispatch run reads in OpenCode: the same words as the Dispatch app.

/** One run as the plugin's server part answers it. */
export type RunView = {
  id: string;
  status: string;
  tier?: string;
  agent?: string;
  model?: string;
  provider?: string;
  role?: string;
  prompt?: string;
  worktree?: string;
  mergedAt?: string;
  runMs?: number;
  filesChanged?: number;
  insertions?: number;
  deletions?: number;
  tokens?: number;
  costUsd?: number;
  error?: string;
};

/** One run's output, and its status, as the plugin's server part answers it. */
export type LogView = {
  status: string;
  text: string;
  lines: number;
  error?: string;
};

/** One agent a forced run can go to, with the models its CLI lists. */
export type TargetAgent = { name: string; models: string[] };

export const ACTIVE = new Set(["PENDING", "RUNNING"]);
export const FAILED = new Set(["FAILED", "CANCELLED", "TIMED_OUT", "ORPHANED"]);

const STATUS_LABELS: Record<string, string> = {
  PENDING: "Queued",
  RUNNING: "Running",
  COMPLETED: "Done",
  FAILED: "Failed",
  CANCELLED: "Cancelled",
  TIMED_OUT: "Timed out",
  ORPHANED: "Interrupted",
};

export function runLink(id: string): string {
  return `dispatch://run/${encodeURIComponent(id)}`;
}

export function firstLine(text: string | undefined, max: number): string {
  const line = String(text || "").split("\n").find((part) => part.trim()) || "";
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

export function statusLabel(run: RunView): string {
  if (run.mergedAt) return "Merged";
  return STATUS_LABELS[run.status] || run.status || "Unknown";
}

/**
 * filterTargets keeps the models whose agent or model name holds every word of
 * the search, case-insensitive, and drops the agents the search leaves empty.
 */
export function filterTargets(agents: readonly TargetAgent[], query: string): TargetAgent[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return agents
    .map((agent) => ({
      name: agent.name,
      models: agent.models.filter((model) => {
        const text = `${agent.name} ${model}`.toLowerCase();
        return words.every((word) => text.includes(word));
      }),
    }))
    .filter((agent) => agent.models.length > 0);
}

export function formatTokens(tokens: number | undefined): string {
  if (typeof tokens !== "number" || tokens <= 0) return "";
  return tokens >= 1000 ? `${(tokens / 1000).toFixed(tokens >= 100_000 ? 0 : 1)}k tokens` : `${tokens} tokens`;
}

export function formatDuration(ms: number | undefined): string {
  if (!ms) return "";
  const seconds = Math.round(ms / 1000);
  return seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
}

export function runDetail(run: RunView): string {
  const parts = [statusLabel(run), [run.agent, run.model].filter(Boolean).join(" · ") || "Routing"];
  if (typeof run.filesChanged === "number" && run.filesChanged > 0) {
    parts.push(`${run.filesChanged} files +${run.insertions || 0} −${run.deletions || 0}`);
  }
  const tokens = formatTokens(run.tokens);
  if (tokens) parts.push(tokens);
  const took = formatDuration(run.runMs);
  if (took && !ACTIVE.has(run.status)) parts.push(took);
  return parts.join(" · ");
}
