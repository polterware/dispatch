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

export interface VisualParams { intensity: number; speed: number; warmth: number; dim: number }
export function stateParams(state: BlobState): VisualParams {
  switch (state) {
    case "listening": return { intensity: 0.15, speed: 0.4, warmth: 0.12, dim: 0 };
    case "speaking": return { intensity: 0.2, speed: 0.55, warmth: 0.72, dim: 0 };
    case "processing": return { intensity: 0.2, speed: 0.8, warmth: 0.44, dim: 0 };
    case "error": return { intensity: 0.1, speed: 0.16, warmth: 1, dim: 0.08 };
    case "disconnected": return { intensity: 0.05, speed: 0, warmth: 0.12, dim: 0.38 };
    default: return { intensity: 0.18, speed: 0.25, warmth: 0, dim: 0 };
  }
}

export function blendParams(from: VisualParams, to: VisualParams, amount: number): VisualParams {
  const t = clamp(amount);
  return { intensity: from.intensity + (to.intensity - from.intensity) * t,
    speed: from.speed + (to.speed - from.speed) * t,
    warmth: from.warmth + (to.warmth - from.warmth) * t,
    dim: from.dim + (to.dim - from.dim) * t };
}

export function easeParams(from: VisualParams, to: VisualParams, elapsedMs: number, durationMs = 250): VisualParams {
  const linear = clamp(elapsedMs / Math.max(1, durationMs));
  return blendParams(from, to, linear * linear * (3 - 2 * linear));
}

export interface Icosphere { positions: Float32Array; normals: Float32Array; uvs: Float32Array; indices: Uint16Array | Uint32Array }

export function createIcosphere(subdivisions = 4): Icosphere {
  const t = (1 + Math.sqrt(5)) / 2;
  let vertices: [number, number, number][] = [[-1,t,0],[1,t,0],[-1,-t,0],[1,-t,0],[0,-1,t],[0,1,t],[0,-1,-t],[0,1,-t],[t,0,-1],[t,0,1],[-t,0,-1],[-t,0,1]];
  let faces: [number, number, number][] = [[0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],[1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],[3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],[4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1]];
  const norm = (v: [number,number,number]): [number,number,number] => { const l=Math.hypot(...v); return [v[0]/l,v[1]/l,v[2]/l]; };
  vertices = vertices.map(norm);
  for (let level=0; level<Math.max(0,Math.floor(subdivisions)); level++) {
    const cache = new Map<string,number>();
    const midpoint = (a:number,b:number) => { const key=a<b?`${a}:${b}`:`${b}:${a}`; const hit=cache.get(key); if(hit!==undefined)return hit; const p=norm([(vertices[a][0]+vertices[b][0])/2,(vertices[a][1]+vertices[b][1])/2,(vertices[a][2]+vertices[b][2])/2]); const i=vertices.push(p)-1; cache.set(key,i); return i; };
    const next: [number,number,number][]=[];
    for(const [a,b,c] of faces){const ab=midpoint(a,b),bc=midpoint(b,c),ca=midpoint(c,a);next.push([a,ab,ca],[b,bc,ab],[c,ca,bc],[ab,bc,ca]);}
    faces=next;
  }
  const positions = new Float32Array(vertices.flat());
  const normals = new Float32Array(positions);
  const uvs = new Float32Array(vertices.length*2);
  vertices.forEach(([x,y,z],i)=>{uvs[i*2]=Math.atan2(z,x)/(2*Math.PI)+0.5;uvs[i*2+1]=Math.asin(Math.max(-1,Math.min(1,y)))/Math.PI+0.5;});
  const flat=faces.flat(); const indices=vertices.length>65535?new Uint32Array(flat):new Uint16Array(flat);
  return {positions,normals,uvs,indices};
}

export interface BlobMatrices { projection: Float32Array; view: Float32Array }
export function createBlobMatrices(aspect: number): BlobMatrices {
  const safeAspect=Math.max(0.0001,Number.isFinite(aspect)?aspect:1), f=1/Math.tan(Math.PI/8), near=0.1, far=100;
  const projection=new Float32Array([f/safeAspect,0,0,0, 0,f,0,0, 0,0,(far+near)/(near-far),-1, 0,0,(2*far*near)/(near-far),0]);
  const view=new Float32Array([1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,-3.15,1]);
  return {projection,view};
}
