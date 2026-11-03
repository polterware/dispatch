#!/usr/bin/env node
// Regenerates crates/dispatch-core/src/models_snapshot.json, the model catalog the Dispatch router
// embeds: prices and limits from models.dev (MIT), and agentic-coding scores from Epoch AI's
// benchmarking hub (CC-BY 4.0): CursorBench per model and reasoning level, plus the Epoch
// Capabilities Index as a fallback. Run by hand when models change; the core never fetches Epoch.
//
//   node tooling/dispatch-models.mjs [--models-dev <api.json>] [--epoch <benchmark_data.zip>]
//
// Without the flags it downloads both sources.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const MODELS_DEV = "https://models.dev/api.json";
const EPOCH = "https://epoch.ai/data/benchmark_data.zip";
const OUT = join(dirname(fileURLToPath(import.meta.url)), "../crates/dispatch-core/src/models_snapshot.json");
// Providers whose models the dispatch CLIs serve; aggregators repeat these and are skipped.
const PROVIDERS = ["openai", "google", "xai", "anthropic", "deepseek", "zai", "moonshotai", "mistral", "alibaba", "meta"];
const LEVELS = { minimal: "minimal", low: "low", medium: "medium", high: "high", "extra high": "xhigh", max: "max" };

function arg(name) {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
}

async function bytes(path, url) {
  if (path) return readFileSync(path);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

/** RFC 4180 rows, with quoted fields that may hold commas, quotes, and newlines. */
function parseCSV(text) {
  const rows = [];
  let row = [], field = "", quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { field += '"'; i++; }
      else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"') quoted = true;
    else if (c === ",") { row.push(field); field = ""; }
    else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field); rows.push(row); row = []; field = "";
    } else field += c;
  }
  if (field || row.length) { row.push(field); rows.push(row); }
  const [head, ...body] = rows;
  return body.filter((r) => r.length > 1).map((r) => Object.fromEntries(head.map((h, i) => [h, r[i] ?? ""])));
}

/** The catalog id of a benchmark name: "GPT-6.1 Sol" and "gpt-6.1-sol" both read gpt-6.1-sol. */
const slug = (name) => name.trim().toLowerCase().replace(/\s+/g, "-");
const num = (v) => (v === "" || v === undefined || Number.isNaN(Number(v)) ? undefined : Number(v));

/** The context tiers of a models.dev price, smallest first: the price a call pays past `size`. */
const tiers = (cost) =>
  (cost.tiers ?? [])
    .filter((t) => t.tier?.type === "context" && t.tier.size > 0)
    .map((t) => ({
      size: t.tier.size, input: t.input ?? 0, output: t.output ?? 0,
      ...(t.cache_read !== undefined && { cacheRead: t.cache_read }),
      ...(t.cache_write !== undefined && { cacheWrite: t.cache_write }),
    }))
    .sort((a, b) => a.size - b.size);

const api = JSON.parse((await bytes(arg("--models-dev"), MODELS_DEV)).toString("utf8"));
const models = [];
for (const provider of PROVIDERS) {
  for (const [id, m] of Object.entries(api[provider]?.models ?? {})) {
    if (!m.tool_call || !(m.modalities?.output ?? []).includes("text") || !m.cost) continue;
    models.push({
      id, provider, name: m.name ?? id,
      input: m.cost.input ?? 0, output: m.cost.output ?? 0,
      ...(m.cost.cache_read !== undefined && { cacheRead: m.cost.cache_read }),
      ...(m.cost.cache_write !== undefined && { cacheWrite: m.cost.cache_write }),
      ...(tiers(m.cost).length && { tiers: tiers(m.cost) }),
      context: m.limit?.context ?? 0, maxOutput: m.limit?.output ?? 0,
      reasoning: Boolean(m.reasoning),
    });
  }
}
models.sort((a, b) => a.provider.localeCompare(b.provider) || a.id.localeCompare(b.id));

const dir = mkdtempSync(join(tmpdir(), "dispatch-models-"));
let scores = [], eci = [];
try {
  const zip = join(dir, "epoch.zip");
  writeFileSync(zip, await bytes(arg("--epoch"), EPOCH));
  execFileSync("unzip", ["-q", "-o", zip, "cursorbench_external.csv", "epoch_capabilities_index/eci_scores.csv", "-d", dir]);
  for (const r of parseCSV(readFileSync(join(dir, "cursorbench_external.csv"), "utf8"))) {
    const score = num(r.Score);
    if (score === undefined) continue;
    // "gpt-5.6-terra_high" carries its level; "Composer 2.5" has none.
    const [base, suffix] = r["Model version"].includes("_") ? r["Model version"].split("_") : [r["Model version"], ""];
    const effort = LEVELS[(r["Reasoning level"] || suffix).toLowerCase()] ?? "";
    scores.push({
      model: slug(base), effort, score,
      costPerTask: num(r["Cost per task"]), tokensPerTask: num(r["Tokens per task"]),
      date: r["Release date"],
    });
  }
  for (const r of parseCSV(readFileSync(join(dir, "epoch_capabilities_index/eci_scores.csv"), "utf8"))) {
    const v = num(r.eci);
    if (v !== undefined) eci.push({ model: slug(r.Model), eci: v });
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}
scores.sort((a, b) => a.model.localeCompare(b.model) || a.effort.localeCompare(b.effort));
eci.sort((a, b) => a.model.localeCompare(b.model));

const snapshot = {
  generated: new Date().toISOString().slice(0, 10),
  sources: [
    { name: "models.dev", url: MODELS_DEV, license: "MIT", fields: "prices and limits" },
    { name: "Epoch AI, CursorBench", url: "https://epoch.ai/benchmarks", license: "CC-BY-4.0", fields: "scores, cost and tokens per task" },
    { name: "Epoch AI, Epoch Capabilities Index", url: "https://epoch.ai/benchmarks", license: "CC-BY-4.0", fields: "eci" },
  ],
  models, scores, eci,
};
const json = JSON.stringify(snapshot, null, 1) + "\n";
writeFileSync(OUT, json);
console.log(`wrote ${OUT}: ${models.length} models, ${scores.length} CursorBench scores, ${eci.length} ECI scores`);
