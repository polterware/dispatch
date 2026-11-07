import { makeCurve } from "../src/math";

const iterations = 500_000;
for (const points of [8, 10]) {
  let checksum = 0;
  const frame = (i: number) => {
    const curve = makeCurve({ time: i * 16.7, amplitude: (i % 101) / 100, processing: 0.4, error: 0, points, radius: 100 });
    checksum += curve.segments[i % points].end.x;
  };
  for (let i = 0; i < 30_000; i++) frame(i);
  checksum = 0;
  const start = performance.now();
  for (let i = 0; i < iterations; i++) {
    frame(i);
  }
  const elapsed = performance.now() - start;
  console.log(`${points} points: ${(elapsed * 1000 / iterations).toFixed(2)} µs/frame math (${(elapsed / iterations).toFixed(6)} ms), checksum=${checksum.toFixed(1)}`);
}
