#!/usr/bin/env node
// Scores the configured Dispatch classifier on the router's golden prompts: accuracy, the
// confusion matrix, and log-loss when the classifier gives probabilities. Manual and outside
// `go test`, because it calls the classifier (a network API or an agent CLI) once per prompt.
//
//   node tooling/dispatch-classify-eval.mjs [--bin dispatch] [--limit N] [--concurrency 4]
//
// Set the classifier first: dispatch config set classifier jev|openai|cli
import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const run = promisify(execFile);
const GOLDEN = join(dirname(fileURLToPath(import.meta.url)), "../go-core/domains/dispatch/testdata/router_golden.json");
const TIERS = ["easy", "normal", "hard"];

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
}

const bin = arg("--bin", "dispatch");
const limit = Number(arg("--limit", "0"));
const concurrency = Math.max(1, Number(arg("--concurrency", "4")));
let prompts = JSON.parse(readFileSync(GOLDEN, "utf8")).prompts;
if (limit > 0) prompts = prompts.slice(0, limit);

async function classify(prompt) {
  try {
    const { stdout } = await run(bin, ["classify", "--json", prompt], { maxBuffer: 1 << 20 });
    return JSON.parse(stdout).difficulty ?? { error: "no difficulty" };
  } catch (error) {
    return { error: String(error.stderr || error.message).trim().split("\n")[0] };
  }
}

const rows = new Array(prompts.length);
let next = 0;
await Promise.all(Array.from({ length: concurrency }, async () => {
  while (next < prompts.length) {
    const i = next++;
    rows[i] = { ...prompts[i], answer: await classify(prompts[i].prompt) };
  }
}));

const confusion = Object.fromEntries(TIERS.map((t) => [t, Object.fromEntries(TIERS.map((u) => [u, 0]))]));
let hits = 0, scored = 0, loss = 0, withProbs = 0, latency = 0;
const errors = [];
for (const row of rows) {
  const a = row.answer;
  if (a.error) {
    errors.push(`${row.prompt}: ${a.error}`);
    continue;
  }
  scored++;
  latency += a.latencyMs ?? 0;
  confusion[row.tier][a.tier]++;
  if (a.tier === row.tier) hits++;
  if (Array.isArray(a.probs) && a.probs.length === 3) {
    withProbs++;
    loss -= Math.log(Math.max(a.probs[TIERS.indexOf(row.tier)], 1e-6));
  }
}

console.log(`classifier: ${rows.find((r) => r.answer.provider)?.answer.provider ?? "unknown"}`);
console.log(`accuracy: ${hits}/${scored} (${scored ? ((100 * hits) / scored).toFixed(1) : "0"}%)`);
if (withProbs) console.log(`log-loss: ${(loss / withProbs).toFixed(3)}`);
if (scored) console.log(`mean latency: ${Math.round(latency / scored)} ms`);
console.log("\nlabel \\ answer  " + TIERS.map((t) => t.padStart(7)).join(""));
for (const t of TIERS) console.log(t.padEnd(16) + TIERS.map((u) => String(confusion[t][u]).padStart(7)).join(""));
if (errors.length) {
  console.log(`\n${errors.length} failed:`);
  for (const line of errors.slice(0, 10)) console.log("  " + line);
}
