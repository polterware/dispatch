import { AdaptiveNormalizer, blendParams, stateParams } from "../src/math";

const iterations = 1_000_000;
const mic = new AdaptiveNormalizer();
const out = new AdaptiveNormalizer();
let checksum = 0;
for (let i = 0; i < 50_000; i++) checksum += mic.update((i % 100) / 100, 1 / 60);
checksum = 0;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  const state = i % 2 ? "listening" : "speaking";
  const params = blendParams(stateParams("idle"), stateParams(state), (i % 60) / 60);
  const level = state === "listening" ? mic.update((i % 100) / 100, 1 / 60) : out.update((i % 100) / 100, 1 / 60);
  checksum += params.intensity + params.speed + level;
}
const elapsed = performance.now() - start;
console.log(`per-frame CPU prep: ${(elapsed * 1000 / iterations).toFixed(2)} µs/frame math (${(elapsed / iterations).toFixed(6)} ms), checksum=${checksum.toFixed(1)}`);
