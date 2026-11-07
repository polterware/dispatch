import { describe, expect, test } from "bun:test";
import { AdaptiveNormalizer, blendParams, makeCurve, stateParams } from "./math";

describe("adaptive level normalization", () => {
  test("tracks quiet and loud input with a decaying peak", () => {
    const n = new AdaptiveNormalizer();
    const quiet = n.update(0.025, 1 / 30);
    expect(quiet).toBeGreaterThan(0);
    n.update(0.8, 1 / 30);
    expect(n.peak).toBeGreaterThan(0.7);
    const peak = n.peak;
    n.update(0, 1);
    expect(n.peak).toBeLessThan(peak);
    expect(n.peak).toBeGreaterThanOrEqual(0.012);
  });
  test("attacks faster than release and clamps invalid input", () => {
    const n = new AdaptiveNormalizer();
    const attack = n.update(1, 0.04);
    expect(attack).toBeGreaterThan(0);
    const release = n.update(0, 0.04);
    expect(release).toBeLessThan(attack);
    expect(n.update(Number.NaN, 0.04)).toBeGreaterThanOrEqual(0);
  });
});

describe("state blending", () => {
  test("interpolates and exposes distinct state targets", () => {
    const idle = stateParams("idle");
    const listening = stateParams("listening");
    expect(blendParams(idle, listening, 0.5).amplitude).toBeCloseTo((idle.amplitude + listening.amplitude) / 2);
    expect(stateParams("processing").processing).toBeGreaterThan(stateParams("idle").processing);
    expect(stateParams("disconnected").dim).toBeGreaterThan(0);
    expect(stateParams("error").warmth).toBe(1);
  });
});

describe("closed curve generation", () => {
  test("produces finite cubic segments inside its canvas bounds", () => {
    const radius = 90;
    const curve = makeCurve({ time: 12_345, amplitude: 0.7, processing: 0.5, error: 0.2, points: 10, radius });
    expect(curve.segments).toHaveLength(10);
    for (const p of [curve.start, ...curve.segments.flatMap((s) => [s.c1, s.c2, s.end])]) {
      expect(Number.isFinite(p.x) && Number.isFinite(p.y)).toBe(true);
      expect(p.x).toBeGreaterThanOrEqual(0);
      expect(p.x).toBeLessThanOrEqual(radius * 2);
      expect(p.y).toBeGreaterThanOrEqual(0);
      expect(p.y).toBeLessThanOrEqual(radius * 2);
    }
    expect(curve.segments.at(-1)?.end).toEqual(curve.start);
  });
});
