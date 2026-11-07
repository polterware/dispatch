import { describe, expect, test } from "bun:test";
import { AdaptiveNormalizer, blendParams, CAMERA_DISTANCE, createBlobMatrices, createIcosphere, easeParams, makeCurve, stateParams } from "./math";
import { BASE_RADIUS, MAX_DISPLACEMENT } from "./webgl";

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
    expect(blendParams(idle, listening, 0.5).accent[1]).toBeCloseTo((idle.accent[1] + listening.accent[1]) / 2);
    expect(stateParams("processing").speed).toBeGreaterThan(idle.speed);
    expect(stateParams("disconnected").dim).toBeGreaterThan(0);
    expect(stateParams("disconnected").speed).toBe(0);
    expect(listening.levelGain).toBeGreaterThan(0);
    expect(stateParams("speaking").levelGain).toBeGreaterThan(0);
    expect(easeParams(idle, listening, 0).amplitude).toBe(idle.amplitude);
    expect(easeParams(idle, listening, 250).amplitude).toBe(listening.amplitude);
    expect(easeParams(idle, listening, 125).amplitude).toBeCloseTo((idle.amplitude + listening.amplitude) / 2);
  });
  test("the loudest blob stays inside the frame", () => {
    for (const state of ["idle", "listening", "processing", "speaking", "error", "disconnected"] as const) {
      const p = stateParams(state);
      const amplitude = Math.min(1, p.amplitude + p.levelGain);
      expect((BASE_RADIUS + amplitude * MAX_DISPLACEMENT) * 1.035).toBeLessThan(0.98);
    }
  });
});

describe("WebGL geometry and camera", () => {
  test("creates a closed subdivided unit icosphere with in-range indices", () => {
    const sphere = createIcosphere(4);
    expect(sphere.positions.length / 3).toBeGreaterThanOrEqual(2500);
    expect(sphere.positions.length / 3).toBeLessThanOrEqual(10000);
    expect(sphere.indices.length).toBe(20 * 256 * 3);
    const edgeUse = new Map<string, number>();
    for (let i = 0; i < sphere.indices.length; i += 3) {
      const face = [sphere.indices[i], sphere.indices[i + 1], sphere.indices[i + 2]];
      for (let edge = 0; edge < 3; edge++) {
        const a = face[edge], b = face[(edge + 1) % 3], key = a < b ? `${a}:${b}` : `${b}:${a}`;
        edgeUse.set(key, (edgeUse.get(key) ?? 0) + 1);
      }
    }
    expect([...edgeUse.values()].every((uses) => uses === 2)).toBe(true);
    for (let i = 0; i < sphere.positions.length; i += 3) {
      expect(Math.hypot(sphere.positions[i], sphere.positions[i + 1], sphere.positions[i + 2])).toBeCloseTo(1, 5);
    }
    for (const index of sphere.indices) expect(index).toBeLessThan(sphere.positions.length / 3);
  });
  test("produces finite perspective and fixed view matrices for aspect ratios", () => {
    for (const aspect of [0.5, 1, 2]) {
      const matrices = createBlobMatrices(aspect);
      expect([...matrices.projection, ...matrices.view].every(Number.isFinite)).toBe(true);
      expect(matrices.projection[0] / matrices.projection[5]).toBeCloseTo(1 / aspect);
      expect(matrices.view[14]).toBeCloseTo(-CAMERA_DISTANCE);
    }
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
