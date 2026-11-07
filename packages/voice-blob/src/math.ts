export type BlobState = "idle" | "listening" | "processing" | "speaking" | "error" | "disconnected";

export interface BlobLevels {
  mic: number;
  out: number;
}

const clamp = (value: number, min = 0, max = 1) => Math.min(max, Math.max(min, value));

export class AdaptiveNormalizer {
  peak = 0.035;
  value = 0;

  update(raw: number, dt: number): number {
    const sample = clamp(Number.isFinite(raw) ? raw : 0);
    const elapsed = clamp(dt, 0, 0.25);
    this.peak = Math.max(sample, this.peak * Math.exp(-elapsed / 2.8), 0.012);
    const floor = Math.min(0.018, this.peak * 0.12);
    const normalized = clamp((sample - floor) / Math.max(0.035, this.peak * 0.82));
    const perceived = Math.sqrt(normalized);
    const time = perceived > this.value ? 0.04 : 0.2;
    const blend = 1 - Math.exp(-elapsed / time);
    this.value += (perceived - this.value) * blend;
    return this.value;
  }
}

export interface ShapePoint { x: number; y: number }
export interface Curve { start: ShapePoint; segments: Array<{ c1: ShapePoint; c2: ShapePoint; end: ShapePoint }> }

function hash(index: number, seed: number): number {
  let n = Math.imul(index ^ seed, 0x45d9f3b);
  n = Math.imul(n ^ (n >>> 16), 0x45d9f3b);
  return ((n ^ (n >>> 16)) >>> 0) / 0xffffffff;
}

function valueNoise(x: number, seed: number): number {
  const i = Math.floor(x);
  const f = x - i;
  const smooth = f * f * (3 - 2 * f);
  return (hash(i, seed) * 2 - 1) * (1 - smooth) + (hash(i + 1, seed) * 2 - 1) * smooth;
}

export function makeCurve(options: {
  time: number;
  amplitude: number;
  processing: number;
  error: number;
  points?: number;
  radius: number;
  seed?: number;
}): Curve {
  const count = options.points ?? 10;
  const { time, radius } = options;
  const points: ShapePoint[] = [];
  for (let i = 0; i < count; i++) {
    const angle = (i / count) * Math.PI * 2;
    const n1 = valueNoise(i * 0.71 + time * 0.00023, options.seed ?? 17);
    const n2 = valueNoise(i * 0.37 - time * 0.00011, (options.seed ?? 17) + 31);
    const orbit = Math.sin(angle * 2 + time * 0.00042) * options.processing * 0.055;
    const tremor = Math.sin(time * 0.06 + i * 9.3) * options.error * 0.035;
    const r = radius * (0.89 + n1 * 0.035 + n2 * 0.02 + options.amplitude * (0.11 + Math.sin(angle * 3 + time * 0.001) * 0.045) + orbit + tremor);
    points.push({ x: radius + Math.cos(angle) * r, y: radius + Math.sin(angle) * r });
  }
  const segments: Curve["segments"] = [];
  for (let i = 0; i < count; i++) {
    const p0 = points[(i - 1 + count) % count];
    const p1 = points[i];
    const p2 = points[(i + 1) % count];
    const p3 = points[(i + 2) % count];
    segments.push({
      c1: { x: clamp(p1.x + (p2.x - p0.x) / 6, 0, radius * 2), y: clamp(p1.y + (p2.y - p0.y) / 6, 0, radius * 2) },
      c2: { x: clamp(p2.x - (p3.x - p1.x) / 6, 0, radius * 2), y: clamp(p2.y - (p3.y - p1.y) / 6, 0, radius * 2) },
      end: p2,
    });
  }
  return { start: points[0], segments };
}

export interface VisualParams { amplitude: number; processing: number; warmth: number; dim: number }
export function stateParams(state: BlobState): VisualParams {
  switch (state) {
    case "listening": return { amplitude: 0.2, processing: 0, warmth: 0, dim: 0 };
    case "speaking": return { amplitude: 0.18, processing: 0, warmth: 0.08, dim: 0 };
    case "processing": return { amplitude: 0.035, processing: 1, warmth: 0.2, dim: 0 };
    case "error": return { amplitude: 0.035, processing: 0, warmth: 1, dim: 0 };
    case "disconnected": return { amplitude: 0, processing: 0, warmth: 0, dim: 0.56 };
    default: return { amplitude: 0.018, processing: 0, warmth: 0.05, dim: 0 };
  }
}

export function blendParams(from: VisualParams, to: VisualParams, amount: number): VisualParams {
  const t = clamp(amount);
  return { amplitude: from.amplitude + (to.amplitude - from.amplitude) * t,
    processing: from.processing + (to.processing - from.processing) * t,
    warmth: from.warmth + (to.warmth - from.warmth) * t,
    dim: from.dim + (to.dim - from.dim) * t };
}
