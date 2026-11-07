"use strict";
Object.defineProperties(exports, { __esModule: { value: true }, [Symbol.toStringTag]: { value: "Module" } });
const web = require("solid-js/web");
const gui = require("@opencode/plugin/gui");
const solidJs = require("solid-js");
const store = require("solid-js/store");
const state = {
  type: "object",
  properties: {
    enabled: { type: "boolean" },
    binary: { type: "string" },
    available: { type: "boolean" }
  },
  required: ["enabled", "binary", "available"],
  additionalProperties: false
};
const Dispatch = {
  id: "dispatch",
  methods: {
    // OpenCode reads every method's input, so one with none takes an empty object.
    state: { input: { type: "object", additionalProperties: false }, output: state },
    setEnabled: {
      input: {
        type: "object",
        properties: { enabled: { type: "boolean" } },
        required: ["enabled"],
        additionalProperties: false
      },
      output: state
    },
    // Opens a dispatch:// link in the Dispatch app; the Desktop's openExternal
    // opens only web links.
    open: {
      input: {
        type: "object",
        properties: { url: { type: "string" } },
        required: ["url"],
        additionalProperties: false
      },
      output: {
        type: "object",
        properties: { opened: { type: "boolean" }, error: { type: "string" } },
        required: ["opened"],
        additionalProperties: false
      }
    },
    runs: {
      input: {
        type: "object",
        properties: { sessionID: { type: "string" }, directory: { type: "string" } },
        required: ["sessionID"],
        additionalProperties: false
      },
      output: {
        type: "object",
        properties: { runs: { type: "array", items: { type: "object" } }, error: { type: "string" } },
        required: ["runs"],
        additionalProperties: false
      }
    },
    // The Delegate check beside the composer pill, per conversation; directory
    // is taken for symmetry with runs and not needed to resolve the root. A
    // forced conversation also keeps the agent and model its runs go to, empty
    // for Auto.
    force: {
      input: {
        type: "object",
        properties: { sessionID: { type: "string" }, directory: { type: "string" } },
        required: ["sessionID"],
        additionalProperties: false
      },
      output: {
        type: "object",
        properties: { force: { type: "boolean" }, agent: { type: "string" }, model: { type: "string" } },
        required: ["force", "agent", "model"],
        additionalProperties: false
      }
    },
    setForce: {
      input: {
        type: "object",
        properties: {
          sessionID: { type: "string" },
          directory: { type: "string" },
          force: { type: "boolean" },
          // The target the forced runs go to; empty strings mean Auto.
          agent: { type: "string" },
          model: { type: "string" }
        },
        required: ["sessionID", "force"],
        additionalProperties: false
      },
      output: {
        type: "object",
        properties: { force: { type: "boolean" }, agent: { type: "string" }, model: { type: "string" } },
        required: ["force", "agent", "model"],
        additionalProperties: false
      }
    },
    // The agents and models a forced conversation can send its runs to, from
    // `dispatch agents --json` and `dispatch models --json`.
    targets: {
      input: { type: "object", additionalProperties: false },
      output: {
        type: "object",
        properties: {
          agents: {
            type: "array",
            items: {
              type: "object",
              properties: {
                name: { type: "string" },
                models: { type: "array", items: { type: "string" } }
              },
              required: ["name", "models"],
              additionalProperties: false
            }
          },
          error: { type: "string" }
        },
        required: ["agents"],
        additionalProperties: false
      }
    },
    // The run's log so far and the status the panel polls on: the Desktop part
    // shows it in place instead of only opening the run in the app.
    log: {
      input: {
        type: "object",
        properties: {
          runID: { type: "string" },
          directory: { type: "string" }
        },
        required: ["runID"],
        additionalProperties: false
      },
      output: {
        type: "object",
        properties: {
          status: { type: "string" },
          text: { type: "string" },
          lines: { type: "integer" },
          error: { type: "string" }
        },
        required: ["status", "text", "lines"],
        additionalProperties: false
      }
    }
  },
  events: {
    changed: {
      schema: {
        type: "object",
        properties: { enabled: { type: "boolean" } },
        required: ["enabled"],
        additionalProperties: false
      }
    },
    started: {
      schema: {
        type: "object",
        properties: { sessionID: { type: "string" }, runID: { type: "string" } },
        required: ["sessionID", "runID"],
        additionalProperties: false
      }
    },
    // The conversation's Delegate check changed, from this window or another.
    forced: {
      schema: {
        type: "object",
        properties: { sessionID: { type: "string" }, force: { type: "boolean" } },
        required: ["sessionID", "force"],
        additionalProperties: false
      }
    }
  }
};
function parseColor(input) {
  const s = input.trim();
  const hsl = s.match(/^hsla?\(\s*([\d.]+)(?:deg)?[,\s]+([\d.]+)%[,\s]+([\d.]+)%/i);
  if (hsl) return hslToRgb([Number(hsl[1]) / 360, Number(hsl[2]) / 100, Number(hsl[3]) / 100]);
  const hex = s.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    let h = hex[1];
    if (h.length === 3) h = h.split("").map((c) => c + c).join("");
    const n = parseInt(h, 16);
    return [n >> 16 & 255, n >> 8 & 255, n & 255];
  }
  const rgb = s.match(/^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)/i);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}
function luminance(color) {
  const c = parseColor(color);
  if (!c) return 0.5;
  const lin = (v) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
}
const DARK_INK = "#1E1A33";
const LIGHT_INK = "#F7F5F2";
function autoInk(color) {
  return luminance(color) < 0.13 ? LIGHT_INK : DARK_INK;
}
function rgbToHsl([r, g, b]) {
  r /= 255;
  g /= 255;
  b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h = 0;
  if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return [h / 6, s, l];
}
function hslToRgb([h, s, l]) {
  if (s === 0) return [l * 255, l * 255, l * 255];
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const f = (t) => {
    t = (t % 1 + 1) % 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  return [f(h + 1 / 3) * 255, f(h) * 255, f(h - 1 / 3) * 255];
}
function hslToCss([h, s, l]) {
  return `hsl(${(h * 360).toFixed(1)} ${(s * 100).toFixed(1)}% ${(l * 100).toFixed(1)}%)`;
}
const clamp01$1 = (v) => Math.min(1, Math.max(0, v));
function shade(color, dl, ds = 0) {
  const c = parseColor(color);
  if (!c) return color;
  const [h, s, l] = rgbToHsl(c);
  return hslToCss([h, clamp01$1(s + ds + (dl < 0 ? -dl * 0.25 : 0)), clamp01$1(l + dl)]);
}
const toLinear = (u) => u <= 0.04045 ? u / 12.92 : Math.pow((u + 0.055) / 1.055, 2.4);
const toSrgb = (u) => u <= 31308e-7 ? 12.92 * u : 1.055 * Math.pow(u, 1 / 2.4) - 0.055;
function oklab(r, g, b) {
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
}
function fromOklab(L, a, b) {
  const l = Math.pow(L + 0.3963377774 * a + 0.2158037573 * b, 3);
  const m = Math.pow(L - 0.1055613458 * a - 0.0638541728 * b, 3);
  const s = Math.pow(L - 0.0894841775 * a - 1.291485548 * b, 3);
  return [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s];
}
const inGamut = (c) => c.every((v) => v >= -1e-4 && v <= 1 + 1e-4);
function richer(color, t) {
  const c = parseColor(color);
  if (!c || !(t > 0)) return color;
  const [L, a, b] = oklab(toLinear(c[0] / 255), toLinear(c[1] / 255), toLinear(c[2] / 255));
  const C = Math.hypot(a, b);
  if (C < 1e-4) return color;
  const ua = a / C, ub = b / C;
  const want = C * (1 + 0.5 * t);
  let best = null, bestC = C;
  for (let k = 0; k <= 6; k++) {
    const Lk = L * (1 - 0.15 * t * k / 6);
    let lo = 0, hi = want;
    for (let it = 0; it < 18; it++) {
      const mid = (lo + hi) / 2;
      if (inGamut(fromOklab(Lk, ua * mid, ub * mid))) lo = mid;
      else hi = mid;
    }
    if (lo > bestC + 4e-3 * k) {
      bestC = lo;
      best = fromOklab(Lk, ua * lo, ub * lo);
    }
    if (lo >= want - 1e-4) break;
  }
  if (!best) return color;
  const out = best.map((v) => Math.round(255 * toSrgb(Math.min(1, Math.max(0, v)))));
  return `rgb(${out[0]} ${out[1]} ${out[2]})`;
}
const PAD = 3;
const SPAN = 100 + 2 * PAD;
const M = 64;
const MM = M * M;
const CONIC_STOPS = 24;
const INF = 1e12;
const EL$1 = 48 * Math.PI / 180;
const E_XY = Math.cos(EL$1), E_Z = Math.sin(EL$1);
function edt1d(f, n, d, s, v, z) {
  let k = 0;
  v[0] = 0;
  z[0] = -1e30;
  z[1] = 1e30;
  for (let q = 1; q < n; q++) {
    let x = 0;
    for (; ; ) {
      const vk = v[k];
      x = (f[q] + q * q - f[vk] - vk * vk) / (2 * (q - vk));
      if (x > z[k]) break;
      k--;
    }
    k++;
    v[k] = q;
    z[k] = x;
    z[k + 1] = 1e30;
  }
  k = 0;
  for (let q = 0; q < n; q++) {
    while (z[k + 1] < q) k++;
    const vk = v[k];
    d[q] = (q - vk) * (q - vk) + f[vk];
    s[q] = vk;
  }
}
function edt2d(mask, site, N, out, near) {
  const f = new Float32Array(N), d = new Float32Array(N), s = new Int32Array(N), v = new Int32Array(N), z = new Float32Array(N + 1);
  const g = new Float32Array(N * N), row = new Int32Array(N * N);
  for (let x = 0; x < N; x++) {
    for (let y = 0; y < N; y++) f[y] = mask[y * N + x] === site ? 0 : INF;
    edt1d(f, N, d, s, v, z);
    for (let y = 0; y < N; y++) {
      g[y * N + x] = d[y];
      row[y * N + x] = s[y];
    }
  }
  for (let y = 0; y < N; y++) {
    const o = y * N;
    for (let x = 0; x < N; x++) f[x] = g[o + x];
    edt1d(f, N, d, s, v, z);
    for (let x = 0; x < N; x++) {
      out[o + x] = d[x];
      if (near) near[o + x] = row[o + s[x]] * N + s[x];
    }
  }
}
function blur5(a, N, tmp) {
  for (let y = 0; y < N; y++) {
    const o = y * N;
    for (let x = 0; x < N; x++) {
      const x0 = x < 2 ? 0 : x - 2, x1 = x < 1 ? 0 : x - 1, x3 = x > N - 2 ? N - 1 : x + 1, x4 = x > N - 3 ? N - 1 : x + 2;
      tmp[o + x] = (a[o + x0] + 4 * a[o + x1] + 6 * a[o + x] + 4 * a[o + x3] + a[o + x4]) * 0.0625;
    }
  }
  for (let x = 0; x < N; x++) {
    for (let y = 0; y < N; y++) {
      const y0 = y < 2 ? 0 : y - 2, y1 = y < 1 ? 0 : y - 1, y3 = y > N - 2 ? N - 1 : y + 1, y4 = y > N - 3 ? N - 1 : y + 2;
      a[y * N + x] = (tmp[y0 * N + x] + 4 * tmp[y1 * N + x] + 6 * tmp[y * N + x] + 4 * tmp[y3 * N + x] + tmp[y4 * N + x]) * 0.0625;
    }
  }
}
function poisson(cov, N, u) {
  const levels = [];
  for (let n = N, f = 1; f <= 4 && n % 2 === 0 || f === 1; n >>= 1, f <<= 1) {
    const mask = new Uint8Array(n * n);
    for (let y = 0; y < n; y++) {
      for (let x = 0; x < n; x++) {
        let sum = 0;
        for (let j = 0; j < f; j++) for (let i = 0; i < f; i++) sum += cov[(y * f + j) * N + x * f + i];
        mask[y * n + x] = sum >= 128 * f * f ? 1 : 0;
      }
    }
    levels.push({ n, mask, phi: new Float32Array(n * n) });
    if (f === 4) break;
  }
  const sweep = (L, s, iters, om) => {
    const { n, mask, phi } = L, s2 = s * s;
    for (let it = 0; it < iters; it++) {
      for (let y = 1; y < n - 1; y++) {
        const o = y * n;
        for (let x = 1; x < n - 1; x++) {
          const i = o + x;
          if (!mask[i]) continue;
          const v = (phi[i - 1] + phi[i + 1] + phi[i - n] + phi[i + n] + s2) * 0.25;
          phi[i] += om * (v - phi[i]);
        }
      }
    }
  };
  for (let l = levels.length - 1; l >= 0; l--) {
    const L = levels[l], f = 1 << l;
    if (l < levels.length - 1) {
      const C = levels[l + 1], n = L.n, cn = C.n;
      for (let y = 0; y < n; y++) {
        const fy = Math.min(cn - 1, Math.max(0, (y + 0.5) / 2 - 0.5)), y0 = fy | 0, y1 = Math.min(cn - 1, y0 + 1), ty = fy - y0;
        for (let x = 0; x < n; x++) {
          const i = y * n + x;
          if (!L.mask[i]) continue;
          const fx = Math.min(cn - 1, Math.max(0, (x + 0.5) / 2 - 0.5)), x0 = fx | 0, x1 = Math.min(cn - 1, x0 + 1), tx = fx - x0;
          L.phi[i] = (C.phi[y0 * cn + x0] * (1 - tx) + C.phi[y0 * cn + x1] * tx) * (1 - ty) + (C.phi[y1 * cn + x0] * (1 - tx) + C.phi[y1 * cn + x1] * tx) * ty;
        }
      }
    }
    const om = Math.min(1.9, 2 / (1 + Math.sin(Math.PI / L.n)) - 0.05);
    sweep(L, u * f, 4, 1);
    sweep(L, u * f, l === 2 ? 100 : l === 1 ? 30 : 16, om);
    sweep(L, u * f, 8, 1);
  }
  return levels[0].phi;
}
const DX = [1, 1, 0, -1, -1, -1, 0, 1], DY = [0, 1, 1, 1, 0, -1, -1, -1];
const DL = [1, Math.SQRT2, 1, Math.SQRT2, 1, Math.SQRT2, 1, Math.SQRT2];
function buildForm(cov, N, halfDepth) {
  const u = SPAN / N, NN = N * N;
  const mask = new Uint8Array(NN);
  for (let i = 0; i < NN; i++) mask[i] = cov[i] >= 128 ? 1 : 0;
  const dIn = new Float32Array(NN), dOut = new Float32Array(NN), nearIn = new Int32Array(NN);
  edt2d(mask, 0, N, dIn, null);
  edt2d(mask, 1, N, dOut, nearIn);
  const sd = new Float32Array(NN), tmp = new Float32Array(NN);
  for (let i = 0; i < NN; i++) {
    const a = cov[i] / 255;
    sd[i] = u * (a > 0 && a < 1 ? a - 0.5 : mask[i] ? Math.sqrt(dIn[i]) - 0.5 : 0.5 - Math.sqrt(dOut[i]));
  }
  blur5(sd, N, tmp);
  blur5(sd, N, tmp);
  const phi = poisson(cov, N, u);
  let phiMax = 0;
  for (let i = 0; i < NN; i++) if (phi[i] > phiMax) phiMax = phi[i];
  const rIn = 2 * Math.sqrt(phiMax);
  const hMax = Math.min(1.5 * halfDepth + 0.32 * rIn, 1.4 * rIn);
  const kh = phiMax > 0 ? hMax / Math.sqrt(phiMax) : 0;
  const h = new Float32Array(NN);
  for (let i = 0; i < NN; i++) h[i] = phi[i] > 0 ? kh * Math.sqrt(phi[i]) : 0;
  blur5(h, N, tmp);
  const form = { N, i00: new Uint16Array(NN), wx: new Uint8Array(NN), wy: new Uint8Array(NN), ao: new Uint8Array(NN), sd, h };
  const { i00, wx, wy, ao } = form;
  const STEPS = N <= 64 ? [1, 2, 3, 5, 8] : N <= 96 ? [1, 2, 4, 7, 11] : [1, 2, 4, 7, 11, 15];
  const halo = 64;
  const last2 = N - 1;
  for (let y = 0; y < N; y++) {
    for (let x = 0; x < N; x++) {
      const i = y * N + x;
      const src = mask[i] ? i : dOut[i] <= halo ? nearIn[i] : -1;
      if (src < 0) continue;
      const sx = src % N, sy = (src - sx) / N;
      const xl = sx > 0 ? sx - 1 : 0, xr = sx < last2 ? sx + 1 : last2, yu = sy > 0 ? sy - 1 : 0, yd = sy < last2 ? sy + 1 : last2;
      let nx = -(h[sy * N + xr] - h[sy * N + xl]) / (2 * u);
      let ny = -(h[yd * N + sx] - h[yu * N + sx]) / (2 * u);
      let nz = 1;
      let len = Math.sqrt(nx * nx + ny * ny + 1);
      nx /= len;
      ny /= len;
      nz /= len;
      const dd = Math.max(0, sd[src]);
      if (dd < 2) {
        let gx = sd[sy * N + xr] - sd[sy * N + xl], gy = sd[yd * N + sx] - sd[yu * N + sx];
        const gl = Math.hypot(gx, gy) || 1;
        gx /= gl;
        gy /= gl;
        const w = 0.7 * (1 - dd / 2);
        nx += w * (-gx - nx);
        ny += w * (-gy - ny);
        nz += w * (0 - nz);
        len = Math.sqrt(nx * nx + ny * ny + nz * nz) || 1;
        nx /= len;
        ny /= len;
        nz /= len;
      }
      const h0 = h[src];
      let occ = 0;
      for (let d = 0; d < 8; d++) {
        let m = 0;
        for (let k = 0; k < STEPS.length; k++) {
          const r = STEPS[k];
          let qx = sx + DX[d] * r, qy = sy + DY[d] * r;
          if (qx < 0) qx = 0;
          else if (qx > last2) qx = last2;
          if (qy < 0) qy = 0;
          else if (qy > last2) qy = last2;
          const t = (h[qy * N + qx] - h0) / (r * u * DL[d]);
          if (t > m) m = t;
        }
        occ += m / Math.sqrt(1 + m * m);
      }
      const e = 1 - Math.min(1, dd / 3);
      const edge = 1 - 0.2 * e * e;
      const aoLin = Math.pow(1 - 0.9 * occ / 8, 1.5) * edge;
      ao[i] = Math.max(1, Math.round(255 * Math.pow(aoLin, 1 / 2.2)));
      const fx = (nx * 0.5 + 0.5) * (M - 1), fy = (ny * 0.5 + 0.5) * (M - 1);
      const cx = Math.min(M - 2, Math.max(0, fx | 0)), cy = Math.min(M - 2, Math.max(0, fy | 0));
      i00[i] = cy * M + cx;
      wx[i] = Math.round(255 * Math.min(1, Math.max(0, fx - cx)));
      wy[i] = Math.round(255 * Math.min(1, Math.max(0, fy - cy)));
    }
  }
  return form;
}
function makeCanvas(n) {
  if (typeof OffscreenCanvas === "function") return new OffscreenCanvas(n, n);
  if (typeof document !== "undefined") {
    const c = document.createElement("canvas");
    c.width = c.height = n;
    return c;
  }
  return null;
}
function ctx2d(c, readBack) {
  return c.getContext("2d", readBack ? { willReadFrequently: true } : void 0);
}
function rasterize(path, N) {
  const c = makeCanvas(N);
  const g = c && ctx2d(c, true);
  if (!g) return null;
  const u = SPAN / N;
  g.setTransform(1 / u, 0, 0, 1 / u, PAD / u, PAD / u);
  g.fillStyle = "#fff";
  g.fill(path);
  const px = g.getImageData(0, 0, N, N).data;
  const cov = new Uint8ClampedArray(N * N);
  for (let i = 0; i < N * N; i++) cov[i] = px[i * 4 + 3];
  return cov;
}
const forms = /* @__PURE__ */ new Map();
const pending = /* @__PURE__ */ new Set();
const pathIds = /* @__PURE__ */ new WeakMap();
let nextPathId = 0;
function pathId(p) {
  let id = pathIds.get(p);
  if (!id) pathIds.set(p, id = `p${nextPathId++}`);
  return id;
}
const queue = [];
let scheduled = false;
const FORCED_MS = 6;
const clock = () => typeof performance !== "undefined" ? performance.now() : Date.now();
const HURRY_MS = 20;
let hurrying = 0;
function pump(deadline) {
  scheduled = false;
  const t0 = clock();
  let fn = queue.shift();
  while (fn) {
    fn();
    const room = hurrying > 0 ? HURRY_MS - (clock() - t0) : deadline && !deadline.didTimeout ? deadline.timeRemaining() - 6 : FORCED_MS - (clock() - t0);
    if (room <= 0) break;
    fn = queue.shift();
  }
  if (queue.length) schedule();
}
function schedule() {
  if (scheduled) return;
  scheduled = true;
  if (hurrying > 0) {
    setTimeout(() => pump(), 0);
    return;
  }
  const ric = globalThis.requestIdleCallback;
  if (ric) ric(pump, { timeout: 60 });
  else setTimeout(() => pump(), 24);
}
function hurry(on) {
  hurrying = Math.max(0, hurrying + (on ? 1 : -1));
  if (on && queue.length) setTimeout(() => pump(), 0);
}
function bakesPending() {
  return pending.size > 0 || furPending.size > 0;
}
function idle(fn) {
  queue.push(fn);
  schedule();
}
const landed = /* @__PURE__ */ new Set();
let missed = false;
let landing = false;
function land() {
  if (landing || !landed.size) return;
  landing = true;
  setTimeout(() => {
    landing = false;
    for (const fn of [...landed]) fn();
  }, 0);
}
function onLanded(fn) {
  landed.add(fn);
  return () => {
    landed.delete(fn);
  };
}
function takeMissed() {
  const m = missed;
  missed = false;
  return m;
}
function formFor(key, path, N, halfDepth, sync) {
  const id = `${key}|${N}|${Math.round(halfDepth)}`;
  const hit = forms.get(id);
  if (hit) return hit;
  const build = () => {
    pending.delete(id);
    if (forms.has(id)) return;
    const cov = rasterize(path, N);
    if (!cov) return;
    if (forms.size >= 48) forms.clear();
    forms.set(id, buildForm(cov, N, halfDepth));
    land();
  };
  if (sync) {
    build();
    return forms.get(id) ?? null;
  }
  if (!pending.has(id)) {
    pending.add(id);
    idle(build);
  }
  missed = true;
  return null;
}
function warmPlastic(key, path, devicePx = 192, depth = 0.65, fabric = false, style = FUR_STOCK, light = 295, lights) {
  const form = formFor(key, path, tierFor(devicePx), 15 * depth, true);
  if (fabric && form) {
    const a = light * Math.PI / 180;
    furReady(form, key, 15 * depth, Math.ceil(SPAN * devicePx / 100), false, Math.sin(a), -Math.cos(a), style, lights ?? stockLights(Math.sin(a), -Math.cos(a)));
  }
}
function tierFor(devicePx) {
  return devicePx <= 100 ? 64 : devicePx <= 224 ? 96 : 128;
}
const reliefs = /* @__PURE__ */ new WeakMap();
function reliefFor(form, K, round) {
  const q = Math.max(0.05, Math.min(1, round));
  const key = `${K}|${q.toFixed(2)}`;
  let byKey = reliefs.get(form);
  if (!byKey) reliefs.set(form, byKey = /* @__PURE__ */ new Map());
  const hit = byKey.get(key);
  if (hit) return hit;
  const { N, h, sd } = form;
  let top = 0;
  for (let i = 0; i < N * N; i++) if (h[i] > top) top = h[i];
  const u = SPAN / N;
  const z = new Float32Array(K);
  const scale2 = new Float32Array(K), cx = new Float32Array(K), cy = new Float32Array(K), inset = new Float32Array(K);
  inset[0] = 1.2;
  const measure = (inside) => {
    let n = 0, sx = 0, sy = 0;
    for (let j = 0, i = 0; j < N; j++) for (let x = 0; x < N; x++, i++) if (inside(i)) n += 1, sx += x, sy += j;
    return n ? [n, (sx / n + 0.5) * u - PAD, (sy / n + 0.5) * u - PAD] : [0, 50, 50];
  };
  const [a0, x0, y0] = measure((i) => sd[i] >= 0);
  scale2[0] = 1;
  cx[0] = x0;
  cy[0] = y0;
  for (let k = 1; k < K; k++) {
    const s = k / K;
    z[k] = top * s;
    const tau = top * Math.pow(s, 1 / q);
    const [ak, xk, yk] = measure((i) => sd[i] >= 0 && h[i] >= tau);
    scale2[k] = Math.min(scale2[k - 1], a0 ? Math.sqrt(ak / a0) : 1);
    cx[k] = xk;
    cy[k] = yk;
    const band = top / (2 * K);
    let n = 0, sum = 0;
    for (let i = 0; i < N * N; i++) {
      if (sd[i] > 0 && Math.abs(h[i] - tau) < band) {
        sum += sd[i];
        n++;
      }
    }
    inset[k] = Math.max(inset[k - 1], n ? sum / n : inset[k - 1]);
  }
  const r = { top, z, q, scale: scale2, cx, cy, inset };
  if (byKey.size > 6) byKey.clear();
  byKey.set(key, r);
  return r;
}
function surfaceAt(key, halfDepth, x, y, round = 1) {
  const prefix = `${key}|`, suffix = `|${Math.round(halfDepth)}`;
  let form = null;
  for (const [id, f] of forms) if (id.startsWith(prefix) && id.endsWith(suffix)) form = f;
  if (!form) return null;
  const { N, h } = form;
  const u = SPAN / N;
  const fx = Math.min(N - 1, Math.max(0, (x + PAD) / u - 0.5)), fy = Math.min(N - 1, Math.max(0, (y + PAD) / u - 0.5));
  const x0 = Math.min(N - 2, fx | 0), y0 = Math.min(N - 2, fy | 0), tx = fx - x0, ty = fy - y0, i = y0 * N + x0;
  const v = (h[i] * (1 - tx) + h[i + 1] * tx) * (1 - ty) + (h[i + N] * (1 - tx) + h[i + N + 1] * tx) * ty;
  let top = 0;
  for (let k = 0; k < N * N; k++) if (h[k] > top) top = h[k];
  if (top <= 0) return 0;
  const q = Math.max(0.05, Math.min(1, round));
  return top * Math.pow(Math.max(0, v) / top, q);
}
const toLin = (v) => v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
const linCache = /* @__PURE__ */ new Map();
function linearColor(color) {
  let c = linCache.get(color);
  if (!c) {
    let rgb = parseColor(color);
    if (!rgb) {
      const cv = makeCanvas(1);
      const g = cv && ctx2d(cv, true);
      if (g) {
        g.fillStyle = color;
        g.fillRect(0, 0, 1, 1);
        const p = g.getImageData(0, 0, 1, 1).data;
        rgb = [p[0], p[1], p[2]];
      } else rgb = [128, 128, 128];
    }
    c = [toLin(rgb[0] / 255), toLin(rgb[1] / 255), toLin(rgb[2] / 255)];
    if (linCache.size > 200) linCache.clear();
    linCache.set(color, c);
  }
  return c;
}
const TONE_N = 2048, TONE_MAX = 2.5, TONE_SCALE = TONE_N / TONE_MAX;
const toneLut = new Float32Array(TONE_N);
for (let i = 0; i < TONE_N; i++) {
  const v = (i + 0.5) / TONE_SCALE;
  const y = v <= 0.75 ? v : 0.75 + 0.25 * (1 - Math.exp(-(v - 0.75) / 0.25));
  toneLut[i] = 255 * (y <= 31308e-7 ? 12.92 * y : 1.055 * Math.pow(y, 1 / 2.4) - 0.055);
}
const tone = (v) => toneLut[v <= 0 ? 0 : v >= TONE_MAX ? TONE_N - 1 : v * TONE_SCALE | 0];
const srgb = (v) => v <= 0 ? 0 : v >= 1 ? 255 : 255 * (v <= 31308e-7 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.4) - 0.055);
const POW_N = 1024;
const powLuts = /* @__PURE__ */ new Map();
function powLut(e) {
  let t = powLuts.get(e);
  if (!t) {
    t = new Float32Array(POW_N + 1);
    for (let i = 0; i <= POW_N; i++) t[i] = Math.pow(i / POW_N, e);
    if (powLuts.size > 16) powLuts.clear();
    powLuts.set(e, t);
  }
  return t;
}
function vivify(c, v, cap = Infinity) {
  if (!(v > 0)) return;
  const y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  const mn = Math.min(c[0], c[1], c[2]), mx = Math.max(c[0], c[1], c[2]);
  let k = 1 + 3 * v;
  if (mn < y) k = Math.min(k, y / (y - mn));
  if (mx > y) k = Math.min(k, Math.max(1, (cap - y) / (mx - y)));
  c[0] = y + (c[0] - y) * k;
  c[1] = y + (c[1] - y) * k;
  c[2] = y + (c[2] - y) * k;
}
const ENV = [0.92, 0.96, 1];
const WARM = [1, 0.98, 0.95];
const smooth = (a, b, v) => {
  const t = v <= a ? 0 : v >= b ? 1 : (v - a) / (b - a);
  return t * t * (3 - 2 * t);
};
const soft = (w, s, x) => 1 - smooth(w - s, w + s, x);
function buildMatcap(out, c, f, p) {
  const { L, V, H, U, W, A, B } = f;
  const vivid = p.vivid ?? 0, px = [0, 0, 0];
  const mx = Math.max(c[0], c[1], c[2], 0.05);
  const tint = [c[0] / mx, c[1] / mx, c[2] / mx];
  const amb = Math.max(0.03, 0.3 - 0.15 * p.shadow);
  const wrap = 0.15 + 0.14 * p.spread;
  const kd = 0.85;
  const e1 = Math.min(90, Math.round(110 / Math.pow(p.spread, 1.3))), e2 = Math.max(2, Math.round(8 / p.spread));
  const lut1 = powLut(e1), lut2 = powLut(e2);
  const ks1 = 0.45 * p.highlight, ks2 = 0.1 * p.highlight, winK = 0.11 * p.highlight, rimK = 0.3 * p.rim;
  const ambT = [amb * tint[0], amb * tint[1], amb * tint[2]];
  for (let j = 0; j < M; j++) {
    for (let i = 0; i < M; i++) {
      let nx = i / (M - 1) * 2 - 1, ny = j / (M - 1) * 2 - 1;
      let r2 = nx * nx + ny * ny;
      if (r2 > 1.14) continue;
      if (r2 > 1) {
        const s = 1 / Math.sqrt(r2);
        nx *= s;
        ny *= s;
        r2 = 1;
      }
      const nz = Math.sqrt(1 - r2);
      const nl = nx * L[0] + ny * L[1] + nz * L[2];
      const nv = Math.max(0, nx * V[0] + ny * V[1] + nz * V[2]);
      const nh = Math.max(0, nx * H[0] + ny * H[1] + nz * H[2]);
      const dif = Math.min(1, Math.max(0, (nl + wrap) / (1 + wrap)));
      const q = 1 - nv, q2 = q * q, f3 = q2 * q, f5 = f3 * q2;
      const ni = nh * POW_N | 0;
      const spec = (ks1 * lut1[ni] + ks2 * lut2[ni]) * (1 + 3 * f5);
      const rx = 2 * nv * nx - V[0], ry = 2 * nv * ny - V[1], rz = 2 * nv * nz - V[2];
      const sky = 0.45 + 0.55 * smooth(-0.4, 0.6, rx * U[0] + ry * U[1] + rz * U[2]);
      const rw = rx * W[0] + ry * W[1] + rz * W[2];
      let win = 0;
      if (rw > 0.5) {
        const ra = (rx * A[0] + ry * A[1] + rz * A[2]) / rw, rb = (rx * B[0] + ry * B[1] + rz * B[2]) / rw;
        win = soft(0.34, 0.12, Math.abs(ra)) * soft(0.12, 0.06, Math.abs(rb));
      }
      const env = rimK * f3 * sky + winK * win;
      const k = (j * M + i) * 3;
      px[0] = c[0] * (ambT[0] + kd * dif);
      px[1] = c[1] * (ambT[1] + kd * dif);
      px[2] = c[2] * (ambT[2] + kd * dif);
      vivify(px, vivid);
      out[k] = tone(px[0] + spec * WARM[0] + env * ENV[0]);
      out[k + 1] = tone(px[1] + spec * WARM[1] + env * ENV[1]);
      out[k + 2] = tone(px[2] + spec * WARM[2] + env * ENV[2]);
    }
  }
}
const lifts = /* @__PURE__ */ new WeakMap();
function edgeLift(form) {
  let l = lifts.get(form);
  if (l) return l;
  const { N, sd, ao } = form;
  l = { ao: new Uint8Array(N * N), k: new Float32Array(N * N) };
  for (let i = 0; i < N * N; i++) {
    if (ao[i] === 0) continue;
    const dd = Math.max(0, sd[i]);
    const e = 1 - Math.min(1, dd / 3);
    const edge = 1 - 0.2 * e * e;
    const lin = Math.min(1, Math.pow(ao[i] / 255, 2.2) / edge);
    l.ao[i] = Math.max(1, Math.round(255 * Math.pow(lin, 1 / 2.2)));
    l.k[i] = 1;
  }
  lifts.set(form, l);
  return l;
}
const FRONT_FADE = 5;
function shadeTexels(form, mc, px, aoMul, lift) {
  const { N, i00, wx, wy, ao, sd } = form;
  const R = M * 3;
  for (let i = 0, k = 0; i < N * N; i++, k += 4) {
    const a = ao[i];
    if (a === 0) {
      px[k + 3] = 0;
      continue;
    }
    let m = lift ? aoMul[lift.ao[i]] * lift.k[i] : aoMul[a];
    const b = i00[i] * 3, x = wx[i] * (1 / 255), y = wy[i] * (1 / 255);
    if (lift) {
      const g = (mc[b + 1] * (1 - x) + mc[b + 4] * x) * (1 - y) + (mc[b + R + 1] * (1 - x) + mc[b + R + 4] * x) * y;
      m = 1 - (1 - m) * (1.1 - 0.75 * Math.min(1, g / 235));
    }
    const w00 = (1 - x) * (1 - y) * m, w10 = x * (1 - y) * m, w01 = (1 - x) * y * m, w11 = x * y * m;
    px[k] = mc[b] * w00 + mc[b + 3] * w10 + mc[b + R] * w01 + mc[b + R + 3] * w11;
    px[k + 1] = mc[b + 1] * w00 + mc[b + 4] * w10 + mc[b + R + 1] * w01 + mc[b + R + 4] * w11;
    px[k + 2] = mc[b + 2] * w00 + mc[b + 5] * w10 + mc[b + R + 2] * w01 + mc[b + R + 5] * w11;
    const t = sd[i] / FRONT_FADE;
    px[k + 3] = t >= 1 ? 255 : t <= 0 ? 0 : Math.round(255 * t * t * (3 - 2 * t));
  }
}
const softs = /* @__PURE__ */ new WeakMap();
function softMask(form, R) {
  let byR = softs.get(form);
  if (!byR) softs.set(form, byR = /* @__PURE__ */ new Map());
  if (byR.has(R)) return byR.get(R);
  const c = makeCanvas(R), g = c && ctx2d(c, false);
  if (!c || !g) {
    byR.set(R, null);
    return null;
  }
  const { N, sd } = form;
  const img = new ImageData(R, R), px = R / SPAN;
  for (let y = 0; y < R; y++) {
    for (let x = 0; x < R; x++) {
      const d = bilerp(sd, N, (x + 0.5) / px / SPAN * N - 0.5, (y + 0.5) / px / SPAN * N - 0.5);
      const t = (d + 0.2) / 0.6;
      const k = (y * R + x) * 4;
      img.data[k] = img.data[k + 1] = img.data[k + 2] = 255;
      img.data[k + 3] = t >= 1 ? 255 : t <= 0 ? 0 : Math.round(255 * t * t * (3 - 2 * t));
    }
  }
  g.putImageData(img, 0, 0);
  byR.set(R, c);
  return c;
}
const rims = /* @__PURE__ */ new WeakMap();
function rimOf(form) {
  let r = rims.get(form);
  if (r) return r;
  const { N, sd } = form;
  const u = SPAN / N;
  const pts = [];
  for (let j = 0, i = 0; j < N; j++) for (let x = 0; x < N; x++, i++) if (sd[i] >= 0 && sd[i] < 2 * u) pts.push((x + 0.5) * u - PAD, (j + 0.5) * u - PAD, sd[i]);
  r = Float32Array.from(pts);
  rims.set(form, r);
  return r;
}
const rings = /* @__PURE__ */ new WeakMap();
function ringOf(form) {
  let r = rings.get(form);
  if (r) return r;
  const rim = rimOf(form);
  const all = [];
  let mx = 0, my = 0;
  for (let i = 0; i < rim.length; i += 3) mx += rim[i], my += rim[i + 1];
  mx /= rim.length / 3 || 1;
  my /= rim.length / 3 || 1;
  for (let i = 0; i < rim.length; i += 3) all.push([rim[i], rim[i + 1], Math.atan2(rim[i + 1] - my, rim[i] - mx), rim[i + 2]]);
  all.sort((p, q) => p[2] - q[2]);
  const step = Math.max(1, Math.floor(all.length / 96));
  const pts = all.filter((_, i) => i % step === 0);
  r = { x: Float32Array.from(pts, (p) => p[0]), y: Float32Array.from(pts, (p) => p[1]), d: Float32Array.from(pts, (p) => p[3]) };
  rings.set(form, r);
  return r;
}
function ringOrder(n, a, k) {
  const start2 = Math.round((a + Math.PI) / (2 * Math.PI) * n);
  const off = k & 1 ? k + 1 >> 1 : -(k >> 1);
  return ((start2 + off) % n + n) % n;
}
function reach(rim, ex, ey) {
  let m = -INF;
  for (let i = 0; i < rim.length; i += 3) {
    const v = rim[i] * ex + rim[i + 1] * ey + rim[i + 2];
    if (v > m) m = v;
  }
  return m;
}
const furs = /* @__PURE__ */ new Map();
const furPending = /* @__PURE__ */ new Set();
const lightBin = (lx, ly) => Math.round(Math.atan2(ly, lx) / (Math.PI / 12));
const furTier = (capPx) => capPx <= 200 ? 192 : capPx <= 360 ? 320 : 512;
const FUR_STOCK = { length: 1, density: 1.6, fuzz: 0.9, clumps: 0.4, curl: 0.7, gravity: 0.9 };
const styleId = (s) => `${s.length.toFixed(2)},${s.density.toFixed(2)},${s.fuzz.toFixed(2)},${s.clumps.toFixed(2)},${s.curl.toFixed(2)},${(s.gravity ?? FUR_STOCK.gravity).toFixed(2)}`;
const furKey = (key, halfDepth, R, bin, style, lights) => `${key}|${Math.round(halfDepth)}|${R}|${bin}|${styleId(style)}|${lightsId(lights)}`;
const queuedFor = /* @__PURE__ */ new Map();
const dropped = /* @__PURE__ */ new Set();
const FUR_MID = 128;
const LOCK_COVER = 3.2;
const LOCK_DENSE = 1 / 2.56;
const LOCK_TAPER = 1 / (2 / 3 * Math.sqrt(1 / 3));
const LOCK_PART = 0.6;
const PILE_AMB = 0.07;
const PILE_FILL = 0.14;
const PILE_WRAP = 0.3;
const PILE_SHADOW = 0.6;
const PILE_AO = 0.35;
const PILE_ROOT = 0.3;
const PILE_KK = 0.3;
const PILE_GAIN = 1.26;
const PILE_LIFT = 1.6;
const PILE_BIAS = 0.045;
const PILE_STAND = 0.5;
const PILE_COS = Math.cos(PILE_STAND), PILE_SIN = Math.sin(PILE_STAND);
const PILE_CREASE = 0.22;
const PILE_H = 0.5;
const GAP_REF = 0.36;
const PILE_LIT = 1.1;
const TIP_TOP = 1.625;
const STRAND_TAPER = 0.5;
const STRAND_RISE = 0.9;
const STRAND_SHORT = 0.7;
const STRAND_CAST_H = 0.15;
const STRAND_CAST = 1.3;
const FILM_M = 20;
const FILM_SPAN = SPAN + 2 * FILM_M;
const HALO_A = 0.8;
const HALO_STREAK = 0.3;
const HALO_HAIR = 0.8;
const HALO_LIFT = 0.7;
const HALO_SHADE = 0.5;
const HALO_FIT = 2.5;
const HALO_CORE = 1.2;
const HALO_INSET = 3;
const HALO_VEIL = 0.8;
const HALO_BLEND = 0.3;
const HALO_EASE = 0.1;
const KEY_EL = 32 * Math.PI / 180;
const BACK_SOFT = 1;
function fabricKey(U, V, front = KEY_EL) {
  const c = Math.cos(front), s = Math.sin(front);
  return norm3([U[0] * c + V[0] * s, U[1] * c + V[1] * s, U[2] * c + V[2] * s]);
}
function fabricBack(K, V) {
  return norm3([0.94 * K[0] - 0.34 * V[0], 0.94 * K[1] - 0.34 * V[1], 0.94 * K[2] - 0.34 * V[2]]);
}
function backFromKey(lx, ly) {
  const x = -0.5 * lx, y = -0.5 * ly - 0.87;
  const l = Math.hypot(x, y) || 1;
  return [x / l, y / l];
}
const stockLights = (lx, ly) => {
  const [bx, by] = backFromKey(lx, ly);
  return { bx, by, front: KEY_EL };
};
const lightsId = (s) => `${Math.round(Math.atan2(s.by, s.bx) / (Math.PI / 36))}|${Math.round(s.front / (Math.PI / 90))}`;
function hash2(x, y) {
  let h = x * 374761393 + y * 668265263 | 0;
  h = Math.imul(h ^ h >>> 13, 1274126177);
  return ((h ^ h >>> 16) >>> 0) / 4294967296;
}
function bilerp(f, N, x, y) {
  const last2 = N - 1;
  const fx = Math.min(last2, Math.max(0, x)), fy = Math.min(last2, Math.max(0, y));
  const x0 = Math.min(last2 - 1, fx | 0), y0 = Math.min(last2 - 1, fy | 0);
  const tx = fx - x0, ty = fy - y0, i = y0 * N + x0;
  return (f[i] * (1 - tx) + f[i + 1] * tx) * (1 - ty) + (f[i + N] * (1 - tx) + f[i + N + 1] * tx) * ty;
}
function furReady(form, key, halfDepth, capPx, sync, lx, ly, style = FUR_STOCK, lights = stockLights(lx, ly)) {
  const R = furTier(capPx), bin = lightBin(lx, ly);
  const id = furKey(key, halfDepth, R, bin, style, lights);
  const hit = furs.get(id);
  if (hit) return hit;
  if (sync) return furFor(form, key, halfDepth, capPx, lx, ly, style, lights);
  const prefix = `${key}|${Math.round(halfDepth)}|`;
  if (R > 192 && !hurrying && !furPending.has(id)) {
    let any = false;
    for (const k of furs.keys()) if (k.startsWith(prefix)) any = true;
    for (const k of furPending) if (k.startsWith(prefix)) any = true;
    if (!any) furReady(form, key, halfDepth, 1, false, lx, ly, style, lights);
  }
  if (!furPending.has(id)) {
    furPending.add(id);
    dropped.delete(id);
    const shape = `${key}|${Math.round(halfDepth)}|${R}|${bin}`;
    const line = queuedFor.get(shape) ?? [];
    line.push(id);
    while (line.length > 2) {
      const old = line.shift();
      dropped.add(old);
      furPending.delete(old);
    }
    queuedFor.set(shape, line);
    const job = furJob(form, id, R, lx, ly, style, lights);
    if (job) for (const step of job.steps) idle(() => dropped.has(id) ? void 0 : step());
  }
  missed = true;
  for (const [k, f] of furs) if (k.startsWith(prefix)) return f;
  return null;
}
function rng$1(seed) {
  let a = seed >>> 0;
  return () => {
    a = a + 1831565813 | 0;
    let t = Math.imul(a ^ a >>> 15, 1 | a);
    t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
    return ((t ^ t >>> 14) >>> 0) / 4294967296;
  };
}
function valueNoise(scale2, seed) {
  return (x, y) => {
    const fx = x / scale2, fy = y / scale2;
    const x0 = Math.floor(fx), y0 = Math.floor(fy);
    const tx = fx - x0, ty = fy - y0;
    const sx = tx * tx * (3 - 2 * tx), sy = ty * ty * (3 - 2 * ty);
    const h = (i, j) => hash2(i + seed * 131, j - seed * 71);
    const a = h(x0, y0), b = h(x0 + 1, y0), c = h(x0, y0 + 1), d = h(x0 + 1, y0 + 1);
    return (a + (b - a) * sx) * (1 - sy) + (c + (d - c) * sx) * sy;
  };
}
function hash3(x, y, z) {
  let h = x * 374761393 + y * 668265263 + z * 1274126177 | 0;
  h = Math.imul(h ^ h >>> 13, 1274126177);
  return ((h ^ h >>> 16) >>> 0) / 4294967296;
}
function valueNoise3(scale2, seed) {
  return (x, y, z) => {
    const fx = x / scale2, fy = y / scale2, fz = z / scale2;
    const x0 = Math.floor(fx), y0 = Math.floor(fy), z0 = Math.floor(fz);
    const tx = fx - x0, ty = fy - y0, tz = fz - z0;
    const sx = tx * tx * (3 - 2 * tx), sy = ty * ty * (3 - 2 * ty), sz = tz * tz * (3 - 2 * tz);
    const h = (i, j, k) => hash3(i + seed * 131, j - seed * 71, k + seed * 17);
    const lerp = (a, b, t) => a + (b - a) * t;
    const c00 = lerp(h(x0, y0, z0), h(x0 + 1, y0, z0), sx), c10 = lerp(h(x0, y0 + 1, z0), h(x0 + 1, y0 + 1, z0), sx);
    const c01 = lerp(h(x0, y0, z0 + 1), h(x0 + 1, y0, z0 + 1), sx), c11 = lerp(h(x0, y0 + 1, z0 + 1), h(x0 + 1, y0 + 1, z0 + 1), sx);
    return lerp(lerp(c00, c10, sy), lerp(c01, c11, sy), sz);
  };
}
function furFor(form, key = "custom", halfDepth = 9.75, capPx = 320, lx = -1, ly = 0, style = FUR_STOCK, lights = stockLights(lx, ly)) {
  const R = furTier(capPx);
  const id = furKey(key, halfDepth, R, lightBin(lx, ly), style, lights);
  const hit = furs.get(id);
  if (hit) return hit;
  const job = furJob(form, id, R, lx, ly, style, lights);
  if (job) for (const step of job.steps) step();
  return furs.get(id) ?? { R: 0, mask: null, haze: null, cut: null, core: null, dark: null, light: null, glint: null, back: null, backSoft: null };
}
function furJob(form, id, R, lx, ly, style = FUR_STOCK, lights = stockLights(lx, ly)) {
  if (furs.has(id)) return null;
  const file = (f) => {
    if (furs.size >= 24) furs.delete(furs.keys().next().value);
    furs.set(id, f);
    furPending.delete(id);
    land();
  };
  const { N } = form;
  const px = R / SPAN;
  const at = (f, x, y) => bilerp(f, N, (x + PAD) / SPAN * N - 0.5, (y + PAD) / SPAN * N - 0.5);
  const sdAt = (x, y) => at(form.sd, x, y);
  const O = PAD + FILM_M, Rb = Math.round(R * FILM_SPAN / SPAN);
  const fur = { R, mask: null, haze: null, cut: null, core: null, dark: null, light: null, glint: null, back: null, backSoft: null };
  const dc = makeCanvas(Rb), lc = makeCanvas(Rb), mc = makeCanvas(R), gc = makeCanvas(Rb), bc = makeCanvas(Rb), sc2 = makeCanvas(Rb);
  const dg = dc && ctx2d(dc, false), lg = lc && ctx2d(lc, false), mg = mc && ctx2d(mc, false);
  const gg = gc && ctx2d(gc, false), bg = bc && ctx2d(bc, false), sg2 = sc2 && ctx2d(sc2, false);
  if (!dc || !lc || !mc || !gc || !bc || !sc2 || !dg || !lg || !mg || !gg || !bg || !sg2) {
    file(fur);
    return null;
  }
  const rand = rng$1(N * 7919 + 17);
  const lean = valueNoise(3.4, 21), tone2 = valueNoise(22, 9);
  const swirl = valueNoise(13, 57);
  const { length: kLen, density: kDen, fuzz: fuzz2, clumps, curl } = style;
  const gravity = style.gravity ?? FUR_STOCK.gravity;
  const ll = Math.hypot(lx, ly) || 1;
  const Lx = lx / ll, Ly = ly / ll;
  const Uf = [Lx, Ly, 0], Vf = [0, 0, 1], Df = [0, 1, 0];
  const K3 = fabricKey(Uf, Vf, lights.front), B3 = fabricBack([lights.bx, lights.by, 0], Vf), H3 = norm3([K3[0], K3[1], K3[2] + 1]);
  const F3 = norm3([-0.55 * Uf[0] + 0.85 * Vf[0] + 0.15 * Df[0], -0.55 * Uf[1] + 0.85 * Vf[1] + 0.15 * Df[1], -0.55 * Uf[2] + 0.85 * Vf[2] + 0.15 * Df[2]]);
  let crownY = 12;
  for (let y = -PAD; y < 60; y += 0.5) if (sdAt(50, y) > 0.5) {
    crownY = y + 1;
    break;
  }
  crownY -= 18;
  const crownX = 50;
  const outward = (x, y) => {
    const e = 0.8;
    const ox = sdAt(x - e, y) - sdAt(x + e, y), oy = sdAt(x, y - e) - sdAt(x, y + e);
    const ol = Math.hypot(ox, oy) || 1;
    return [ox / ol, oy / ol];
  };
  const flow = (x, y) => {
    let rx = x - crownX, ry = y - crownY;
    const rl = Math.hypot(rx, ry) || 1;
    rx /= rl;
    ry /= rl;
    const fall = gravity * (1 - 0.6 * Math.exp(-rl / 8));
    let dx = rx * (1 - 0.8 * fall), dy = ry * (1 - 0.8 * fall) + fall;
    const d0 = sdAt(x, y);
    if (d0 < 5) {
      const [ox, oy] = outward(x, y);
      const up = Math.max(0, -oy);
      const ex = ox * (1 - gravity * up), ey = oy * (1 - gravity * up) + 1.2 * gravity;
      const w = Math.max(0, Math.min(1, 1 - d0 / 5)) * 0.75;
      dx = dx * (1 - w) + ex * w * 1.6;
      dy = dy * (1 - w) + ey * w * 1.6;
    }
    const a = (lean(x, y) - 0.5) * (0.5 + 1.6 * curl) + (swirl(x, y) - 0.5) * (0.6 + curl);
    const ca = Math.cos(a), sa = Math.sin(a);
    const qx = dx * ca - dy * sa, qy = dx * sa + dy * ca;
    const m = Math.hypot(qx, qy) || 1;
    return [qx / m, qy / m];
  };
  const steps = [];
  const rows = (n, chunk, fn) => {
    for (let y0 = 0; y0 < n; y0 += chunk) steps.push(() => fn(y0, Math.min(n, y0 + chunk)));
  };
  const hAt = (x, y) => at(form.h, x, y);
  const slopeAt = (x, y) => {
    const e = 0.6;
    const gx = (hAt(x + e, y) - hAt(x - e, y)) / (2 * e), gy = (hAt(x, y + e) - hAt(x, y - e)) / (2 * e);
    const m = Math.hypot(gx, gy);
    return [m > 1e-6 ? -gx / m : 0, m > 1e-6 ? -gy / m : 0, m / Math.sqrt(1 + m * m)];
  };
  const G = 112, gu = FILM_SPAN / G;
  const flowX = new Float32Array(G * G), flowY = new Float32Array(G * G), tiltG = new Float32Array(G * G);
  const nXg = new Float32Array(G * G), nYg = new Float32Array(G * G), nZg = new Float32Array(G * G);
  const onG = new Float32Array(G * G);
  const pfX = new Float32Array(G * G), pfY = new Float32Array(G * G), foreG = new Float32Array(G * G);
  const hG = new Float32Array(G * G), creaseG = new Float32Array(G * G);
  const toneG = new Float32Array(G * G);
  rows(G, 28, (j0, j1) => {
    for (let j = j0, i = j0 * G; j < j1; j++) {
      for (let x = 0; x < G; x++, i++) {
        const X = (x + 0.5) * gu - O, Y = (j + 0.5) * gu - O;
        let [fx, fy] = flow(X, Y);
        const [dx, dy, tiltS] = slopeAt(X, Y);
        const w = tiltS * (0.25 + 0.5 * (1 - gravity)) * (1 - gravity * Math.max(0, -dy));
        fx = fx * (1 - w) + dx * w;
        fy = fy * (1 - w) + (dy + 0.35 * gravity) * w;
        const fl = Math.hypot(fx, fy) || 1;
        fx /= fl;
        fy /= fl;
        flowX[i] = fx;
        flowY[i] = fy;
        tiltG[i] = tiltS;
        let nx = dx * tiltS, ny = dy * tiltS, nz = Math.sqrt(Math.max(0, 1 - tiltS * tiltS));
        const d0 = sdAt(X, Y);
        if (d0 < 2) {
          const [ox, oy] = outward(X, Y);
          const w2 = d0 >= 0 ? 0.7 * (1 - d0 / 2) : Math.min(1, 0.7 - 0.2 * d0);
          nx += w2 * (ox - nx);
          ny += w2 * (oy - ny);
          nz -= w2 * nz;
          const nl = Math.hypot(nx, ny, nz) || 1;
          nx /= nl;
          ny /= nl;
          nz /= nl;
        }
        nXg[i] = nx;
        nYg[i] = ny;
        nZg[i] = nz;
        onG[i] = smooth(-3.5, -1.5, d0);
        toneG[i] = tone2(X, Y);
        hG[i] = d0 > 0 ? hAt(X, Y) : 0;
        const s = Math.sqrt(nx * nx + ny * ny), gx = s > 1e-4 ? nx / s : 0, gy = s > 1e-4 ? ny / s : 0;
        const a = fx * gx + fy * gy, k = (1 - Math.max(0, nz)) * a;
        let qx = PILE_COS * (fx - k * gx) + PILE_SIN * s * gx, qy = PILE_COS * (fy - k * gy) + PILE_SIN * s * gy;
        const att = smooth(-0.5, 1.5, d0);
        qx = fx + (qx - fx) * att;
        qy = fy + (qy - fy) * att;
        const ql = Math.sqrt(qx * qx + qy * qy) || 1;
        pfX[i] = qx / ql;
        pfY[i] = qy / ql;
        foreG[i] = 1 + (Math.min(1, ql) - 1) * att;
      }
    }
  });
  steps.push(() => {
    const tmp = new Float32Array(G * G), avg = Float32Array.from(hG);
    for (let pass = 0; pass < 3; pass++) {
      for (let y = 0, i = 0; y < G; y++) for (let x = 0; x < G; x++, i++) {
        let acc = 0;
        for (let t = -2; t <= 2; t++) acc += avg[y * G + Math.min(G - 1, Math.max(0, x + t))];
        tmp[i] = acc / 5;
      }
      for (let y = 0, i = 0; y < G; y++) for (let x = 0; x < G; x++, i++) {
        let acc = 0;
        for (let t = -2; t <= 2; t++) acc += tmp[Math.min(G - 1, Math.max(0, y + t)) * G + x];
        avg[i] = acc / 5;
      }
    }
    for (let j = 0, i = 0; j < G; j++) for (let x = 0; x < G; x++, i++) {
      const inside = smooth(0.2, 2, sdAt((x + 0.5) * gu - O, (j + 0.5) * gu - O));
      const valley = smooth(0.3, 3.5, avg[i] - hG[i]) * inside;
      const under = smooth(0.15, 0.85, nYg[i]) * (1 - nZg[i]);
      creaseG[i] = Math.min(1, valley + 0.45 * under) * onG[i];
    }
  });
  const gridAt = (f, X, Y) => bilerp(f, G, (X + O) / gu - 0.5, (Y + O) / gu - 0.5);
  const dirAt = (X, Y) => {
    const ux = gridAt(pfX, X, Y), uy = gridAt(pfY, X, Y);
    const ul = Math.sqrt(ux * ux + uy * uy);
    return ul > 1e-4 ? [ux / ul, uy / ul] : [0, 1];
  };
  const freeDirAt = (X, Y) => {
    const ux = gridAt(flowX, X, Y), uy = gridAt(flowY, X, Y);
    const ul = Math.sqrt(ux * ux + uy * uy);
    return ul > 1e-4 ? [ux / ul, uy / ul] : [0, 1];
  };
  const Rl = Rb >> 1, pl = Rl / FILM_SPAN;
  const lock = new Float32Array(Rl * Rl), lockLit = new Float32Array(Rl * Rl);
  const lockAt = (f, X, Y) => bilerp(f, Rl, (X + O) * pl - 0.5, (Y + O) * pl - 0.5);
  const bump = valueNoise(0.7 + 0.35 * kLen, 61), swell = valueNoise(2.2 + 0.6 * kLen, 67);
  const tuftK = (0.2 + 0.5 * fuzz2) * Math.min(1.8, kLen);
  const tuftAt = (X, Y) => {
    const v = 0.3 * Math.max(-1.6, Math.min(1.6, lockAt(lock, X, Y))) / 1.6 + 0.25 * (bump(X, Y) - 0.5) * 2.4 + 0.45 * (swell(X, Y) - 0.5) * 2.4;
    return tuftK * Math.max(-1, Math.min(1, v));
  };
  const lockW = 0.8 + 0.45 * kLen;
  const lockWd = lockW * (0.65 + 0.4 * clumps), lockLd = 1.5 + 1.6 * kLen;
  const hMax = 0.32 * lockWd * (0.15 + 0.85 * clumps);
  const pileH = new Float32Array(Rb * Rb).fill(-0.25 * hMax), own = new Int32Array(Rb * Rb).fill(-1);
  const LP = 16;
  const cell = Math.sqrt(0.6 * lockWd * lockLd / LOCK_COVER / LOCK_DENSE);
  const gn = Math.ceil(FILM_SPAN / cell);
  const locks = new Float32Array(gn * gn * LP);
  let nLocks = 0;
  const stand0 = 0.45 - 0.2 * gravity;
  const tuftN = valueNoise3(1.6 * lockLd, 79), sizeN = valueNoise(9, 73);
  const tuftH = 1.5 * hMax * (0.25 + clumps);
  steps.push(() => {
    for (let gy = 0; gy < gn; gy++) for (let gx = 0; gx < gn; gx++) {
      const X = (gx + rand()) * cell - O, Y = (gy + rand()) * cell - O;
      const r1 = rand(), r2 = rand(), r3 = rand(), r4 = rand(), r5 = rand(), r6 = rand(), r7 = rand();
      let nx = gridAt(nXg, X, Y), ny = gridAt(nYg, X, Y), nz = gridAt(nZg, X, Y);
      const nl = Math.hypot(nx, ny, nz) || 1;
      nx /= nl;
      ny /= nl;
      nz /= nl;
      const [fx, fy] = freeDirAt(X, Y);
      const ja = (r1 - 0.5) * (0.2 + 0.45 * curl), cj = Math.cos(ja), sj = Math.sin(ja);
      const ux = fx * cj - fy * sj, uy = fx * sj + fy * cj;
      const a = ux * nx + uy * ny;
      let tx = ux - a * nx, ty = uy - a * ny, tz = -a * nz;
      let tl = Math.hypot(tx, ty, tz);
      if (tl < 0.05) {
        tx = nz * nx;
        ty = nz * ny;
        tz = nz * nz - 1;
        tl = Math.hypot(tx, ty, tz) || 1;
      }
      tx /= tl;
      ty /= tl;
      tz /= tl;
      let bx = ny * tz - nz * ty, by = nz * tx - nx * tz, bz = nx * ty - ny * tx;
      const st = stand0 + 0.2 * r2, cs = Math.cos(st), sn = Math.sin(st);
      const grow = 0.8 + 0.45 * sizeN(X, Y);
      const Wd = lockWd * grow * (0.8 + 0.4 * r3), Ld = lockLd * grow * (0.75 + 0.5 * r4);
      let ax = (tx * cs + nx * sn) * Ld * px, ay = (ty * cs + ny * sn) * Ld * px;
      let La = Math.hypot(ax, ay);
      if (La < 1.5) {
        ax = ux;
        ay = uy;
        La = 1.5;
      } else {
        ax /= La;
        ay /= La;
      }
      let across = -bx * ay + by * ax;
      if (across < 0) {
        bx = -bx;
        by = -by;
        bz = -bz;
        across = -across;
      }
      const Wa = Math.max(0.6, 0.5 * Wd * px * Math.max(0.15, across));
      const seen2 = La / (Ld * px) * (Wa / (0.5 * Wd * px));
      const tn = tuftN(X, Y, hAt(X, Y));
      if (r5 > LOCK_DENSE / Math.max(LOCK_DENSE, seen2) * (1 - LOCK_PART * clumps * (1 - smooth(0.25, 0.65, tn)))) continue;
      const b = nLocks++ * LP;
      locks[b] = (X + O) * px;
      locks[b + 1] = (Y + O) * px;
      locks[b + 2] = ax;
      locks[b + 3] = ay;
      locks[b + 4] = La;
      locks[b + 5] = Wa;
      locks[b + 6] = 0.1 * hMax * r6 + tuftH * tn;
      locks[b + 7] = (r7 - 0.5) * 1.2 * curl;
      locks[b + 8] = tx;
      locks[b + 9] = ty;
      locks[b + 10] = tz;
      locks[b + 11] = bx;
      locks[b + 12] = by;
      locks[b + 13] = bz;
      locks[b + 14] = Ld;
      locks[b + 15] = Wd;
    }
  });
  const lockAlong = (s) => (s < 0.25 ? 4 * s : 1) * (1 - 0.5 * s);
  const SPLAT_PARTS = 6;
  for (let part = 0; part < SPLAT_PARTS; part++) steps.push(() => {
    const k0 = Math.floor(nLocks * part / SPLAT_PARTS), k1 = Math.floor(nLocks * (part + 1) / SPLAT_PARTS);
    for (let k = k0; k < k1; k++) {
      const b = k * LP;
      const ox = locks[b], oy = locks[b + 1], ax = locks[b + 2], ay = locks[b + 3], La = locks[b + 4], Wa = locks[b + 5], z0 = locks[b + 6], bend = locks[b + 7];
      const ext = Wa * (1 + Math.abs(bend)) + 1, ex = ox + ax * La, ey = oy + ay * La;
      const xa = Math.max(0, Math.floor(Math.min(ox, ex) - ext)), xb = Math.min(Rb - 1, Math.ceil(Math.max(ox, ex) + ext));
      const ya = Math.max(0, Math.floor(Math.min(oy, ey) - ext)), yb = Math.min(Rb - 1, Math.ceil(Math.max(oy, ey) + ext));
      const iLa = 1 / La, iWa = 1 / Wa;
      for (let y = ya; y <= yb; y++) {
        const dy = y + 0.5 - oy;
        for (let x = xa, i = y * Rb + xa; x <= xb; x++, i++) {
          const dx = x + 0.5 - ox;
          const s = (dx * ax + dy * ay) * iLa;
          if (s <= 0 || s >= 1) continue;
          const w = LOCK_TAPER * Math.sqrt(s) * (1 - s);
          const v = (dy * ax - dx * ay) * iWa - bend * s * s;
          if (v >= w || v <= -w) continue;
          const q = v / w;
          const h = z0 + hMax * lockAlong(s) * (1 - q * q);
          if (h > pileH[i]) {
            pileH[i] = h;
            own[i] = k;
          }
        }
      }
    }
  });
  const pileB = new Float32Array(Rb * Rb);
  const rB = Math.max(1, Math.round(0.3 * lockWd * px));
  const line = new Float32Array(Rb);
  const box = (src, dst, r, k, stride) => {
    const step = stride === 1 ? Rb : 1, inv = 1 / (2 * r + 1);
    const at2 = (t) => line[t < 0 ? 0 : t >= Rb ? Rb - 1 : t];
    const o = k * step;
    for (let t = 0; t < Rb; t++) line[t] = src[o + t * stride];
    let acc = 0;
    for (let t = -r; t <= r; t++) acc += at2(t);
    for (let t = 0; t < Rb; t++) {
      dst[o + t * stride] = acc * inv;
      acc += at2(t + r + 1) - at2(t - r);
    }
  };
  steps.push(() => {
    for (let y = 0; y < Rb; y++) box(pileH, pileB, rB, y, 1);
    for (let y = 0; y < Rb; y++) box(pileB, pileB, rB, y, 1);
  });
  steps.push(() => {
    for (let x = 0; x < Rb; x++) box(pileB, pileB, rB, x, Rb);
    for (let x = 0; x < Rb; x++) box(pileB, pileB, rB, x, Rb);
  });
  steps.push(() => {
    for (let y = 0; y < Rb; y++) box(pileH, pileH, 1, y, 1);
    for (let x = 0; x < Rb; x++) box(pileH, pileH, 1, x, Rb);
  });
  const darkImg = new ImageData(Rb, Rb), lightImg = new ImageData(Rb, Rb);
  const glintImg = new ImageData(Rb, Rb), backImg = new ImageData(Rb, Rb), softImg = new ImageData(Rb, Rb);
  const white = new Float32Array(Rb * Rb);
  const fib = new Float32Array(11);
  const fibreLight = (X, Y, ux, uy, stand) => {
    const fx = Math.min(G - 1, Math.max(0, (X + O) / gu - 0.5)), fy = Math.min(G - 1, Math.max(0, (Y + O) / gu - 0.5));
    const x0 = Math.min(G - 2, fx | 0), y0 = Math.min(G - 2, fy | 0);
    const u = fx - x0, v = fy - y0, j = y0 * G + x0;
    const w00 = (1 - u) * (1 - v), w10 = u * (1 - v), w01 = (1 - u) * v, w11 = u * v;
    const read = (f) => f[j] * w00 + f[j + 1] * w10 + f[j + G] * w01 + f[j + G + 1] * w11;
    let nx = read(nXg), ny = read(nYg), nz = read(nZg);
    const nn = Math.sqrt(nx * nx + ny * ny + nz * nz) || 1;
    nx /= nn;
    ny /= nn;
    nz /= nn;
    const nK = nx * K3[0] + ny * K3[1] + nz * K3[2];
    fib[4] = nx;
    fib[5] = ny;
    fib[6] = nz;
    fib[7] = nK;
    fib[8] = read(creaseG);
    const on = read(onG);
    fib[9] = on;
    fib[10] = read(toneG);
    if (on <= 0) {
      fib[0] = fib[1] = fib[3] = 0;
      fib[2] = 0.7;
      return;
    }
    const key = smooth(-0.2, 0.5, nK);
    const along2 = ux * nx + uy * ny;
    let tx = ux - along2 * nx, ty = uy - along2 * ny, tz = -along2 * nz;
    const tl = Math.sqrt(tx * tx + ty * ty + tz * tz);
    if (tl > 1e-3) {
      tx /= tl;
      ty /= tl;
      tz /= tl;
    } else tx = 0, ty = 0, tz = 1;
    const cs = Math.cos(stand), sn = Math.sin(stand);
    tx = tx * cs + nx * sn;
    ty = ty * cs + ny * sn;
    tz = tz * cs + nz * sn;
    const tH = tx * H3[0] + ty * H3[1] + tz * H3[2], nH = nx * H3[0] + ny * H3[1] + nz * H3[2];
    const a1 = tH - 0.12 * nH, a2 = tH + 0.2 * nH;
    const b1 = Math.max(0, 1 - a1 * a1), b2 = b1 * b1, b4 = b2 * b2, b8 = b4 * b4, b16 = b8 * b8;
    const c1 = Math.max(0, 1 - a2 * a2), c2 = c1 * c1;
    fib[0] = key * (0.85 * b16 * b16 * b4 + 0.15 * c2 * c2);
    fib[0] *= (0.4 + 0.6 * (1 - smooth(0.35, 0.85, nK))) * on;
    const side = Math.sqrt(Math.max(0, 1 - tz * tz));
    fib[1] = smooth(-0.3, 0.6, nx * B3[0] + ny * B3[1] + nz * B3[2]) * Math.pow(1 - nz, 1.6) * (0.4 + 0.6 * side) * (1 - 0.6 * Math.max(0, ny)) * on;
    fib[3] = smooth(-0.5, 0.75, nx * B3[0] + ny * B3[1] + nz * B3[2]) * Math.pow(1 - nz, 0.85) * (1 - 0.6 * Math.max(0, ny)) * on;
    fib[2] = key * on + 0.7 * (1 - on);
  };
  steps.push(() => {
    for (let y = 0, i = 0; y < Rb; y++) for (let x = 0; x < Rb; x++, i++) {
      const h = hash2(x + 911, y + 37);
      white[i] = h * h * h;
    }
  });
  const W_MEAN = 0.25, W_SD = Math.sqrt(1 / 7 - 1 / 16);
  const strandHalf = Math.max(1, Math.round((0.35 + 0.4 * kLen) * px));
  const strandW = [], strandMean = new Float32Array(2 * strandHalf + 1), strandSd = new Float32Array(2 * strandHalf + 1);
  for (let L = 0; L <= 2 * strandHalf; L++) {
    const w = new Float32Array(L + 1);
    let m1 = 0, v2 = 0;
    for (let t = 0; t <= L; t++) {
      const tau = L ? t / L : 0, p = tau <= STRAND_SHORT ? 1 : (1 - tau) / (1 - STRAND_SHORT);
      w[t] = 1 - STRAND_TAPER * tau;
      m1 += w[t] * p * W_MEAN;
      v2 += w[t] * w[t] * (p / 7 - W_MEAN * W_MEAN * p * p);
    }
    strandW.push(w);
    strandMean[L] = m1;
    strandSd[L] = Math.sqrt(Math.max(1e-6, v2));
  }
  const strandK = 0.12 * (R >= 512 ? 1 : R >= 320 ? 0.75 : 0.4) * Math.min(1.4, 0.55 + 0.35 * kDen);
  const reach2 = Math.max(1, 1.5 * lockWd * px), NSH = Math.max(1, Math.min(5, Math.round(reach2)));
  const shX = new Int32Array(NSH), shY = new Int32Array(NSH), shD = new Float32Array(NSH);
  for (let t = 0; t < NSH; t++) {
    const d = (t + 1) / NSH * reach2;
    shX[t] = Math.round(Lx * d);
    shY[t] = Math.round(Ly * d);
    shD[t] = Math.hypot(shX[t], shY[t]) / px;
  }
  const difLut = powLut(1.35), gamLut = powLut(1 / 2.2);
  const keyOn = (nk) => {
    const q = (nk + PILE_WRAP) / (1 + PILE_WRAP);
    return q <= 0 ? 0 : q >= 1 ? 1 : difLut[q * POW_N | 0];
  };
  const depthK = R >= 512 ? 1 : R >= 320 ? 0.8 : 0.55;
  const seen = new Float32Array(4);
  const lockSeen = (x, y, X, Y, k) => {
    if (k < 0) {
      const [ux2, uy2] = dirAt(X, Y);
      seen[0] = ux2;
      seen[1] = uy2;
      seen[2] = 0.5;
      seen[3] = 0;
      return;
    }
    const b = k * LP;
    const ax = locks[b + 2], ay = locks[b + 3], La = locks[b + 4], Wa = locks[b + 5], bend = locks[b + 7];
    const dx = x + 0.5 - locks[b], dy = y + 0.5 - locks[b + 1];
    const s = Math.min(0.995, Math.max(5e-3, (dx * ax + dy * ay) / La));
    const ss = Math.sqrt(s), w = LOCK_TAPER * ss * (1 - s), dw = LOCK_TAPER * (0.5 * (1 - s) / ss - ss);
    const q = Math.max(-1, Math.min(1, ((dy * ax - dx * ay) / Wa - bend * s * s) / w));
    const sl = Math.max(-1.2, Math.min(1.2, q * dw + 2 * bend * s));
    const ux = La * ax - sl * Wa * ay, uy = La * ay + sl * Wa * ax;
    const ul = Math.sqrt(ux * ux + uy * uy) || 1;
    seen[0] = ux / ul;
    seen[1] = uy / ul;
    seen[2] = s;
    seen[3] = lockAlong(s) * (1 - q * q);
  };
  const strandBuf = new Float32Array(Rb * Rb);
  rows(Rb, 32, (y0, y1) => {
    for (let y = y0; y < y1; y++) {
      const Y = (y + 0.5) / px - O;
      for (let x = 0, i = y * Rb; x < Rb; x++, i++) {
        const X = (x + 0.5) / px - O;
        lockSeen(x, y, X, Y, own[i]);
        const ux = seen[0], uy = seen[1];
        const gx = Math.min(G - 1, Math.max(0, (X + O) / gu - 0.5)), gy = Math.min(G - 1, Math.max(0, (Y + O) / gu - 0.5));
        const gx0 = Math.min(G - 2, gx | 0), gy0 = Math.min(G - 2, gy | 0), gtx = gx - gx0, gty = gy - gy0, gj = gy0 * G + gx0;
        const tilt2 = (tiltG[gj] * (1 - gtx) + tiltG[gj + 1] * gtx) * (1 - gty) + (tiltG[gj + G] * (1 - gtx) + tiltG[gj + G + 1] * gtx) * gty;
        const fore = (foreG[gj] * (1 - gtx) + foreG[gj + 1] * gtx) * (1 - gty) + (foreG[gj + G] * (1 - gtx) + foreG[gj + G + 1] * gtx) * gty;
        const L = 2 * Math.max(1, Math.min(strandHalf, Math.round(strandHalf * (0.5 + 0.5 * tilt2) * fore / PILE_COS)));
        const sw = strandW[L];
        let acc = 0, accR = 0;
        for (let t = 0; t <= L; t++) {
          const sx = Math.round(x - ux * t), sy = Math.round(y - uy * t);
          if (sx < 0 || sy < 0 || sx >= Rb || sy >= Rb) {
            acc += strandMean[L] / (L + 1);
            continue;
          }
          const m = white[sy * Rb + sx], f = m * 53.7, Ls = L * (STRAND_SHORT + (1 - STRAND_SHORT) * (f - Math.floor(f)));
          if (t <= Ls) {
            acc += m * sw[t];
            accR += m * sw[t] * t / (Ls || 1);
          }
        }
        const strand = (acc - strandMean[L]) / strandSd[L];
        const along2 = acc > 1e-4 ? accR / acc : 0.5;
        strandBuf[i] = strand + STRAND_RISE * Math.min(2.5, Math.max(0, strand)) * (along2 - 0.45);
      }
    }
  });
  let lsum = 0, lsum2 = 0;
  rows(Rb, 16, (y0, y1) => {
    const dd = darkImg.data, ld = lightImg.data, gd = glintImg.data, bd = backImg.data, sd2 = softImg.data;
    for (let y = y0; y < y1; y++) {
      const Y = (y + 0.5) / px - O;
      for (let x = 0; x < Rb; x++) {
        const X = (x + 0.5) / px - O;
        const i = y * Rb + x, k = own[i], b = k * LP;
        lockSeen(x, y, X, Y, k);
        const ux = seen[0], uy = seen[1], s = seen[2], hgt = seen[3];
        fibreLight(X, Y, ux, uy, 0.25 + 0.2 * hgt);
        const nx = fib[4], ny = fib[5], nz = fib[6], nKb = fib[7];
        const xl0 = x > 0 ? i - 1 : i, xr0 = x < Rb - 1 ? i + 1 : i, yu0 = y > 0 ? i - Rb : i, yd0 = y < Rb - 1 ? i + Rb : i;
        const gx = (pileH[xr0] - pileH[xl0]) * px / (xr0 - xl0 || 1), gy = (pileH[yd0] - pileH[yu0]) * px * Rb / (yd0 - yu0 || 1);
        const l1 = Math.sqrt(nx * nx + ny * ny);
        let t1x = 1, t1y = 0, t2x = 0, t2y = 1, t2z = 0;
        if (l1 > 1e-3) {
          t1x = -ny / l1;
          t1y = nx / l1;
          t2x = -nz * nx / l1;
          t2y = -nz * ny / l1;
          t2z = l1;
        }
        const g1 = Math.max(-1.5, Math.min(1.5, gx * t1x + gy * t1y)), g2 = Math.max(-1.5, Math.min(1.5, gx * t2x + gy * t2y));
        let mx = nx - g1 * t1x - g2 * t2x, my = ny - g1 * t1y - g2 * t2y, mz = nz - g2 * t2z;
        const ml = Math.sqrt(mx * mx + my * my + mz * mz) || 1;
        mx /= ml;
        my /= ml;
        mz /= ml;
        let kk = 1;
        if (k >= 0) {
          const tx = locks[b + 8], ty = locks[b + 9], tz = locks[b + 10];
          const fx = 0.9 * tx + 0.44 * nx, fy = 0.9 * ty + 0.44 * ny, fz = 0.9 * tz + 0.44 * nz;
          const fK = (fx * K3[0] + fy * K3[1] + fz * K3[2]) / (Math.sqrt(fx * fx + fy * fy + fz * fz) || 1);
          kk = 1 - PILE_KK + PILE_KK * Math.min(1.2, Math.sqrt(Math.max(0, 1 - fK * fK)) / 0.85);
        }
        const nKp = mx * K3[0] + my * K3[1] + mz * K3[2];
        const h0 = pileH[i];
        let sh = 1;
        if (nKb > -0.25) {
          const sinE = Math.min(0.97, Math.max(0.05, nKb)), tanE = sinE / Math.sqrt(1 - sinE * sinE);
          let occ = 0;
          for (let t = 0; t < NSH; t++) {
            const sx = x + shX[t], sy = y + shY[t];
            if (sx < 0 || sy < 0 || sx >= Rb || sy >= Rb) break;
            const o = pileH[sy * Rb + sx] - h0 - shD[t] * tanE;
            if (o > occ) occ = o;
          }
          sh = 1 - PILE_SHADOW * smooth(0, 0.35 * hMax, occ);
        }
        let ao = 1 - PILE_AO * smooth(0, 0.6 * hMax, pileB[i] - h0);
        if (k >= 0) ao *= 1 - PILE_ROOT * (1 - smooth(0, 0.35, s));
        const fillP = PILE_AMB + PILE_FILL * Math.max(0, mx * F3[0] + my * F3[1] + mz * F3[2]);
        const fillB = PILE_AMB + PILE_FILL * Math.max(0, nx * F3[0] + ny * F3[1] + nz * F3[2]);
        let r = ao * (fillP + keyOn(nKp) * sh * kk) * PILE_GAIN / (fillB + keyOn(nKb));
        if (k >= 0) r *= (1 + 0.15 * s * s) * (1 + 0.08 * (hash2(k, 5) - 0.5));
        const crease = fib[8];
        r *= 1 - PILE_CREASE * crease * (0.6 + 0.4 * smooth(0, 0.6 * hMax, pileB[i] - h0));
        r = 1 + (r - 1) * depthK;
        const on = fib[9], lq = (fillB + keyOn(nKb)) / PILE_LIT;
        const lbs = (lq >= 1 ? 1 : gamLut[lq * POW_N | 0]) * on + 0.75 * (1 - on);
        let m0 = Math.sqrt(r > 2.4 ? 2.4 : r);
        if (m0 > 1) m0 = 1 + (m0 - 1) / (1 + 2.5 * lbs * (m0 - 1));
        const strand = strandBuf[i];
        let cast = 0;
        if (nKb > 0) {
          const kx = K3[0] - nKb * nx, ky = K3[1] - nKb * ny, kt = Math.sqrt(1 - nKb * nKb);
          const reachS = kt > 1e-3 ? STRAND_CAST_H * px * Math.min(3, kt / Math.max(0.2, nKb)) / kt : 0;
          const cx = Math.min(Rb - 1.001, Math.max(0, x + kx * reachS)), cy = Math.min(Rb - 1.001, Math.max(0, y + ky * reachS));
          const cx0 = cx | 0, cy0 = cy | 0, tx = cx - cx0, ty = cy - cy0, ci2 = cy0 * Rb + cx0;
          const caster = (strandBuf[ci2] * (1 - tx) + strandBuf[ci2 + 1] * tx) * (1 - ty) + (strandBuf[ci2 + Rb] * (1 - tx) + strandBuf[ci2 + Rb + 1] * tx) * ty;
          cast = Math.max(0, caster - Math.max(0, strand)) * smooth(0, 0.3, nKb);
        }
        let m = m0 * (1 + strandK * (strand * (0.45 + 0.55 * fib[2]) * (0.75 + 0.5 * s) - STRAND_CAST * cast)) + 0.04 * (fib[10] - 0.5) + PILE_BIAS * depthK * on;
        const edgeD = Math.min(X + O, FILM_SPAN - O - X, Y + O, FILM_SPAN - O - Y);
        const fadeE = edgeD >= 8 ? 1 : edgeD <= 0 ? 0 : edgeD / 8 * (edgeD / 8) * (3 - 2 * edgeD / 8);
        m = 1 + (m - 1) * fadeE;
        const k4 = i * 4;
        if (m < 1) dd[k4 + 3] = Math.round(255 * Math.min(1, (1 - m) * lbs / Math.max(0.4, lbs - GAP_REF)));
        else {
          ld[k4] = ld[k4 + 1] = ld[k4 + 2] = 255;
          ld[k4 + 3] = Math.round(255 * Math.min(1, PILE_LIFT * (TIP_TOP - 1) * lbs / (TIP_TOP - lbs) * (m - 1) * (1 - 0.7 * crease)));
        }
        const lit = Math.max(-2, Math.min(2, 4 * (m0 - 1)));
        const xl = x >> 1, yl = y >> 1;
        if (xl < Rl && yl < Rl) {
          const j = yl * Rl + xl;
          lock[j] += 0.25 * h0;
          lockLit[j] += 0.25 * lit;
        }
        const tipK = smooth(-0.4, 1, lit) * Math.max(0, Math.min(1.4, 0.65 + 0.25 * strand));
        gd[k4] = gd[k4 + 1] = gd[k4 + 2] = bd[k4] = bd[k4 + 1] = bd[k4 + 2] = 255;
        gd[k4 + 3] = Math.round(255 * Math.min(1, fib[0] * tipK));
        bd[k4 + 3] = Math.round(255 * Math.min(1, fib[1] * Math.max(0, Math.min(1.6, 0.5 + 0.4 * strand))));
        const onPile = Math.max(0.55, Math.min(1.3, 0.85 + 0.15 * strand + 0.12 * lit));
        sd2[k4] = sd2[k4 + 1] = sd2[k4 + 2] = 255;
        sd2[k4 + 3] = Math.round(255 * Math.min(1, 0.85 * fib[3] * onPile));
      }
    }
  });
  steps.push(() => {
    const n = Rl * Rl;
    for (let i = 0; i < n; i++) {
      lsum += lock[i];
      lsum2 += lock[i] * lock[i];
    }
    const mean = lsum / n, sdv = Math.sqrt(Math.max(1e-9, lsum2 / n - mean * mean));
    for (let i = 0; i < n; i++) lock[i] = Math.max(-2.5, Math.min(2.5, (lock[i] - mean) / sdv));
  });
  steps.push(() => {
    dg.putImageData(darkImg, 0, 0);
    lg.putImageData(lightImg, 0, 0);
    gg.putImageData(glintImg, 0, 0);
    sg2.putImageData(softImg, 0, 0);
    bg.putImageData(backImg, 0, 0);
    for (const g of [dg, lg, gg, bg]) {
      g.setTransform(1, 0, 0, 1, FILM_M * px, FILM_M * px);
      g.lineCap = "round";
      g.lineJoin = "round";
    }
  });
  const SHADES = 16;
  const area = FILM_SPAN * FILM_SPAN;
  const fine = R >= 512 ? 1 : R >= 320 ? 0.45 : 0;
  const kinds = [
    { count: 0.12, len: 1.2, vary: 1, endOn: 0.6, width: 0.09, spread: 30, alpha: 0.4, deep: 1, lift: 0.25, light: 0.46, dark: 0.34, bend: 0.93, clump: [1, 0], clumpW: 0, shadow: 0, shadowW: 1, over: 0, crowd: false, short: 0 },
    { count: 0.4 * fine, len: 0.9, vary: 1.2, endOn: 0.35, width: 0.13, spread: 100, alpha: 0.26, deep: 0.2, lift: 0.18, light: 0.55, dark: 0.06, bend: 1.2, clump: [1, Math.round(1 + 2 * clumps)], clumpW: 0.08 + 0.07 * clumps, shadow: 0.25, shadowW: 2, over: 0.25, crowd: true, short: 1 }
  ];
  const shadeOf = (v) => Math.min(SHADES - 1, Math.max(0, Math.round((Math.max(-1, Math.min(1, v)) + 1) / 2 * (SHADES - 1))));
  const GLINTS = 6;
  const glintOf = (v) => Math.min(GLINTS - 1, v * GLINTS | 0);
  const SEGS = 4;
  const hx = new Float32Array(SEGS + 1), hy = new Float32Array(SEGS + 1);
  for (const L of kinds) {
    if (L.count <= 0) continue;
    const perRoot = L.clump[0] + 0.5 * L.clump[1];
    const parts = Math.max(1, Math.ceil(area * L.count * kDen * (L.crowd ? 2 : 1) * (0.5 + 0.5 * perRoot) / 9e3));
    for (let part = 0; part < parts; part++) steps.push(() => {
      const buckets = [], glints = [], backs = [], shades = [];
      for (let i = 0; i < SHADES; i++) buckets.push(new Path2D());
      for (let i = 0; i < GLINTS; i++) glints.push(new Path2D()), backs.push(new Path2D());
      for (let i = 0; i < 3; i++) shades.push(new Path2D());
      const over = new Path2D();
      const count = Math.round(area * L.count * kDen * (L.crowd ? 2 : 1) / parts);
      for (let n = 0; n < count; n++) {
        const X = rand() * FILM_SPAN - O, Y = rand() * FILM_SPAN - O;
        const r0 = rand(), r1 = rand(), r2 = rand(), r3 = rand(), r4 = rand(), r5 = rand();
        if (r5 * 8 > Math.min(X + O, FILM_SPAN - O - X, Y + O, FILM_SPAN - O - Y)) continue;
        if (L.crowd && r5 > (sdAt(X, Y) > 0 ? 0.5 / Math.max(0.25, gridAt(nZg, X, Y)) : 0.5)) continue;
        const [fx, fy] = dirAt(X, Y);
        const tiltS = gridAt(tiltG, X, Y), vl = gridAt(foreG, X, Y) / PILE_COS;
        const ang = (r1 - 0.5) * (0.6 + 0.8 * curl) * (1 - 0.5 * tiltS);
        const ca = Math.cos(ang), sa = Math.sin(ang);
        const ux = fx * ca - fy * sa, uy = fx * sa + fy * ca;
        const longer = r2 < 0.1 ? 1.35 + 0.6 * r3 : 1;
        const len = (L.len + L.vary * r3 * r3) * px * kLen * longer * vl * (1 - L.endOn + 1.25 * L.endOn * tiltS);
        fibreLight(X, Y, ux, uy, 0.1 + 0.5 * hash2(n, 7 + part));
        let v = 0.22 * lockAt(lock, X, Y) + 0.3 * lockAt(lockLit, X, Y);
        const lit = r0 < L.light * (0.4 + 0.6 * fib[2]);
        if (lit) v += 0.2 + 0.28 * r4;
        else if (r0 < L.light + L.dark) v -= 0.2 + 0.28 * r4;
        v += L.lift;
        v += 0.45 * (fib[2] - 0.5) - 0.35 * fib[8];
        if (v > 0) v *= 0.3 + 0.7 * fib[2];
        const deepen = 0.15 * (1 - fib[2]);
        const x0 = (X + PAD) * px, y0 = (Y + PAD) * px;
        const b1 = (rand() - 0.5) * L.bend * curl * len, b2 = r4 < 0.3 ? -0.6 * b1 : b1 * (0.6 + 0.8 * rand());
        const hairs = L.clump[0] + (rand() * (L.clump[1] + 1) | 0);
        const wC = L.clumpW * px * Math.min(1.5, kLen);
        for (let k = 0; k < hairs; k++) {
          const o = hairs > 1 ? rand() - 0.5 : 0, f = hairs > 1 ? 0.7 + 0.3 * rand() : 1, j = hairs > 1 ? (rand() - 0.5) * 0.3 : 0;
          const rx = x0 - uy * o * wC, ry = y0 + ux * o * wC;
          const lk = len * f, e1 = b1 + j * lk, e2 = b2 + j * lk - o * wC * 0.7;
          const c1x = rx + ux * lk * 0.33 - uy * e1, c1y = ry + uy * lk * 0.33 + ux * e1;
          const c2x = rx + ux * lk * 0.67 - uy * e2, c2y = ry + uy * lk * 0.67 + ux * e2;
          const x1 = rx + ux * lk - uy * (b2 - o * wC * 0.75), y1 = ry + uy * lk + ux * (b2 - o * wC * 0.75);
          const sg = lk < 4 ? 2 : lk < 10 ? 3 : SEGS;
          for (let s = 0; s <= sg; s++) {
            const t = s / sg, q = 1 - t;
            const a = q * q * q, b = 3 * q * q * t, c = 3 * q * t * t, d = t * t * t;
            hx[s] = a * rx + b * c1x + c * c2x + d * x1;
            hy[s] = a * ry + b * c1y + c * c2y + d * y1;
          }
          const vh = v + (hairs > 1 ? (rand() - 0.5) * 0.25 : 0), rise = 0.3 + 0.15 * fib[2];
          for (let s = 0; s < sg; s++) {
            const bk = buckets[shadeOf(vh - 0.3 - deepen * (1 - s / sg) + rise * (s + 0.5) / sg)];
            bk.moveTo(hx[s], hy[s]);
            bk.lineTo(hx[s + 1], hy[s + 1]);
          }
          if (lit && L.over > 0) {
            over.moveTo(hx[1], hy[1]);
            for (let s = 2; s <= sg; s++) over.lineTo(hx[s], hy[s]);
          }
          if (k === 0 && L.shadow > 0 && fib[2] > 0.25 && v > -0.2) {
            const kk = L.shadow * px * Math.min(1, kLen), sh = shades[Math.min(2, (fib[2] - 0.25) * 4 | 0)];
            for (let s = 1; s <= sg; s++) {
              const t = s / sg, ox = -Lx * kk * t, oy = -Ly * kk * t + 0.3 * kk * t;
              if (s === 1) sh.moveTo(hx[0], hy[0]);
              sh.lineTo(hx[s] + ox, hy[s] + oy);
            }
          }
          const h = sg >> 1;
          if (fib[0] > 0.3) {
            const gb = glints[glintOf(fib[0])];
            gb.moveTo(hx[h], hy[h]);
            for (let s = h + 1; s <= sg; s++) gb.lineTo(hx[s], hy[s]);
          }
          if (fib[1] > 0.1) {
            const bb = backs[glintOf(fib[1])];
            bb.moveTo(hx[h], hy[h]);
            for (let s = h + 1; s <= sg; s++) bb.lineTo(hx[s], hy[s]);
          }
        }
      }
      const shortK = Math.min(1, kLen) ** L.short;
      const lw = Math.max(0.45, L.width * px * (0.5 + 0.5 * shortK));
      if (L.over > 0) {
        dg.globalCompositeOperation = "destination-out";
        dg.lineWidth = lw;
        dg.globalAlpha = L.over * shortK;
        dg.strokeStyle = "#000";
        dg.stroke(over);
        dg.globalCompositeOperation = "source-over";
      }
      if (L.shadow > 0) {
        dg.lineWidth = lw * L.shadowW;
        dg.strokeStyle = "#000";
        for (let i = 0; i < 3; i++) {
          dg.globalAlpha = 0.03 * (i + 1) * shortK;
          dg.stroke(shades[i]);
        }
      }
      for (let i = 0; i < SHADES; i++) {
        const v = FUR_MID + (i / (SHADES - 1) * 2 - 1) * L.spread;
        const g = v < FUR_MID ? dg : lg;
        g.lineWidth = lw;
        g.globalAlpha = L.alpha * shortK * (v < FUR_MID ? L.deep * (1 - v / FUR_MID) : (v - FUR_MID) / (255 - FUR_MID));
        g.strokeStyle = v < FUR_MID ? "#000" : "#fff";
        g.stroke(buckets[i]);
      }
      for (const g of [gg, bg]) {
        g.lineWidth = lw;
        g.strokeStyle = "#fff";
      }
      for (let i = 0; i < GLINTS; i++) {
        gg.globalAlpha = 0.4 * (i + 0.5) / GLINTS;
        bg.globalAlpha = 0.8 * (i + 0.5) / GLINTS;
        gg.stroke(glints[i]);
        bg.stroke(backs[i]);
      }
      dg.globalAlpha = lg.globalAlpha = gg.globalAlpha = bg.globalAlpha = 1;
    });
  }
  const fringe = new Path2D();
  const fringeBack = [];
  for (let i = 0; i < GLINTS; i++) fringeBack.push(new Path2D());
  const cells = [];
  const cu = SPAN / N;
  let edgeTries = 0;
  const FRINGE_PARTS = 2;
  for (let part = 0; part < FRINGE_PARTS; part++) steps.push(() => {
    if (part === 0) {
      for (let i = 0; i < N * N; i++) if (form.sd[i] > -1.6 && form.sd[i] < 2.2) cells.push(i);
      edgeTries = Math.round(cells.length * cu * cu * 24 * (0.3 + 1.4 * fuzz2) * Math.sqrt(kDen) * (0.5 + 0.5 * Math.min(1, R / 512)) * (1 - 0.55 * fine));
    }
    const n1 = Math.floor(edgeTries * (part + 1) / FRINGE_PARTS);
    for (let n = Math.floor(edgeTries * part / FRINGE_PARTS); n < n1; n++) {
      const c = cells[rand() * cells.length | 0];
      const X = (c % N + rand()) * cu - PAD, Y = ((c / N | 0) + rand()) * cu - PAD;
      const d0 = sdAt(X, Y);
      if (d0 < -1.2 || d0 > 1.8) continue;
      const d = d0 + tuftAt(X, Y);
      if (d < -0.2 || d > 1.2) continue;
      const [fx, fy] = flow(X, Y);
      const [ox, oy] = outward(X, Y);
      const ang = (rand() - 0.5) * 1.1;
      const ca = Math.cos(ang), sa = Math.sin(ang);
      let ux = fx * ca - fy * sa, uy = fx * sa + fy * ca;
      const r1 = rand();
      const hang = 1 + 0.5 * gravity * Math.max(0, oy) - 0.35 * gravity * Math.max(0, -oy);
      let fl = (0.3 + 1.5 * r1 * r1 * r1) * px * kLen * (0.5 + fuzz2) * hang;
      const x0 = (X + PAD) * px, y0 = (Y + PAD) * px;
      let bend = (rand() - 0.5) * 1.17 * curl * fl;
      if (oy < -0.3 && rand() < 0.06 * (0.3 + fuzz2)) {
        ux = ox * 0.8 + (rand() - 0.5) * 0.6;
        uy = oy * 0.8;
        fl *= 1.2;
        bend = (rand() < 0.5 ? -1 : 1) * (0.25 + 0.3 * gravity) * fl;
      }
      fringe.moveTo(x0, y0);
      const qx = x0 + ux * fl * 0.5 - uy * bend, qy = y0 + uy * fl * 0.5 + ux * bend, ex = x0 + ux * fl, ey = y0 + uy * fl + gravity * fl * 0.25;
      fringe.quadraticCurveTo(qx, qy, ex, ey);
      fibreLight(X, Y, ux, uy, 0.3);
      if (fib[1] > 0.08) {
        const fb = fringeBack[glintOf(fib[1])];
        fb.moveTo(x0, y0);
        fb.quadraticCurveTo(qx, qy, ex, ey);
      }
    }
  });
  const tufts = new Path2D(), tuftTips = [new Path2D(), new Path2D(), new Path2D()];
  const tuftBack = [];
  for (let i = 0; i < GLINTS; i++) tuftBack.push(new Path2D());
  steps.push(() => {
    const band = cells.length * cu * cu, around = band / 3.8;
    const count = Math.round(around / (0.45 + 0.3 * kLen) * (0.3 + 0.8 * fuzz2) * Math.sqrt(kDen) * (1 + 0.6 * fine));
    for (let n = 0, tries = 0; n < count && tries < 6 * count; tries++) {
      const c = cells[rand() * cells.length | 0];
      const X = (c % N + rand()) * cu - PAD, Y = ((c / N | 0) + rand()) * cu - PAD;
      const r0 = rand(), r1 = rand(), r2 = rand(), r3 = rand();
      const d0 = sdAt(X, Y);
      if (d0 < -0.5 || d0 > 1.6) continue;
      const d = d0 + tuftAt(X, Y);
      if (d < 0 || d > 0.9) continue;
      if (lockAt(lock, X, Y) < -0.4 + 0.8 * r3) continue;
      n++;
      const [fx, fy] = flow(X, Y);
      const [ox, oy] = outward(X, Y);
      const a = (r0 - 0.5) * 0.8 * (0.5 + curl), ca = Math.cos(a), sa = Math.sin(a);
      let sx = 0.75 * ox + 0.25 * fx, sy = 0.75 * oy + 0.25 * fy;
      let ex = 0.55 * fx + 0.45 * ox, ey = 0.55 * fy + 0.45 * oy + 0.35 * gravity;
      const sl = Math.hypot(sx, sy) || 1, el = Math.hypot(ex, ey) || 1;
      [sx, sy] = [(sx * ca - sy * sa) / sl, (sx * sa + sy * ca) / sl];
      [ex, ey] = [(ex * ca - ey * sa) / el, (ex * sa + ey * ca) / el];
      const hang = 1 + 0.5 * gravity * Math.max(0, oy) - 0.35 * gravity * Math.max(0, -oy);
      const Lt = (0.5 + 1.1 * r1 * r1) * px * kLen * (0.5 + fuzz2) * hang;
      const wT = (0.3 + 0.35 * r2) * px * Math.min(1.5, kLen);
      const x0 = (X + PAD) * px, y0 = (Y + PAD) * px;
      const cx = x0 + sx * Lt * 0.5, cy = y0 + sy * Lt * 0.5;
      const tx = x0 + (sx * 0.4 + ex * 0.6) * Lt, ty = y0 + (sy * 0.4 + ey * 0.6) * Lt;
      const qx = -sy, qy = sx;
      fibreLight(X, Y, ex, ey, 0.4);
      const fb = fib[1] > 0.08 ? tuftBack[glintOf(fib[1])] : null;
      const tt = tuftTips[Math.min(2, fib[2] * 3 | 0)];
      const hairs = 4 + (rand() * 5 | 0);
      for (let k = 0; k < hairs; k++) {
        const o = rand() - 0.5, f = 0.7 + 0.3 * rand(), w = (rand() - 0.5) * 0.35;
        const hx0 = x0 + qx * o * wT, hy0 = y0 + qy * o * wT;
        const hcx = cx + qx * o * wT * 0.6, hcy = cy + qy * o * wT * 0.6;
        const htx = hx0 + (tx + qx * (o * 0.25 + w) * wT - hx0) * f, hty = hy0 + (ty + qy * (o * 0.25 + w) * wT - hy0) * f;
        tufts.moveTo(hx0, hy0);
        tufts.quadraticCurveTo(hcx, hcy, htx, hty);
        const mx = 0.25 * hx0 + 0.5 * hcx + 0.25 * htx, my = 0.25 * hy0 + 0.5 * hcy + 0.25 * hty;
        tt.moveTo(mx, my);
        tt.quadraticCurveTo(0.5 * (hcx + htx), 0.5 * (hcy + hty), htx, hty);
        if (fb) {
          fb.moveTo(mx, my);
          fb.quadraticCurveTo(0.5 * (hcx + htx), 0.5 * (hcy + hty), htx, hty);
        }
      }
    }
  });
  steps.push(() => {
    const tw = Math.max(0.4, 0.11 * px);
    dg.lineWidth = lg.lineWidth = bg.lineWidth = tw;
    dg.globalCompositeOperation = "destination-out";
    dg.globalAlpha = 0.6;
    dg.strokeStyle = "#000";
    lg.strokeStyle = "#fff";
    for (let i = 0; i < 3; i++) {
      dg.stroke(tuftTips[i]);
      lg.globalAlpha = 0.08 + 0.11 * i;
      lg.stroke(tuftTips[i]);
    }
    dg.globalCompositeOperation = "source-over";
    bg.strokeStyle = "#fff";
    for (let i = 0; i < GLINTS; i++) {
      bg.globalAlpha = (i + 0.5) / GLINTS;
      bg.stroke(tuftBack[i]);
    }
    dg.globalAlpha = lg.globalAlpha = bg.globalAlpha = 1;
    lg.lineWidth = dg.lineWidth = Math.max(0.35, 0.08 * px);
    lg.globalAlpha = 0.5 * ((140 - FUR_MID) / (255 - FUR_MID));
    lg.strokeStyle = "#fff";
    lg.stroke(fringe);
    lg.globalAlpha = 1;
    bg.lineWidth = lg.lineWidth;
    bg.strokeStyle = "#fff";
    for (let i = 0; i < GLINTS; i++) {
      bg.globalAlpha = (i + 0.5) / GLINTS;
      bg.stroke(fringeBack[i]);
    }
    bg.globalAlpha = 1;
    dg.globalCompositeOperation = "destination-out";
    dg.globalAlpha = 0.7;
    dg.strokeStyle = "#000";
    dg.stroke(fringe);
    dg.globalAlpha = 1;
    dg.globalCompositeOperation = "source-over";
  });
  const mi = new ImageData(R, R), zi = new ImageData(R, R), ci = new ImageData(R, R);
  const hOut = 0.5 + 1.2 * fuzz2, reachOut = hOut + tuftK;
  const haloA = HALO_A * (1 - 0.6 * fine), haloStreak = HALO_STREAK * (1 + 1.2 * fine);
  const hn = Math.max(1, Math.round(HALO_HAIR * kLen * px)), hs1 = hn + 1;
  const hsd = W_SD * Math.sqrt(1 + hn * (2 * hn + 1) / (3 * hs1)) / hs1;
  const off = Math.round(FILM_M * px);
  rows(R, 64, (y0, y1) => {
    const md = mi.data, zd = zi.data, cd = ci.data;
    for (let y = y0; y < y1; y++) {
      const Y = (y + 0.5) / px - PAD;
      for (let x = 0; x < R; x++) {
        const X = (x + 0.5) / px - PAD;
        const k = (y * R + x) * 4;
        md[k] = md[k + 1] = md[k + 2] = zd[k] = zd[k + 1] = zd[k + 2] = 255;
        const d0 = sdAt(X, Y);
        if (d0 >= tuftK + HALO_CORE) {
          md[k + 3] = cd[k + 3] = 255;
          continue;
        }
        const d = d0 <= -reachOut ? d0 : d0 + tuftAt(X, Y);
        if (d >= 0) {
          md[k + 3] = 255;
          cd[k + 3] = Math.round(255 * smooth(0.25 * HALO_CORE, HALO_CORE, d));
          continue;
        }
        const z = Math.min(1, -d / (2 * hOut));
        const nB = gridAt(nXg, X, Y) * B3[0] + gridAt(nYg, X, Y) * B3[1];
        zd[k + 3] = Math.round(255 * HALO_LIFT * z * z * (3 - 2 * z) * (0.45 + 0.55 * smooth(-0.3, 0.6, nB)));
        if (d <= -hOut) continue;
        const o = 1 + d / hOut, f = o * o * (3 - 2 * o);
        const [ux, uy] = freeDirAt(X, Y);
        let acc = 0;
        for (let t = -hn; t <= hn; t++) {
          const sx = Math.round(x + off + ux * t), sy = Math.round(y + off + uy * t);
          acc += (sx < 0 || sy < 0 || sx >= Rb || sy >= Rb ? W_MEAN : white[sy * Rb + sx]) * (1 - Math.abs(t) / hs1);
        }
        const strand = (acc / hs1 - W_MEAN) / hsd;
        md[k + 3] = Math.round(255 * Math.max(0, Math.min(1, f * (haloA + haloStreak * strand))));
      }
    }
  });
  steps.push(() => {
    const zc = makeCanvas(R), zg = zc && ctx2d(zc, false);
    if (zc && zg) {
      zg.putImageData(zi, 0, 0);
      fur.haze = zc;
    }
    mg.putImageData(mi, 0, 0);
    mg.lineCap = "round";
    mg.lineWidth = Math.max(0.35, 0.08 * px);
    mg.strokeStyle = "rgba(255,255,255,0.45)";
    mg.stroke(fringe);
    mg.lineWidth = Math.max(0.4, 0.11 * px);
    mg.strokeStyle = "rgba(255,255,255,0.8)";
    mg.stroke(tufts);
    const oc = makeCanvas(R), og = oc && ctx2d(oc, false);
    if (oc && og) {
      og.putImageData(ci, 0, 0);
      fur.core = oc;
    }
    fur.dark = dc;
    fur.light = lc;
    fur.glint = gc;
    fur.back = bc;
    fur.backSoft = sc2;
    fur.mask = mc;
    file(fur);
  });
  return { steps };
}
function furCut(fur) {
  if (fur.cut || !fur.mask) return fur.cut;
  const R = fur.mask.width, cc = makeCanvas(R), cg = cc && ctx2d(cc, false);
  if (cc && cg) {
    cg.fillRect(0, 0, R, R);
    cg.globalCompositeOperation = "destination-out";
    cg.drawImage(fur.mask, 0, 0);
    fur.cut = cc;
  }
  return fur.cut;
}
function buildFabricMatcap(out, c, f, p) {
  const { V, U, D } = f;
  const vivid = p.vivid ?? 0, px = [0, 0, 0];
  const L = fabricKey(U, V, p.front);
  const Lb = fabricBack(f.K, V);
  const amb = Math.max(0.03, 0.2 - 0.15 * p.shadow);
  const fillK = Math.max(0.04, 0.24 - 0.13 * p.shadow);
  const wrap = Math.min(0.7, 0.05 + 0.12 * p.spread);
  const kd = 1.3;
  const floorK = 0.34 * Math.min(1.6, p.shadow / 0.35);
  const Fl = norm3([-0.55 * U[0] + 0.85 * V[0] + 0.15 * D[0], -0.55 * U[1] + 0.85 * V[1] + 0.15 * D[1], -0.55 * U[2] + 0.85 * V[2] + 0.15 * D[2]]);
  const satin2 = p.sheen ?? 0;
  const backSoft = Math.min(1, Math.max(0, p.backSoft ?? BACK_SOFT));
  const sheenK = 0.16 * p.highlight * satin2, rimK = 1.15 * p.rim * satin2, sssK = 0.2 + 0.25 * vivid;
  const mx = Math.max(c[0], c[1], c[2], 0.02);
  const cb = [c[0] / mx, c[1] / mx, c[2] / mx];
  const rimIn = [cb[0], cb[1], cb[2]];
  const wR = 0.45 * (1 - vivid), wS = 0.25 * (1 - vivid);
  const rimOut = [wR + (1 - wR) * cb[0], wR + (1 - wR) * cb[1], wR + (1 - wR) * cb[2]];
  const sheenC = [wS + (1 - wS) * cb[0], wS + (1 - wS) * cb[1], wS + (1 - wS) * cb[2]];
  const yOf = (v) => 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
  const de = 1.8 + 2.2 * vivid;
  const d0 = [Math.pow(c[0], de), Math.pow(c[1], de), Math.pow(c[2], de)];
  const dk = yOf(c) / Math.max(1e-4, yOf(d0));
  const deep = [d0[0] * dk, d0[1] * dk, d0[2] * dk];
  const sc = [cb[0] * cb[0] * c[0], cb[1] * cb[1] * c[1], cb[2] * cb[2] * c[2]];
  for (let j = 0; j < M; j++) {
    for (let i = 0; i < M; i++) {
      let nx = i / (M - 1) * 2 - 1, ny = j / (M - 1) * 2 - 1;
      let r2 = nx * nx + ny * ny;
      if (r2 > 1.14) continue;
      if (r2 > 1) {
        const s = 1 / Math.sqrt(r2);
        nx *= s;
        ny *= s;
        r2 = 1;
      }
      const nz = Math.sqrt(1 - r2);
      const nl = nx * L[0] + ny * L[1] + nz * L[2];
      const nv = Math.max(0, nx * V[0] + ny * V[1] + nz * V[2]);
      const nd = nx * D[0] + ny * D[1] + nz * D[2];
      const nb = nx * Lb[0] + ny * Lb[1] + nz * Lb[2];
      const q = 1 - nv;
      const w = wrap + (0.45 - wrap) * q * q;
      let dif = Math.min(1, Math.max(0, (nl + w) / (1 + w)));
      dif = Math.pow(dif, 1.35);
      const fill = fillK * Math.max(0, nx * Fl[0] + ny * Fl[1] + nz * Fl[2]);
      const floor = 1 - floorK * Math.max(0, nd) * (0.6 + 0.4 * q);
      const light = (amb + kd * dif + fill) * floor;
      const t = Math.min(1, Math.max(0, 1 - light));
      const sb = Math.max(0, 1 - (nl + 0.08) * (nl + 0.08) / 0.18);
      const sss = sssK * sb * sb * floor;
      const lit = Math.min(1, Math.max(0, (nl + 0.2) / 0.8));
      const sheen = sheenK * q * q * lit;
      const bl = smooth(-0.3, 0.6, nb);
      const rq = smooth(0.18 - 0.12 * backSoft, 0.62 + 0.18 * backSoft, q);
      const rim = rimK * rq * Math.sqrt(rq) * (0.3 + 0.7 * bl) * (1 - 0.7 * Math.max(0, nd));
      const hot = rq * rq * Math.sqrt(rq) * (1 - 0.5 * backSoft);
      const k = (j * M + i) * 3;
      let r = 0, g = 0, b = 0;
      for (let ch = 0; ch < 3; ch++) {
        const base = c[ch] + (deep[ch] - c[ch]) * t * 0.9;
        const v = base * light + sss * sc[ch] + sheen * sheenC[ch] + rim * (rimIn[ch] + (rimOut[ch] - rimIn[ch]) * hot);
        if (ch === 0) r = v;
        else if (ch === 1) g = v;
        else b = v;
      }
      const m = Math.max(r, g, b);
      if (m > 0.8) {
        const kk = (0.8 + 0.2 * (1 - Math.exp(-(m - 0.8) / 0.2))) / m;
        r *= kk;
        g *= kk;
        b *= kk;
      }
      if (vivid > 0) {
        px[0] = r;
        px[1] = g;
        px[2] = b;
        vivify(px, vivid, 1);
        [r, g, b] = px;
      }
      out[k] = srgb(r);
      out[k + 1] = srgb(g);
      out[k + 2] = srgb(b);
    }
  }
}
const GAP_DEEP = 0.28, GAP_SCATTER = 0.42;
const gapFilms = /* @__PURE__ */ new WeakMap();
function gapFilm(fur, color, vivid = 0) {
  const src = fur.dark;
  if (!src) return null;
  let byColor = gapFilms.get(fur);
  if (!byColor) gapFilms.set(fur, byColor = /* @__PURE__ */ new Map());
  const vq = Math.round(Math.min(1, vivid) * 10) / 10;
  const key = `${color}|${vq}`;
  const hit = byColor.get(key);
  if (hit !== void 0) return hit;
  const c = makeCanvas(src.width), g = c && ctx2d(c, false);
  if (c && g) {
    const lin = linearColor(color);
    const e = [srgb(lin[0]) / 255, srgb(lin[1]) / 255, srgb(lin[2]) / 255];
    const mx = Math.max(e[0], e[1], e[2], 1e-3);
    const lu = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    const pale = Math.min(e[0], e[1], e[2]) / mx;
    const kd = GAP_DEEP + GAP_SCATTER * lu * lu * pale;
    const ch = (v) => Math.round(255 * kd * v * Math.pow(v / mx, 3.2 + 3 * vq));
    const r = ch(e[0]), gr = ch(e[1]), b = ch(e[2]);
    const deep = (0.2126 * r + 0.7152 * gr + 0.0722 * b) / 255 / Math.max(1e-3, lu);
    g.globalAlpha = Math.min(1, Math.sqrt((1 - GAP_REF) / Math.max(0.05, 1 - deep)));
    g.drawImage(src, 0, 0);
    g.globalAlpha = 1;
    g.globalCompositeOperation = "source-in";
    g.fillStyle = `rgb(${r} ${gr} ${b})`;
    g.fillRect(0, 0, src.width, src.height);
  }
  if (byColor.size >= 4) byColor.clear();
  byColor.set(key, c && g ? c : null);
  return c && g ? c : null;
}
const TIP_TINT = 0.6;
const tipFilms = /* @__PURE__ */ new WeakMap();
function tipFilm(fur, color, vivid) {
  const src = fur.light;
  if (!src) return null;
  const v = Math.round((TIP_TINT + (1 - TIP_TINT) * Math.min(1, Math.max(0, vivid))) * 10) / 10;
  const key = `${color}|${v}`;
  let byKey = tipFilms.get(fur);
  if (!byKey) tipFilms.set(fur, byKey = /* @__PURE__ */ new Map());
  const hit = byKey.get(key);
  if (hit !== void 0) return hit;
  const c = makeCanvas(src.width), g = c && ctx2d(c, false);
  if (c && g) {
    g.drawImage(src, 0, 0);
    g.globalCompositeOperation = "source-in";
    const lin = linearColor(color);
    const e = [srgb(lin[0]), srgb(lin[1]), srgb(lin[2])];
    const mx = Math.max(e[0], e[1], e[2], 1);
    const ch = (u) => Math.round(255 + (255 * u / mx - 255) * Math.min(1, v));
    g.fillStyle = `rgb(${ch(e[0])} ${ch(e[1])} ${ch(e[2])})`;
    g.fillRect(0, 0, src.width, src.height);
  }
  if (byKey.size >= 6) byKey.clear();
  byKey.set(key, c && g ? c : null);
  return c && g ? c : null;
}
const sheenFilms = /* @__PURE__ */ new WeakMap();
function sheenFilm(fur, color, p) {
  const gl = fur.glint, bk = fur.back;
  if (!gl || !bk) return null;
  const hk = Math.min(1, 0.45 * p.highlight * (p.shine ?? 0)), rk = Math.min(1, 1.1 * p.rim);
  if (hk <= 0.01 && rk <= 0.01) return null;
  const vivid = Math.min(1, p.vivid ?? 0);
  const soft2 = Math.min(1, Math.max(0, Math.round((p.backSoft ?? BACK_SOFT) * 20) / 20));
  const key = `${color}|${hk.toFixed(2)}|${rk.toFixed(2)}|${vivid.toFixed(1)}|${soft2}`;
  let byKey = sheenFilms.get(fur);
  if (!byKey) sheenFilms.set(fur, byKey = /* @__PURE__ */ new Map());
  const hit = byKey.get(key);
  if (hit !== void 0) return hit;
  const W = gl.width;
  const c = makeCanvas(W), g = c && ctx2d(c, false);
  const t = makeCanvas(W), tg = t && ctx2d(t, false);
  if (c && g && t && tg) {
    const lin = linearColor(color);
    const e = [srgb(lin[0]), srgb(lin[1]), srgb(lin[2])];
    const mx = Math.max(e[0], e[1], e[2], 1);
    const mix2 = (k) => `rgb(${[0, 1, 2].map((i) => Math.round(255 + (255 * e[i] / mx - 255) * k)).join(" ")})`;
    const layers = [
      [bk, mix2(0.65 + 0.35 * vivid), rk * (1 - 0.85 * soft2)],
      [fur.backSoft, mix2(0.7 + 0.3 * vivid), rk * soft2],
      [gl, mix2(0.45 + 0.55 * vivid), hk]
    ];
    for (const [src, fill, a] of layers) {
      if (!src) continue;
      if (a <= 0.01) continue;
      tg.globalCompositeOperation = "copy";
      tg.drawImage(src, 0, 0);
      tg.globalCompositeOperation = "source-in";
      tg.fillStyle = fill;
      tg.fillRect(0, 0, W, W);
      g.globalAlpha = a;
      g.drawImage(t, 0, 0);
    }
    g.globalAlpha = 1;
  }
  if (byKey.size >= 6) byKey.clear();
  byKey.set(key, c && g ? c : null);
  return c && g ? c : null;
}
const norm3 = (v) => {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};
function capFrame(r) {
  const cr = Math.cos(r.roll), sr = Math.sin(r.roll);
  const lx = cr * r.lx + sr * r.ly, ly = -sr * r.lx + cr * r.ly;
  const mirror = r.facing < 0 ? -1 : 1;
  const zf = mirror * Math.min(1, Math.abs(r.facing) / 0.16);
  const { cy, sy, cp, sp } = r;
  const local = (x, y, z, zk = zf) => norm3([cy * x + sy * sp * y - sy * cp * z, cp * y + sp * z, zk * (sy * x - cy * sp * y + cy * cp * z)]);
  const L = local(E_XY * lx, E_XY * ly, E_Z);
  const V = local(0, 0, 1, mirror);
  const H = norm3([L[0] + V[0], L[1] + V[1], L[2] + V[2]]);
  const U = local(lx, ly, 0, mirror);
  const cw = Math.cos(80 * Math.PI / 180), sw = Math.sin(80 * Math.PI / 180);
  const wx = cw * lx - sw * ly, wy = sw * lx + cw * ly;
  const Ws = norm3([0.55 * wx, 0.55 * wy, 0.83]);
  const As = norm3([Ws[1], -Ws[0], 0]);
  const Bs = [Ws[1] * As[2] - Ws[2] * As[1], Ws[2] * As[0] - Ws[0] * As[2], Ws[0] * As[1] - Ws[1] * As[0]];
  const W = local(Ws[0], Ws[1], Ws[2], mirror), A = local(As[0], As[1], As[2], mirror), B = local(Bs[0], Bs[1], Bs[2], mirror);
  const D = local(sr, cr, 0, mirror);
  const [kx0, ky0] = r.bx !== void 0 && r.by !== void 0 ? [r.bx, r.by] : backFromKey(r.lx, r.ly);
  const K = local(cr * kx0 + sr * ky0, -sr * kx0 + cr * ky0, 0, mirror);
  return { L, V, H, U, W, A, B, D, K };
}
const states = /* @__PURE__ */ new WeakMap();
function stateFor(ctx, outline) {
  const key = ctx.canvas ?? ctx;
  let byOutline = states.get(key);
  if (!byOutline) {
    byOutline = /* @__PURE__ */ new Map();
    states.set(key, byOutline);
  }
  let s = byOutline.get(outline);
  if (!s) {
    s = {
      N: 0,
      img: null,
      mc: new Float32Array(MM * 3),
      mcPrev: new Float32Array(MM * 3),
      mcMix: new Float32Array(MM * 3),
      mixVersion: 0,
      blendT: 1,
      blendFrames: 1,
      sinceBuild: 0,
      L: null,
      V: null,
      lx: NaN,
      ly: NaN,
      base: "",
      shadow: NaN,
      highlight: NaN,
      spread: NaN,
      rim: NaN,
      vivid: NaN,
      front: NaN,
      sheen: NaN,
      backSoft: NaN,
      K: null,
      version: 0,
      imgVersion: -1,
      imgAoK: NaN,
      imgForm: null,
      aoK: -1,
      aoMul: new Float32Array(256),
      scratch: [null, null],
      scratchIdx: 0,
      scratchN: 0,
      scratchStale: true,
      sprites: [null, null, null, null, null],
      sliceG: [],
      sliceIns: [],
      sliceSpr: /* @__PURE__ */ new Map(),
      sliceSprKey: "",
      sliceSprMade: /* @__PURE__ */ new Set(),
      sliceKey: "",
      body: [null, null],
      bodyIdx: 0,
      spriteVersion: -1,
      spritePx: 0,
      spriteFur: null,
      miss: [0, 0],
      missAt: [0, 0],
      turnAt: [0, 0],
      fade: null,
      lay: 1,
      layTmp: null,
      haloTmp: null
    };
    if (byOutline.size > 4) byOutline.clear();
    byOutline.set(outline, s);
  }
  return s;
}
const BIN = 1 / 48;
const moved = (a, b) => !b || Math.abs(a[0] - b[0]) >= BIN || Math.abs(a[1] - b[1]) >= BIN || Math.abs(a[2] - b[2]) >= BIN;
function mulAffine(A, B) {
  return [
    A[0] * B[0] + A[2] * B[1],
    A[1] * B[0] + A[3] * B[1],
    A[0] * B[2] + A[2] * B[3],
    A[1] * B[2] + A[3] * B[3],
    A[0] * B[4] + A[2] * B[5] + A[4],
    A[1] * B[4] + A[3] * B[5] + A[5]
  ];
}
function raysOf(img, form, c0) {
  const { N, sd } = form;
  const data = img.data;
  const toG = (v) => (v + PAD) / SPAN * N - 0.5;
  const dOut = [], cOut = [];
  for (let s = 0; s < CONIC_STOPS; s++) {
    const phi = s / CONIC_STOPS * Math.PI * 2, ux = Math.cos(phi), uy = Math.sin(phi);
    const ds = [], cs = [];
    for (let r = 0; r < 80; r += 0.5) {
      const x = c0[0] + ux * r, y = c0[1] + uy * r;
      const d = bilerp(sd, N, toG(x), toG(y));
      if (d < 0) break;
      const gx = Math.round(toG(x)), gy = Math.round(toG(y));
      if (gx < 0 || gy < 0 || gx >= N || gy >= N) break;
      const k = (gy * N + gx) * 4;
      ds.push(d);
      cs.push(data[k], data[k + 1], data[k + 2]);
    }
    dOut.push(Float32Array.from(ds));
    cOut.push(Float32Array.from(cs));
  }
  return { d: dOut, c: cOut };
}
function sliceGradient(ctx, rays, inset, dark, c0, lxy, linear) {
  const at = (s) => {
    const d = rays.d[s], c = rays.c[s];
    const ins = typeof inset === "number" ? inset : inset[s], k = 1 - (typeof dark === "number" ? dark : dark[s]);
    let j = 0;
    for (let i = 0; i < d.length; i++) if (d[i] >= ins) j = i;
    return c.length ? `rgb(${c[j * 3] * k | 0} ${c[j * 3 + 1] * k | 0} ${c[j * 3 + 2] * k | 0})` : "rgb(0 0 0)";
  };
  if (!linear && typeof ctx.createConicGradient === "function") {
    const g2 = ctx.createConicGradient(0, c0[0], c0[1]);
    for (let s = 0; s <= CONIC_STOPS; s++) g2.addColorStop(s / CONIC_STOPS, at(s % CONIC_STOPS));
    return g2;
  }
  const g = ctx.createLinearGradient(c0[0] + lxy[0] * 50, c0[1] + lxy[1] * 50, c0[0] - lxy[0] * 50, c0[1] - lxy[1] * 50);
  const ray = (a) => Math.round((a % (Math.PI * 2) + Math.PI * 2) % (Math.PI * 2) / (Math.PI * 2 / CONIC_STOPS)) % CONIC_STOPS;
  g.addColorStop(0, at(ray(Math.atan2(lxy[1], lxy[0]))));
  g.addColorStop(0.5, at(ray(Math.atan2(lxy[0], -lxy[1]))));
  g.addColorStop(1, at(ray(Math.atan2(-lxy[1], -lxy[0]))));
  return g;
}
function hazeColor(rays, base, rim, vivid = 0) {
  const m = /rgb\((\d+) (\d+) (\d+)\)/.exec(brightest(rays));
  const b = m ? [+m[1], +m[2], +m[3]] : [255, 255, 255];
  const lin = linearColor(base);
  const e = [srgb(lin[0]), srgb(lin[1]), srgb(lin[2])];
  const mx = Math.max(e[0], e[1], e[2], 1);
  const k = Math.min(0.85, 0.75 * rim);
  const pale = 0.45 + 0.55 * Math.min(1, vivid);
  const q = (i) => Math.round(b[i] + (255 + (255 * e[i] / mx - 255) * pale - b[i]) * k);
  return `rgb(${q(0)} ${q(1)} ${q(2)})`;
}
function brightest(rays) {
  let best = -1, r = 0, g = 0, b = 0;
  for (const c of rays.c) {
    for (let i = 0; i < c.length; i += 3) {
      const l = 0.2126 * c[i] + 0.7152 * c[i + 1] + 0.0722 * c[i + 2];
      if (l > best) best = l, r = c[i], g = c[i + 1], b = c[i + 2];
    }
  }
  const q = (v) => Math.min(255, Math.round(v / 4) * 4);
  return `rgb(${q(r)} ${q(g)} ${q(b)})`;
}
function stackReach(rel, K, ex, ey, dl1, vx, vy, h, at) {
  const v0 = vx * rel.cx[0] + vy * rel.cy[0], ve = (vx * ex + vy * ey) * dl1;
  let best = h, kb = 0, far = false;
  for (let k = 1; k < K; k++) {
    const r = vx * rel.cx[k] + vy * rel.cy[k] + rel.scale[k] * (h - v0);
    if (r + rel.z[k] * ve > best) best = r + rel.z[k] * ve, kb = k, far = false;
    if (r - rel.z[k] * ve > best) best = r - rel.z[k] * ve, kb = k, far = true;
  }
  if (at) at.k = kb, at.far = far;
  return best;
}
function haloStretch(form, rel, K, ex, ey, dl1) {
  if (dl1 <= 1e-4) return [1, 0, 0, 1, 0, 0];
  const rim = rimOf(form);
  const hp = reach(rim, ex, ey), hn = reach(rim, -ex, -ey);
  const up = stackReach(rel, K, ex, ey, dl1, ex, ey, hp), un = stackReach(rel, K, ex, ey, dl1, -ex, -ey, hn);
  const s1 = (up + un) / Math.max(1, hp + hn) - 1, t = up - (s1 + 1) * hp;
  return [1 + s1 * ex * ex, s1 * ex * ey, s1 * ex * ey, 1 + s1 * ey * ey, ex * t, ey * t];
}
function haloRays(form, rel, K, ex, ey, dl1, S, backShade) {
  const rim = rimOf(form);
  const inset = new Float32Array(CONIC_STOPS), dark = new Float32Array(CONIC_STOPS);
  const s1 = S[0] + S[3] - 2, at = { k: 0, far: false };
  for (let s = 0; s < CONIC_STOPS; s++) {
    const phi = s / CONIC_STOPS * Math.PI * 2, ux = Math.cos(phi), uy = Math.sin(phi);
    const ue = ux * ex + uy * ey, q = s1 / (1 + s1);
    let vx = ux - q * ue * ex, vy = uy - q * ue * ey;
    const vl = Math.hypot(vx, vy);
    vx /= vl;
    vy /= vl;
    stackReach(rel, K, ex, ey, dl1, vx, vy, reach(rim, vx, vy), at);
    inset[s] = Math.max(HALO_INSET, rel.inset[at.k]);
    dark[s] = at.far ? HALO_SHADE * backShade * (1 - Math.exp(-2.4 * (at.k / K))) : 0;
  }
  return { inset, dark };
}
const misses = /* @__PURE__ */ new WeakMap();
const MISS_N = 96, MISS_STEP = 0.1;
function haloMiss(form, path, rel, K, S, dxs, dys, sync) {
  const key = `${K}|${rel.q}|${Math.round(dxs / MISS_STEP)}|${Math.round(dys / MISS_STEP)}`;
  let byKey = misses.get(form);
  if (!byKey) misses.set(form, byKey = /* @__PURE__ */ new Map());
  const hit = byKey.get(key);
  if (hit || hit === null && !sync) return hit ?? null;
  if (byKey.size > 256) byKey.clear();
  if (!sync) {
    byKey.set(key, null);
    idle(() => {
      byKey.set(key, measureMiss(form, path, rel, K, S, dxs, dys));
    });
    return null;
  }
  const r = measureMiss(form, path, rel, K, S, dxs, dys);
  byKey.set(key, r);
  return r;
}
function measureMiss(form, path, rel, K, S, dxs, dys) {
  const c = makeCanvas(MISS_N), g = c && ctx2d(c, true);
  if (!g) return [0, 0];
  const u = SPAN / MISS_N;
  const det = S[0] * S[3] - S[1] * S[2];
  const ia = S[3] / det, ib = -S[1] / det, ic = -S[2] / det, id = S[0] / det;
  const ie = -(ia * S[4] + ic * S[5]), jf = -(ib * S[4] + id * S[5]);
  g.fillStyle = "#fff";
  for (let k = 0; k < K; k++) {
    for (const z of k ? [rel.z[k], -rel.z[k]] : [0]) {
      g.setTransform(1 / u, 0, 0, 1 / u, PAD / u, PAD / u);
      g.transform(ia, ib, ic, id, ie, jf);
      g.translate(z * dxs, z * dys);
      const sk = rel.scale[k];
      if (k) g.transform(sk, 0, 0, sk, rel.cx[k] - sk * rel.cx[0], rel.cy[k] - sk * rel.cy[0]);
      g.fill(path);
    }
  }
  const px = g.getImageData(0, 0, MISS_N, MISS_N).data;
  const on = new Uint8Array(MISS_N * MISS_N), dist = new Float32Array(MISS_N * MISS_N);
  for (let i = 0; i < MISS_N * MISS_N; i++) on[i] = px[i * 4 + 3] > 127 ? 1 : 0;
  edt2d(on, 1, MISS_N, dist, null);
  let cut = 0, over = 0;
  for (let y = 0, i = 0; y < MISS_N; y++) {
    for (let x = 0; x < MISS_N; x++, i++) {
      const d = bilerp(form.sd, form.N, (x + 0.5) / MISS_N * form.N - 0.5, (y + 0.5) / MISS_N * form.N - 0.5);
      if (on[i] && d < 0) cut = Math.max(cut, -d);
      if (!on[i] && d > 0) over = Math.max(over, Math.sqrt(dist[i]) * u);
    }
  }
  return [cut, over];
}
const WEBKIT = typeof navigator !== "undefined" && /AppleWebKit\//.test(navigator.userAgent) && !/Chrome\/|Chromium\/|Edg\//.test(navigator.userAgent);
const GECKO = typeof navigator !== "undefined" && /Firefox\//.test(navigator.userAgent);
function drawPlasticCap(ctx, cfg, rig, pal, union, mat) {
  const N = tierFor(rig.dev);
  const form = formFor(cfg.typeKey ?? pathId(cfg.path), cfg.path, N, rig.halfDepth, !!rig.still);
  if (!form) return false;
  const fabric = cfg.shading === "fabric";
  const capPx = Math.ceil(SPAN * rig.dev / 100);
  const lights = (() => {
    const [bx, by] = rig.bx !== void 0 && rig.by !== void 0 ? [rig.bx, rig.by] : backFromKey(rig.lx, rig.ly);
    return { bx, by, front: mat.front ?? KEY_EL };
  })();
  const fur = fabric ? furReady(form, cfg.typeKey ?? pathId(cfg.path), rig.halfDepth, capPx, !!rig.still && !cfg.furOnIdle, rig.lx, rig.ly, cfg.fur ?? FUR_STOCK, lights) : null;
  if (fabric && !fur) return false;
  const st = stateFor(ctx, (cfg.typeKey ?? pathId(cfg.path)) + (fabric ? "|fabric" : ""));
  const f = capFrame(rig);
  const lxy = (() => {
    const l = Math.hypot(f.L[0], f.L[1]);
    return l < 0.05 ? [0, -1] : [f.L[0] / l, f.L[1] / l];
  })();
  if (moved(f.L, st.L) || moved(f.V, st.V) || rig.lx !== st.lx || rig.ly !== st.ly || pal.base !== st.base || mat.shadow !== st.shadow || mat.highlight !== st.highlight || mat.spread !== st.spread || mat.rim !== st.rim || (mat.vivid ?? 0) !== st.vivid || (mat.front ?? KEY_EL) !== st.front || (mat.sheen ?? 0) !== st.sheen || (mat.backSoft ?? BACK_SOFT) !== st.backSoft || fabric && moved(f.K, st.K)) {
    if (st.version > 0) st.mcPrev.set(st.mcMix);
    if (fabric) buildFabricMatcap(st.mc, linearColor(pal.base), f, mat);
    else buildMatcap(st.mc, linearColor(pal.base), f, mat);
    if (st.version === 0 || rig.still) {
      st.mcMix.set(st.mc);
      st.blendT = 1;
    } else {
      st.blendFrames = Math.min(10, Math.max(1, st.sinceBuild));
      st.blendT = 0;
    }
    st.sinceBuild = 0;
    st.mixVersion++;
    st.L = f.L;
    st.V = f.V;
    st.lx = rig.lx;
    st.ly = rig.ly;
    st.base = pal.base;
    st.shadow = mat.shadow;
    st.highlight = mat.highlight;
    st.spread = mat.spread;
    st.rim = mat.rim;
    st.vivid = mat.vivid ?? 0;
    st.front = mat.front ?? KEY_EL;
    st.sheen = mat.sheen ?? 0;
    st.backSoft = mat.backSoft ?? BACK_SOFT;
    st.K = f.K;
    st.version++;
  }
  st.sinceBuild++;
  if (st.blendT < 1) {
    st.blendT = Math.min(1, st.blendT + 1 / st.blendFrames);
    const e = st.blendT >= 1 ? 1 : st.blendT * st.blendT * (3 - 2 * st.blendT);
    const a = st.mcPrev, b = st.mc, o = st.mcMix;
    for (let i = 0; i < MM * 3; i++) o[i] = a[i] + (b[i] - a[i]) * e;
    st.mixVersion++;
  }
  const aoK = fur ? Math.min(1.2, 0.3 + 0.8 * mat.shadow) : Math.min(1.3, 1.2 * mat.shadow);
  if (aoK !== st.aoK) {
    for (let a = 0; a < 256; a++) st.aoMul[a] = Math.max(0, 1 - aoK * (1 - a / 255));
    st.aoK = aoK;
  }
  if (!st.img || st.N !== N) {
    st.img = new ImageData(N, N);
    st.N = N;
    st.imgVersion = -1;
  }
  if (st.imgVersion !== st.mixVersion || st.imgAoK !== aoK || st.imgForm !== form) {
    shadeTexels(form, st.mcMix, st.img.data, st.aoMul, fur ? edgeLift(form) : void 0);
    st.imgVersion = st.mixVersion;
    st.imgAoK = aoK;
    st.imgForm = form;
    st.scratchStale = true;
  }
  if (st.scratchN !== N) {
    st.scratch = [null, null];
    st.scratchN = N;
    st.scratchStale = true;
  }
  if (st.scratchStale) {
    st.scratchIdx ^= 1;
    let sc2 = st.scratch[st.scratchIdx];
    if (!sc2) {
      const c = makeCanvas(N);
      const g2 = c && ctx2d(c, false);
      if (!c || !g2) return false;
      sc2 = st.scratch[st.scratchIdx] = { c, g: g2 };
    }
    sc2.g.putImageData(st.img, 0, 0);
    st.scratchStale = false;
  }
  const sc = st.scratch[st.scratchIdx];
  const capSrc = sc.c;
  const K = rig.dev <= 100 ? 14 : rig.dev <= 224 ? 20 : 26;
  const rel = reliefFor(form, K, rig.round ?? 1);
  const { cy, sy, cp, sp } = rig;
  const [ca, cb, cc, cd, ce, cf] = rig.ctm;
  const m1 = sy * sp;
  const ta = ca * cy + cc * m1, tb = cb * cy + cd * m1, tc = cc * cp, td = cd * cp;
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  const near = rig.facing >= 0 ? 1 : -1;
  const dxs = sy / cy, dys = -sp / (cy * cp);
  const dl1 = Math.hypot(dxs, dys);
  let lead = 50;
  for (let k = 1; k < K; k++) lead = Math.max(lead, 50 * rel.scale[k] + rel.z[k] * dl1);
  const ex = dl1 > 1e-6 ? near * dxs / dl1 : 1, ey = dl1 > 1e-6 ? near * dys / dl1 : 0;
  const backShade = Math.min(0.7, 0.55 * Math.min(1.6, mat.shadow / 0.35));
  const haloS = fur ? haloStretch(form, rel, K, ex, ey, dl1) : null;
  const spx = Math.ceil(SPAN * rig.dev / 100);
  if (st.spritePx !== spx) {
    st.sprites = [null, null, null, null, null];
    st.spritePx = spx;
    st.spriteVersion = -1;
  }
  if (st.spriteVersion !== st.version || st.spriteFur !== fur) {
    const k = spx / SPAN;
    const soft2 = fur ? softMask(form, fur.R) : null;
    const rays = raysOf(st.img, form, [rel.cx[0], rel.cy[0]]);
    for (let i = 0; i < 4; i++) {
      if (i === 2) continue;
      let spr = st.sprites[i];
      if (!spr) {
        const c = makeCanvas(spx);
        const g3 = c && ctx2d(c, false);
        if (!c || !g3) return false;
        spr = st.sprites[i] = { c, g: g3 };
      }
      const g2 = spr.g;
      g2.setTransform(1, 0, 0, 1, 0, 0);
      g2.globalCompositeOperation = "source-over";
      g2.clearRect(0, 0, spx, spx);
      g2.imageSmoothingEnabled = true;
      g2.imageSmoothingQuality = "high";
      const halo = i === 0 && fur && fur.mask ? fur : null;
      if (halo && halo.haze) {
        g2.imageSmoothingQuality = "low";
        g2.drawImage(halo.haze, 0, 0, spx, spx);
        g2.imageSmoothingQuality = "high";
        g2.globalCompositeOperation = "source-in";
        g2.fillStyle = hazeColor(rays, pal.base, mat.rim, mat.vivid ?? 0);
        g2.fillRect(0, 0, spx, spx);
        g2.globalCompositeOperation = "destination-over";
      }
      g2.setTransform(k, 0, 0, k, PAD * k, PAD * k);
      const edge = halo && haloS ? haloRays(form, rel, K, ex, ey, dl1, haloS, backShade) : null;
      g2.fillStyle = i === 1 ? "#000" : sliceGradient(g2, rays, edge ? edge.inset : fur ? Math.max(HALO_INSET, rel.inset[0]) : rel.inset[0], edge ? edge.dark : 0, [rel.cx[0], rel.cy[0]], lxy, WEBKIT);
      if (fur && fur.mask) {
        g2.fillRect(-PAD, -PAD, SPAN, SPAN);
        g2.setTransform(1, 0, 0, 1, 0, 0);
        g2.globalCompositeOperation = "destination-in";
        g2.drawImage((i === 0 ? fur.mask : soft2) ?? fur.mask, 0, 0, spx, spx);
        g2.globalCompositeOperation = "source-over";
      } else g2.fill(cfg.path);
    }
    if (fur && fur.core) {
      let band = st.sprites[4];
      if (!band) {
        const c = makeCanvas(spx);
        const bg = c && ctx2d(c, false);
        if (c && bg) band = st.sprites[4] = { c, g: bg };
      }
      if (band) {
        band.g.setTransform(1, 0, 0, 1, 0, 0);
        band.g.globalCompositeOperation = "copy";
        band.g.drawImage(st.sprites[0].c, 0, 0);
        band.g.globalCompositeOperation = "destination-out";
        band.g.drawImage(fur.core, 0, 0, spx, spx);
        band.g.globalCompositeOperation = "source-over";
      }
    }
    st.spriteVersion = st.version;
    st.spriteFur = fur;
  }
  const stretch = (lead + 50) / 100, shift = (lead - 50) / 2;
  const A = 1 + (stretch - 1) * ex * ex, B = (stretch - 1) * ex * ey, D = 1 + (stretch - 1) * ey * ey;
  const capM = mulAffine(mulAffine(rig.ctm, [cy, m1, 0, cp, 0, 0]), [A, B, B, D, shift * ex - 50 * A - 50 * B, shift * ey - 50 * B - 50 * D]);
  const film = fur?.dark && fur.light ? fur : null;
  let g = ctx, ox = 0, oy = 0, bw = 0, bh = 0;
  if (film) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const z of [-rel.top, rel.top]) {
      const e = z * sy - 50 * cy, fo = -z * cy * sp - 50 * m1 - 50 * cp;
      const E = ca * e + cc * fo + ce, F = cb * e + cd * fo + cf;
      for (const x of [-PAD, 100 + PAD]) for (const y of [-PAD, 100 + PAD]) {
        const X = ta * x + tc * y + E, Y = tb * x + td * y + F;
        if (X < x0) x0 = X;
        if (X > x1) x1 = X;
        if (Y < y0) y0 = Y;
        if (Y > y1) y1 = Y;
      }
    }
    ox = Math.floor(x0) - 1;
    oy = Math.floor(y0) - 1;
    bw = Math.ceil(x1) + 1 - ox;
    bh = Math.ceil(y1) + 1 - oy;
    st.bodyIdx ^= 1;
    let b = st.body[st.bodyIdx];
    if (!b || b.w < bw || b.h < bh || b.w > 2 * bw + 64 || b.h > 2 * bh + 64) {
      const w = Math.ceil(bw / 64) * 64, h = Math.ceil(bh / 64) * 64;
      let c = null;
      if (typeof document !== "undefined") {
        c = document.createElement("canvas");
        c.width = w;
        c.height = h;
      } else if (typeof OffscreenCanvas !== "undefined") c = new OffscreenCanvas(w, h);
      const cg = c && ctx2d(c, false);
      b = c && cg ? st.body[st.bodyIdx] = { c, g: cg, w: c.width, h: c.height } : null;
    }
    if (b) {
      g = b.g;
      g.setTransform(1, 0, 0, 1, 0, 0);
      g.globalCompositeOperation = "source-over";
      g.globalAlpha = 1;
      g.clearRect(0, 0, bw, bh);
      g.imageSmoothingEnabled = true;
      g.imageSmoothingQuality = "high";
    } else ox = oy = 0;
  }
  const at = (z) => {
    const e = z * sy - 50 * cy, fo = -z * cy * sp - 50 * m1 - 50 * cp;
    g.setTransform(ta, tb, tc, td, ca * e + cc * fo + ce - ox, cb * e + cd * fo + cf - oy);
  };
  const [fringed, black, , side] = st.sprites;
  const c0 = [rel.cx[0], rel.cy[0]];
  const sliceKey = `${st.version}|${K}|${rel.q}`;
  if (st.sliceKey !== sliceKey) {
    const rays = raysOf(st.img, form, c0);
    st.sliceG = [];
    st.sliceIns = [];
    for (let k = 0; k < K; k++) {
      const inset = fur ? Math.max(HALO_INSET, rel.inset[k]) : rel.inset[k];
      st.sliceIns.push(inset);
      st.sliceG.push(sliceGradient(ctx, rays, inset, 0, c0, lxy, WEBKIT));
    }
    st.sliceKey = sliceKey;
  }
  const useSpr = GECKO && typeof document !== "undefined";
  const sprKey = `${sliceKey}|${spx}`;
  if (useSpr && st.sliceSprKey !== sprKey) {
    st.sliceSprKey = sprKey;
    st.sliceSprMade = /* @__PURE__ */ new Set();
    if (st.sliceSpr.size > 32) st.sliceSpr.clear();
  }
  const sprite = (k) => {
    const ins = st.sliceIns[k];
    let sp2 = st.sliceSpr.get(ins);
    if (sp2 && st.sliceSprMade.has(ins)) return sp2;
    if (!sp2 || sp2.c.width !== spx) {
      const c = document.createElement("canvas");
      c.width = c.height = spx;
      const cg = c.getContext("2d");
      if (!cg) return void 0;
      sp2 = { c, g: cg };
      st.sliceSpr.set(ins, sp2);
    }
    const kk = spx / SPAN;
    sp2.g.setTransform(1, 0, 0, 1, 0, 0);
    sp2.g.clearRect(0, 0, spx, spx);
    sp2.g.setTransform(kk, 0, 0, kk, PAD * kk, PAD * kk);
    sp2.g.fillStyle = st.sliceG[k];
    sp2.g.fill(cfg.path);
    st.sliceSprMade.add(ins);
    return sp2;
  };
  const slice = (k, z, dark) => {
    at(z);
    const sk = rel.scale[k];
    if (k > 0) {
      g.transform(sk, 0, 0, sk, rel.cx[k] - sk * rel.cx[0], rel.cy[k] - sk * rel.cy[0]);
      const spr = useSpr ? sprite(k) : void 0;
      if (spr) g.drawImage(spr.c, -PAD, -PAD, SPAN, SPAN);
      else {
        g.fillStyle = st.sliceG[k];
        g.fill(cfg.path);
      }
      if (dark > 0.01) {
        g.globalAlpha = dark;
        g.fillStyle = "#000";
        g.fill(cfg.path);
        g.globalAlpha = 1;
      }
      return;
    }
    if (fur) g.drawImage(side.c, -PAD, -PAD, SPAN, SPAN);
    else {
      g.fillStyle = st.sliceG[0];
      g.fill(cfg.path);
    }
    if (fur && g === ctx && fringeA > 0.01) {
      g.globalAlpha = fringeA;
      g.drawImage(fringed.c, -PAD, -PAD, SPAN, SPAN);
      g.globalAlpha = 1;
    }
    if (dark > 0.01) {
      g.globalAlpha = dark;
      g.drawImage(black.c, -PAD, -PAD, SPAN, SPAN);
      g.globalAlpha = 1;
    }
  };
  const faceOn = Math.abs(rig.facing);
  const fringeA = faceOn >= 0.8 ? 1 : faceOn <= 0.45 ? 0 : (faceOn - 0.45) / 0.35;
  const ring = ringOf(form), rn = ring.x.length, sd = form.sd, NF = form.N;
  const hideBy = 1.5 / (Math.sqrt(Math.abs(ta * td - tb * tc)) || 1) + 0.4;
  const texel = SPAN / NF;
  const toG = (v) => (v + PAD) / SPAN * NF - 0.5;
  const leanFar = Math.atan2(-near * dys, -near * dxs), leanNear = Math.atan2(near * dys, near * dxs);
  for (let k = K - 1; k >= 1; k--) {
    const z = -near * rel.z[k], sk = rel.scale[k];
    if (fur) {
      const bx = rel.cx[k] - sk * rel.cx[0] + z * dxs, by = rel.cy[k] - sk * rel.cy[0] + z * dys;
      let hidden = Number.isFinite(bx) && Number.isFinite(by);
      for (let j = 0; hidden && j < rn; j++) {
        const i = ringOrder(rn, leanFar, j);
        if (bilerp(sd, NF, toG(sk * ring.x[i] + bx), toG(sk * ring.y[i] + by)) < hideBy + sk * (ring.d[i] + texel)) hidden = false;
      }
      if (hidden) continue;
    }
    slice(k, z, fur ? 0 : backShade * (1 - Math.exp(-2.4 * (k / K))));
  }
  if (fur && dl1 > 1e-3) {
    const hn = reach(rimOf(form), -ex, -ey), span = 1.3 * rel.top * dl1;
    at(0);
    const ramp = g.createLinearGradient(-ex * hn, -ey * hn, -ex * (hn + span), -ey * (hn + span));
    for (let s = 0; s <= 8; s++) {
      const t = s / 8;
      ramp.addColorStop(t, `rgba(0,0,0,${(backShade * (1 - Math.exp(-2.4 * t)) * t * t * (3 - 2 * t)).toFixed(3)})`);
    }
    g.globalCompositeOperation = "source-atop";
    g.fillStyle = ramp;
    g.fillRect(-PAD - 60, -PAD - 60, SPAN + 120, SPAN + 120);
    g.globalCompositeOperation = "source-over";
  }
  slice(0, 0, 0);
  const face = Math.abs(rig.facing);
  const frontA = face >= 0.72 ? 1 : face <= 0.34 ? 0 : ((face - 0.34) / 0.38) ** 1.5;
  const cdet = capM[0] * capM[3] - capM[1] * capM[2];
  const coverBy = FRONT_FADE + 1.5 / (Math.sqrt(Math.abs(cdet)) || 1);
  for (let k = 1; k < K; k++) {
    const z = near * rel.z[k], sk = rel.scale[k];
    if (frontA >= 1 && Math.abs(cdet) > 1e-9) {
      const e = z * sy - 50 * cy, fo = -z * cy * sp - 50 * m1 - 50 * cp;
      const E = ca * e + cc * fo + ce - capM[4], F = cb * e + cd * fo + cf - capM[5];
      const ux = rel.cx[k] - sk * rel.cx[0], uy = rel.cy[k] - sk * rel.cy[0];
      let covered = true;
      for (let j = 0; covered && j < rn; j++) {
        const i = ringOrder(rn, leanNear, j);
        const u = sk * ring.x[i] + ux, v = sk * ring.y[i] + uy;
        const X = ta * u + tc * v + E, Y = tb * u + td * v + F;
        const qx = (capM[3] * X - capM[2] * Y) / cdet, qy = (capM[0] * Y - capM[1] * X) / cdet;
        if (bilerp(sd, NF, toG(qx), toG(qy)) < coverBy + sk * (ring.d[i] + texel)) covered = false;
      }
      if (covered) continue;
    }
    slice(k, z, 0);
  }
  if (frontA > 0.01) {
    g.save();
    g.setTransform(capM[0], capM[1], capM[2], capM[3], capM[4] - ox, capM[5] - oy);
    g.globalAlpha = frontA;
    g.drawImage(capSrc, -PAD, -PAD, SPAN, SPAN);
    g.restore();
  }
  if (fur && fur.mask && haloS && g !== ctx) {
    const mx = near * dxs, my = near * dys;
    const stale = Math.abs(mx - st.missAt[0]) > 1.5 * MISS_STEP || Math.abs(my - st.missAt[1]) > 1.5 * MISS_STEP;
    const measured = dl1 > 1e-4 ? haloMiss(form, cfg.path, rel, K, haloS, mx, my, !!rig.still || stale) : [0, 0];
    if (measured) {
      st.miss = measured;
      st.missAt = [mx, my];
    }
    const [cut, over] = st.miss;
    const miss = Math.max(cut, over);
    let back = smooth(HALO_FIT / 2, 2 * HALO_FIT, miss), gone = smooth(HALO_FIT / 2, HALO_FIT, over);
    const quick = Math.abs(mx - st.turnAt[0]) > MISS_STEP || Math.abs(my - st.turnAt[1]) > MISS_STEP;
    st.turnAt = [mx, my];
    const settle = !rig.still && !quick;
    if (settle && st.fade) {
      const ease = (a, b) => Math.abs(b - a) < 0.02 ? b : a + (b - a) * 0.2;
      back = ease(st.fade[0], back);
      gone = ease(st.fade[1], gone);
    }
    st.fade = [back, gone];
    const band = st.sprites[4];
    g.imageSmoothingQuality = "low";
    at(0);
    g.transform(haloS[0], haloS[1], haloS[2], haloS[3], haloS[4], haloS[5]);
    const layTo = (1 - smooth(0, HALO_BLEND, back)) * (1 - gone);
    const lay = st.lay = !settle ? layTo : st.lay + Math.max(-HALO_EASE, Math.min(HALO_EASE, layTo - st.lay));
    let h = null;
    if (lay > 0 && lay < 1) {
      let t = st.layTmp;
      if (!t || t.c.width < bw || t.c.height < bh) {
        const n = Math.ceil(Math.max(bw, bh) / 64) * 64;
        let c = null;
        if (typeof document !== "undefined") {
          c = document.createElement("canvas");
          c.width = c.height = n;
        } else c = makeCanvas(n);
        const cg = c && ctx2d(c, false);
        t = st.layTmp = c && cg ? { c, g: cg } : null;
      }
      if (t) {
        h = t.g;
        h.setTransform(1, 0, 0, 1, 0, 0);
        h.globalAlpha = 1;
        h.globalCompositeOperation = "copy";
        h.drawImage(g.canvas, 0, 0, bw, bh, 0, 0, bw, bh);
        h.imageSmoothingQuality = "low";
        h.setTransform(g.getTransform());
      }
    }
    const veil = HALO_VEIL * Math.min(1, dl1 / 0.2);
    const laid = (d) => {
      if (WEBKIT) {
        let t = st.haloTmp;
        if (!t || t.c.width < bw || t.c.height < bh) {
          const n = Math.ceil(Math.max(bw, bh) / 64) * 64;
          let c = null;
          if (typeof document !== "undefined") {
            c = document.createElement("canvas");
            c.width = c.height = n;
          } else c = makeCanvas(n);
          const cg = c && ctx2d(c, false);
          t = st.haloTmp = c && cg ? { c, g: cg } : null;
        }
        if (t) {
          const tg = t.g;
          tg.setTransform(1, 0, 0, 1, 0, 0);
          tg.globalCompositeOperation = "source-over";
          tg.globalAlpha = 1;
          tg.clearRect(0, 0, bw, bh);
          tg.imageSmoothingEnabled = true;
          tg.imageSmoothingQuality = "low";
          tg.setTransform(d.getTransform());
          tg.drawImage(fringed.c, -PAD, -PAD, SPAN, SPAN);
          d.save();
          d.setTransform(1, 0, 0, 1, 0, 0);
          d.globalCompositeOperation = "destination-atop";
          d.drawImage(t.c, 0, 0, bw, bh, 0, 0, bw, bh);
          d.restore();
        }
      } else {
        d.globalCompositeOperation = "destination-atop";
        d.drawImage(fringed.c, -PAD, -PAD, SPAN, SPAN);
      }
      if (band && veil > 0.01) {
        d.globalCompositeOperation = "source-atop";
        d.globalAlpha = veil;
        d.drawImage(band.c, -PAD, -PAD, SPAN, SPAN);
        d.globalAlpha = 1;
      }
      d.globalCompositeOperation = "source-over";
    };
    if (h) laid(h);
    if (lay >= 1 || !h && lay >= 0.5) laid(g);
    else {
      const cutAway = back < 1 ? furCut(fur) : null;
      if (cutAway) {
        const f2 = 1 + back * (miss + HALO_FIT) / 25, cx = rel.cx[0], cy2 = rel.cy[0];
        g.save();
        g.transform(f2, 0, 0, f2, cx * (1 - f2), cy2 * (1 - f2));
        g.globalCompositeOperation = "destination-out";
        g.drawImage(cutAway, -PAD, -PAD, SPAN, SPAN);
        g.restore();
      }
      if (gone < 1) {
        g.globalAlpha = 1 - gone;
        g.globalCompositeOperation = "destination-over";
        g.drawImage(fringed.c, -PAD, -PAD, SPAN, SPAN);
        g.globalAlpha = 1;
      }
      if (h) {
        g.setTransform(1, 0, 0, 1, 0, 0);
        g.globalCompositeOperation = "destination-in";
        g.globalAlpha = 1 - lay;
        g.fillStyle = "#000";
        g.fillRect(0, 0, bw, bh);
        g.globalCompositeOperation = "lighter";
        g.globalAlpha = lay;
        g.drawImage(h.canvas, 0, 0, bw, bh, 0, 0, bw, bh);
        g.globalAlpha = 1;
      }
    }
    g.globalCompositeOperation = "source-over";
  }
  if (g !== ctx && film) {
    g.globalCompositeOperation = "source-atop";
    g.imageSmoothingQuality = "low";
    const lin = linearColor(pal.base);
    const lum = Math.pow(0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2], 1 / 2.2);
    const dark = gapFilm(film, pal.base, mat.vivid ?? 0) ?? film.dark, tips = tipFilm(film, pal.base, mat.vivid ?? 0) ?? film.light;
    const sheenA = rig.dev < 160 ? 0 : face >= 0.88 ? 1 : face <= 0.55 ? 0 : (face - 0.55) / 0.33;
    const sheen = sheenA > 0.01 ? sheenFilm(film, pal.base, mat) : null;
    const lift = near * PILE_H * (cfg.fur ?? FUR_STOCK).length;
    const capT = mulAffine(mulAffine(mulAffine(rig.ctm, [1, 0, 0, 1, lift * sy, -lift * cy * sp]), [cy, m1, 0, cp, 0, 0]), [A, B, B, D, shift * ex - 50 * A - 50 * B, shift * ey - 50 * B - 50 * D]);
    const lay = (M2, a, withSheen, T = M2) => {
      g.setTransform(M2[0], M2[1], M2[2], M2[3], M2[4] - ox, M2[5] - oy);
      g.globalAlpha = a;
      g.drawImage(dark, -PAD - FILM_M, -PAD - FILM_M, FILM_SPAN, FILM_SPAN);
      if (T !== M2) g.setTransform(T[0], T[1], T[2], T[3], T[4] - ox, T[5] - oy);
      g.globalAlpha = a * (0.25 + 0.75 * lum);
      g.drawImage(tips, -PAD - FILM_M, -PAD - FILM_M, FILM_SPAN, FILM_SPAN);
      if (withSheen && sheen) {
        g.globalAlpha = a * sheenA;
        g.drawImage(sheen, -PAD - FILM_M, -PAD - FILM_M, FILM_SPAN, FILM_SPAN);
      }
      g.globalAlpha = 1;
    };
    const sideU = face >= 0.62 ? 0 : face <= 0.36 ? 1 : (0.62 - face) / 0.26;
    if (sideU < 1) lay(capM, 1 - sideU, true, capT);
    if (sideU > 0) {
      const k = rig.dev / 100, cxs = ox + bw / 2, cys = oy + bh / 2;
      lay([k, 0, 0, k, cxs - 50 * k, cys - 50 * k], sideU, false);
    }
    g.globalCompositeOperation = "source-over";
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(g.canvas, 0, 0, bw, bh, ox, oy, bw, bh);
    ctx.restore();
  }
  ctx.setTransform(ca, cb, cc, cd, ce, cf);
  const hairA = face >= 0.95 ? 1 : face <= 0.8 ? 0 : (face - 0.8) / 0.15;
  if (!fur && rig.dev >= 256 && mat.rim > 0 && mat.highlight > 0 && hairA > 0.01) {
    ctx.save();
    ctx.globalCompositeOperation = "source-atop";
    ctx.setTransform(capM[0], capM[1], capM[2], capM[3], capM[4], capM[5]);
    const al = hairA * Math.min(0.5, 0.3 * mat.rim * Math.min(1.4, mat.highlight));
    const g2 = ctx.createLinearGradient(50 + lxy[0] * 50, 50 + lxy[1] * 50, 50 - lxy[0] * 50, 50 - lxy[1] * 50);
    g2.addColorStop(0, `rgba(235,244,255,${al.toFixed(3)})`);
    g2.addColorStop(0.45, `rgba(235,244,255,${(0.35 * al).toFixed(3)})`);
    g2.addColorStop(0.75, "rgba(235,244,255,0)");
    ctx.strokeStyle = g2;
    ctx.lineJoin = "round";
    ctx.lineWidth = 1.3;
    ctx.stroke(cfg.path);
    ctx.restore();
  }
  return true;
}
function makeView(m, o, L, H, px) {
  return {
    m,
    o,
    L,
    H,
    px,
    toScreen: (p) => {
      const x = p[0] - o[0], y = p[1] - o[1], z = p[2] - o[2];
      return [m[0] * x + m[1] * y + m[2] * z, m[3] * x + m[4] * y + m[5] * z];
    },
    depth: (p) => m[6] * (p[0] - o[0]) + m[7] * (p[1] - o[1]) + m[8] * (p[2] - o[2]),
    dir: (v) => norm([m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]])
  };
}
const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const norm = (v) => {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};
const TAU$2 = Math.PI * 2;
function segs(view, len, lo, hi, quad = 11) {
  return Math.max(lo, Math.min(hi, Math.round(len * view.px / quad)));
}
function makeMesh(surf, nu, nv, inside, smooth2 = true) {
  const W = nu + 1, N = W * (nv + 1);
  const pos = new Float64Array(N * 3);
  let ext = 0;
  for (let j = 0, k = 0; j <= nv; j++) {
    for (let i = 0; i <= nu; i++, k++) {
      const p = surf(i / nu, j / nv);
      pos[k * 3] = p[0];
      pos[k * 3 + 1] = p[1];
      pos[k * 3 + 2] = p[2];
      ext = Math.max(ext, Math.abs(p[0] - pos[0]) + Math.abs(p[1] - pos[1]) + Math.abs(p[2] - pos[2]));
    }
  }
  const P = (k) => [pos[k * 3], pos[k * 3 + 1], pos[k * 3 + 2]];
  const fn = new Float64Array(nu * nv * 3);
  for (let j = 0; j < nv; j++) {
    for (let i = 0; i < nu; i++) {
      const a = j * W + i;
      const A = P(a), B = P(a + 1), C = P(a + W + 1), D = P(a + W);
      let n = cross(sub(C, A), sub(D, B));
      const mid = [(A[0] + B[0] + C[0] + D[0]) / 4, (A[1] + B[1] + C[1] + D[1]) / 4, (A[2] + B[2] + C[2] + D[2]) / 4];
      const ins = typeof inside === "function" ? inside((i + 0.5) / nu, (j + 0.5) / nv) : inside;
      if (dot(n, sub(mid, ins)) < 0) n = [-n[0], -n[1], -n[2]];
      n = norm(n);
      const f = (j * nu + i) * 3;
      fn[f] = n[0];
      fn[f + 1] = n[1];
      fn[f + 2] = n[2];
    }
  }
  const eps = 1e-6 + ext * 1e-5;
  const same = (p, q) => Math.abs(pos[p * 3] - pos[q * 3]) + Math.abs(pos[p * 3 + 1] - pos[q * 3 + 1]) + Math.abs(pos[p * 3 + 2] - pos[q * 3 + 2]) < eps;
  let closed = true;
  for (let j = 0; j <= nv && closed; j++) closed = same(j * W, j * W + nu);
  let vn = null;
  if (smooth2) {
    const acc = new Float64Array(N * 3);
    for (let j = 0; j < nv; j++) {
      for (let i = 0; i < nu; i++) {
        const f = (j * nu + i) * 3;
        const a = j * W + i;
        for (const k of [a, a + 1, a + W + 1, a + W]) {
          acc[k * 3] += fn[f];
          acc[k * 3 + 1] += fn[f + 1];
          acc[k * 3 + 2] += fn[f + 2];
        }
      }
    }
    const join = (ks) => {
      let x = 0, y = 0, z = 0;
      for (const k of ks) {
        x += acc[k * 3];
        y += acc[k * 3 + 1];
        z += acc[k * 3 + 2];
      }
      for (const k of ks) {
        acc[k * 3] = x;
        acc[k * 3 + 1] = y;
        acc[k * 3 + 2] = z;
      }
    };
    if (closed) for (let j = 0; j <= nv; j++) join([j * W, j * W + nu]);
    for (const j of [0, nv]) {
      let pole = true;
      for (let i = 1; i <= nu && pole; i++) pole = same(j * W, j * W + i);
      if (pole) join(Array.from({ length: W }, (_, i) => j * W + i));
    }
    for (let k = 0; k < N; k++) {
      const x = acc[k * 3], y = acc[k * 3 + 1], z = acc[k * 3 + 2];
      const l = Math.hypot(x, y, z) || 1;
      acc[k * 3] = x / l;
      acc[k * 3 + 1] = y / l;
      acc[k * 3 + 2] = z / l;
    }
    vn = acc;
  }
  return { nu, nv, closed, pos, fn, vn };
}
function makeTube(path, up, prof, nu, nv, closed = false) {
  const e = 1e-3;
  const at = (u) => path(closed ? (u % 1 + 1) % 1 : Math.max(0, Math.min(1, u)));
  return makeMesh(
    (u, v) => {
      const c = path(u);
      const T = norm(sub(at(u + e), at(u - e)));
      const k = dot(up, T);
      const Nn = norm([up[0] - T[0] * k, up[1] - T[1] * k, up[2] - T[2] * k]);
      const Bn = cross(T, Nn);
      const [a, b] = prof(u, v * TAU$2);
      return [c[0] + Nn[0] * a + Bn[0] * b, c[1] + Nn[1] * a + Bn[1] * b, c[2] + Nn[2] * a + Bn[2] * b];
    },
    nu,
    nv,
    (u) => path(u)
  );
}
const section = (a, b, p = 1) => (_u, ph) => {
  const c = Math.cos(ph), s = Math.sin(ph);
  return [a * Math.sign(c) * Math.pow(Math.abs(c), p), b * Math.sign(s) * Math.pow(Math.abs(s), p)];
};
function projectMesh(view, mesh) {
  const { nu, nv, pos, fn, vn } = mesh;
  const W = nu + 1, N = W * (nv + 1), C = nu * nv;
  const [m0, m1, m2, m3, m4, m5, m6, m7, m8] = view.m;
  const [ox, oy, oz] = view.o;
  const x = new Float64Array(N), y = new Float64Array(N), d = new Float64Array(N);
  for (let k = 0; k < N; k++) {
    const X = pos[k * 3] - ox, Y = pos[k * 3 + 1] - oy, Z = pos[k * 3 + 2] - oz;
    x[k] = m0 * X + m1 * Y + m2 * Z;
    y[k] = m3 * X + m4 * Y + m5 * Z;
    d[k] = m6 * X + m7 * Y + m8 * Z;
  }
  const turn = (src, dst, count) => {
    for (let k = 0; k < count; k++) {
      const a = src[k * 3], b = src[k * 3 + 1], c = src[k * 3 + 2];
      const X = m0 * a + m1 * b + m2 * c, Y = m3 * a + m4 * b + m5 * c, Z = m6 * a + m7 * b + m8 * c;
      const l = Math.hypot(X, Y, Z) || 1;
      dst[k * 3] = X / l;
      dst[k * 3 + 1] = Y / l;
      dst[k * 3 + 2] = Z / l;
    }
  };
  let n = null;
  if (vn) turn(vn, n = new Float64Array(N * 3), N);
  const f = new Float64Array(C * 3), cd = new Float64Array(C);
  turn(fn, f, C);
  for (let j = 0, c = 0; j < nv; j++) {
    for (let i = 0; i < nu; i++, c++) {
      const a = j * W + i;
      cd[c] = (d[a] + d[a + 1] + d[a + W] + d[a + W + 1]) / 4;
    }
  }
  return { nu, nv, closed: mesh.closed, x, y, d, n, f, cd };
}
function boundsOf(ps) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const p of ps) {
    for (let k = 0; k < p.x.length; k++) {
      if (p.x[k] < x0) x0 = p.x[k];
      if (p.x[k] > x1) x1 = p.x[k];
      if (p.y[k] < y0) y0 = p.y[k];
      if (p.y[k] > y1) y1 = p.y[k];
    }
  }
  return { x0, y0, x1, y1, cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, w: x1 - x0, h: y1 - y0 };
}
function monotone(mx, my, s, e) {
  const ax = mx[e] - mx[s], ay = my[e] - my[s];
  let prev = -Infinity;
  for (let k = s; k <= e; k++) {
    const t = (mx[k] - mx[s]) * ax + (my[k] - my[s]) * ay;
    if (t < prev - 1e-9) return false;
    prev = t;
  }
  return true;
}
const clampByte = (v) => v < 0 ? 0 : v > 255 ? 255 : Math.round(v);
const css = (c) => `rgb(${clampByte(c[0])},${clampByte(c[1])},${clampByte(c[2])})`;
function paintLayers(ctx, layers, px) {
  const pieces = [];
  const corners = [];
  for (const l of layers) {
    const bias = l.bias ?? 0, keep2 = l.keep, cull = l.cull !== false;
    for (const P of l.parts) {
      const { nu, nv, f, cd, x, y } = P, W = nu + 1;
      const vis = (c) => (!cull || f[c * 3 + 2] > -0.04) && (!keep2 || keep2(cd[c]));
      for (let j = 0; j < nv; j++) {
        if (l.flat) {
          for (let i = 0; i < nu; i++) {
            const c = j * nu + i;
            if (!vis(c)) continue;
            pieces.push({ d: cd[c] + bias, l, p: P, j, cols: [i, i + 1], cells: [c] });
            corners.push([x[j * W + i], y[j * W + i]], [x[(j + 1) * W + i + 1], y[(j + 1) * W + i + 1]]);
          }
          continue;
        }
        const runs = [];
        let cur = null;
        for (let i = 0; i < nu; i++) {
          if (vis(j * nu + i)) {
            if (!cur) runs.push(cur = []);
            cur.push(i);
          } else cur = null;
        }
        if (P.closed && runs.length > 1 && runs[0][0] === 0 && runs[runs.length - 1][runs[runs.length - 1].length - 1] === nu - 1) {
          runs[0] = runs.pop().concat(runs[0]);
        }
        for (const run of runs) {
          const cols = run.concat(run[run.length - 1] + 1);
          const mx = [], my = [];
          for (let k = 0; k < cols.length; k++) {
            const t = j * W + cols[k], b = t + W;
            mx.push((x[t] + x[b]) / 2);
            my.push((y[t] + y[b]) / 2);
            if (k === 0 || k === cols.length - 1) corners.push([x[t], y[t]], [x[b], y[b]]);
            else if (j === 0) corners.push([x[t], y[t]]);
            else if (j === nv - 1) corners.push([x[b], y[b]]);
          }
          let s = 0;
          while (s < cols.length - 1) {
            let e = s + 1;
            while (e + 1 < cols.length && monotone(mx, my, s, e + 1)) e++;
            let dd = 0;
            for (let k = s; k < e; k++) dd += cd[j * nu + run[k]];
            pieces.push({ d: dd / (e - s) + bias, l, p: P, j, cols: cols.slice(s, e + 1), cells: run.slice(s, e) });
            s = e;
          }
        }
      }
    }
  }
  pieces.sort((a, b) => a.d - b.d);
  const grow = 0.5 / px;
  for (const pc of pieces) {
    const { p: P, l, j, cols, cells } = pc;
    const { x, y, n, f } = P, W = P.nu + 1;
    const nc = cols.length;
    const tx = [], ty = [], bx = [], by = [];
    const t0 = j * W + cols[0], tn = j * W + cols[nc - 1];
    let ax = (x[tn] + x[tn + W]) / 2 - (x[t0] + x[t0 + W]) / 2, ay = (y[tn] + y[tn + W]) / 2 - (y[t0] + y[t0 + W]) / 2;
    const al = Math.hypot(ax, ay);
    const ux = al > 1e-9 ? ax / al : 0, uy = al > 1e-9 ? ay / al : 0;
    for (let k = 0; k < nc; k++) {
      const t = j * W + cols[k], b = t + W;
      let dx = x[t] - x[b], dy = y[t] - y[b];
      const dl = Math.hypot(dx, dy);
      if (dl > 1e-9) dx /= dl, dy /= dl;
      else dx = dy = 0;
      const e = k === 0 ? -grow : k === nc - 1 ? grow : 0;
      tx.push(x[t] + dx * grow + ux * e);
      ty.push(y[t] + dy * grow + uy * e);
      bx.push(x[b] - dx * grow + ux * e);
      by.push(y[b] - dy * grow + uy * e);
    }
    const colour = (k) => {
      if (n && !l.flat) {
        const t = (j * W + cols[k]) * 3, b = t + W * 3;
        return l.shade(norm([n[t] + n[b], n[t + 1] + n[b + 1], n[t + 2] + n[b + 2]]));
      }
      const c0 = cells[Math.max(0, Math.min(cells.length - 1, k - 1))] * 3, c1 = cells[Math.min(cells.length - 1, k)] * 3;
      return l.shade(l.flat ? [f[c1], f[c1 + 1], f[c1 + 2]] : norm([f[c0] + f[c1], f[c0 + 1] + f[c1 + 1], f[c0 + 2] + f[c1 + 2]]));
    };
    let style;
    if (l.flat || al < 1e-6) style = css(colour(0));
    else {
      const g = ctx.createLinearGradient((x[t0] + x[t0 + W]) / 2, (y[t0] + y[t0 + W]) / 2, (x[tn] + x[tn + W]) / 2, (y[tn] + y[tn + W]) / 2);
      let last2 = null;
      const m0x = (x[t0] + x[t0 + W]) / 2, m0y = (y[t0] + y[t0 + W]) / 2;
      for (let k = 0; k < nc; k++) {
        const c = colour(k);
        if (last2 && k < nc - 1 && Math.abs(c[0] - last2[0]) + Math.abs(c[1] - last2[1]) + Math.abs(c[2] - last2[2]) < 5) continue;
        const t = j * W + cols[k];
        const off = (((x[t] + x[t + W]) / 2 - m0x) * ax + ((y[t] + y[t + W]) / 2 - m0y) * ay) / (al * al);
        g.addColorStop(off < 0 ? 0 : off > 1 ? 1 : off, css(c));
        last2 = c;
      }
      style = g;
    }
    ctx.fillStyle = style;
    ctx.beginPath();
    ctx.moveTo(tx[0], ty[0]);
    for (let k = 1; k < nc; k++) ctx.lineTo(tx[k], ty[k]);
    for (let k = nc - 1; k >= 0; k--) ctx.lineTo(bx[k], by[k]);
    ctx.closePath();
    ctx.fill();
  }
  return corners;
}
function hull(pts) {
  const p = pts.slice().sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  if (p.length < 3) return p;
  const cr = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
  const lo = [], up = [];
  for (const q of p) {
    while (lo.length >= 2 && cr(lo[lo.length - 2], lo[lo.length - 1], q) <= 0) lo.pop();
    lo.push(q);
  }
  for (let i = p.length - 1; i >= 0; i--) {
    const q = p[i];
    while (up.length >= 2 && cr(up[up.length - 2], up[up.length - 1], q) <= 0) up.pop();
    up.push(q);
  }
  lo.pop();
  up.pop();
  return lo.concat(up);
}
const clamp01 = (x) => x < 0 ? 0 : x > 1 ? 1 : x;
const mix = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
const scale = (a, k) => [a[0] * k, a[1] * k, a[2] * k];
const WHITE = [255, 255, 255];
function felt(view, base, k = 1) {
  const L = view.L;
  return (n) => {
    const nl = dot(n, L);
    const dif = Math.pow(clamp01((nl + 0.4) / 1.4), 1.35);
    const graze = Math.pow(1 - clamp01(n[2]), 2);
    const lit = (0.22 + 0.95 * dif + 0.22 * graze * clamp01(nl + 0.6)) * k;
    const floor = 26 * dif * k;
    return [base[0] * lit + floor, base[1] * lit + floor, base[2] * lit + floor * 1.05];
  };
}
function velvet(view, base) {
  const L = view.L, sheen = mix(base, WHITE, 0.3);
  return (n) => {
    const nl = dot(n, L);
    const dif = clamp01((nl + 0.3) / 1.3);
    const graze = Math.pow(1 - clamp01(n[2]), 1.6);
    return mix(scale(base, 0.2 + 0.7 * dif), sheen, 0.6 * graze * (0.4 + 0.6 * dif));
  };
}
function paper(view, base) {
  const L = view.L, H = view.H;
  return (n) => {
    const dif = clamp01((dot(n, L) + 0.2) / 1.2);
    const spec = Math.pow(clamp01(dot(n, H)), 22) * 0.3;
    return mix(scale(base, 0.36 + 0.7 * dif), WHITE, spec);
  };
}
function plastic(view, base, gloss = 1) {
  const L = view.L, H = view.H;
  return (n) => {
    const dif = clamp01((dot(n, L) + 0.25) / 1.25);
    const nh = clamp01(dot(n, H));
    const spec = (Math.pow(nh, 16) * 0.2 + Math.pow(nh, 80) * 0.6) * gloss;
    const rim = Math.pow(1 - clamp01(n[2]), 3) * 0.14 * gloss;
    return mix(scale(base, 0.28 + 0.78 * dif), WHITE, clamp01(spec + rim));
  };
}
function satin(view, base) {
  const L = view.L, H = view.H, deep = scale(base, 0.42), sheen = mix(base, WHITE, 0.55);
  return (n) => {
    const dif = clamp01((dot(n, L) + 0.35) / 1.35);
    const nh = clamp01(dot(n, H));
    const s = Math.pow(nh, 6) * 0.5 + Math.pow(nh, 40) * 0.35;
    return mix(mix(deep, base, dif), sheen, clamp01(s));
  };
}
function metal(view, tint, dark, glint) {
  const H = view.H, L = view.L;
  return (n) => {
    const rz = 2 * n[2] * n[2] - 1, ry = 2 * n[2] * n[1];
    const up = -ry;
    const sky = clamp01(0.45 + up * 0.85 + rz * 0.1);
    const horizon = Math.exp(-Math.pow((up + 0.12) / 0.16, 2)) * 0.55;
    const nl = clamp01(dot(n, L));
    const c = mix(dark, tint, clamp01(0.1 + sky * 0.72 + nl * 0.3));
    const spec = Math.pow(clamp01(dot(n, H)), 70) + horizon * 0.4;
    return mix(c, glint, clamp01(spec));
  };
}
const gold = (view) => metal(view, [255, 200, 88], [86, 50, 8], [255, 247, 220]);
const silver = (view) => metal(view, [232, 236, 242], [58, 62, 72], [255, 255, 255]);
function gem(view, base) {
  const L = view.L, H = view.H, deep = scale(base, 0.22), clear = mix(base, WHITE, 0.3);
  return (n) => {
    const c = mix(deep, clear, clamp01(clamp01(n[2]) * 0.75 + dot(n, L) * 0.4));
    return mix(c, WHITE, Math.pow(clamp01(dot(n, H)), 24) * 0.95);
  };
}
const SPRITE = {
  gold: [[0, "#fffbe8"], [0.08, "#ffe7a0"], [0.3, "#eeb546"], [0.62, "#ad7214"], [0.87, "#5a3806"], [1, "#8c5c18"]],
  silver: [[0, "#ffffff"], [0.08, "#f3f5f9"], [0.32, "#c6ccd6"], [0.64, "#7a808c"], [0.88, "#3d4149"], [1, "#6b717c"]],
  pearl: [[0, "#ffffff"], [0.1, "#fdf9f7"], [0.45, "#ebe4e6"], [0.8, "#c6bccb"], [1, "#a89db5"]]
};
function sphereSprite(ctx, view, c, rad, kind) {
  const s = view.toScreen(c);
  const H = view.H;
  const g = ctx.createRadialGradient(s[0] + H[0] * rad * 0.85, s[1] + H[1] * rad * 0.85, 0, s[0], s[1], rad);
  if (typeof kind === "string") for (const [t, col] of SPRITE[kind]) g.addColorStop(t, col);
  else {
    g.addColorStop(0, css(mix(kind, WHITE, 0.8)));
    g.addColorStop(0.1, css(mix(kind, WHITE, 0.3)));
    g.addColorStop(0.35, css(kind));
    g.addColorStop(0.85, css(scale(kind, 0.5)));
    g.addColorStop(1, css(scale(kind, 0.62)));
  }
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(s[0], s[1], rad, 0, TAU$2);
  ctx.fill();
}
function cutGem(ctx, view, c, N, T, rad, col) {
  const B = cross(N, T);
  const at = (r, a, h) => {
    const x = r * Math.cos(a), y = r * Math.sin(a);
    return [c[0] + x * T[0] + y * B[0] + h * N[0], c[1] + x * T[1] + y * B[1] + h * N[1], c[2] + x * T[2] + y * B[2] + h * N[2]];
  };
  const K = 8, tr = 0.55, th = 0.36 * rad;
  const girdle = [], table = [];
  for (let k = 0; k < K; k++) {
    const a = (k + 0.5) / K * TAU$2;
    girdle.push(at(rad, a, 0));
    table.push(at(rad * tr, a, th));
  }
  const shade2 = gem(view, col);
  const poly = (pts, n) => {
    const nv2 = view.dir(n);
    if (nv2[2] <= 0.02) return;
    ctx.fillStyle = css(shade2(nv2));
    ctx.beginPath();
    pts.forEach((p, i) => {
      const s = view.toScreen(p);
      if (i) ctx.lineTo(s[0], s[1]);
      else ctx.moveTo(s[0], s[1]);
    });
    ctx.closePath();
    ctx.fill();
    ctx.strokeStyle = ctx.fillStyle;
    ctx.stroke();
  };
  ctx.lineWidth = Math.min(0.3, 1 / view.px);
  ctx.lineJoin = "round";
  for (let k = 0; k < K; k++) {
    const g0 = girdle[k], g1 = girdle[(k + 1) % K], t1 = table[(k + 1) % K], t0 = table[k];
    let n = cross(sub(t1, g0), sub(t0, g1));
    if (dot(n, N) < 0) n = [-n[0], -n[1], -n[2]];
    poly([g0, g1, t1, t0], norm(n));
  }
  poly(table, N);
  const nv = view.dir(N);
  if (nv[2] > 0.1) {
    const s0 = view.toScreen(at(0, 0, th));
    const L = view.L;
    ctx.fillStyle = "rgba(255,255,255,0.3)";
    ctx.beginPath();
    const s1 = view.toScreen(at(rad * tr * 0.8, Math.atan2(-L[1], -L[0]) + 2.2, th));
    const s2 = view.toScreen(at(rad * tr * 0.9, Math.atan2(-L[1], -L[0]) + Math.PI, th));
    const s3 = view.toScreen(at(rad * tr * 0.8, Math.atan2(-L[1], -L[0]) + Math.PI + 0.94, th));
    ctx.moveTo(s0[0], s0[1]);
    ctx.lineTo(s1[0], s1[1]);
    ctx.lineTo(s2[0], s2[1]);
    ctx.lineTo(s3[0], s3[1]);
    ctx.closePath();
    ctx.fill();
  }
}
function seat(m, frac) {
  const W = m.right - m.left;
  for (let i = 0; i < m.widths.length; i++) if (m.widths[i] >= frac * W) return i * m.step - PAD;
  return m.top;
}
const marks = /* @__PURE__ */ new WeakMap();
function marksFor(path) {
  const hit = marks.get(path);
  if (hit) return hit;
  const N = 128;
  const cov = rasterize(path, N);
  if (!cov) return null;
  const u = SPAN / N;
  const toX = (i) => (i + 0.5) * u - PAD;
  const inside = (x, y) => cov[y * N + x] >= 128;
  const c0 = Math.floor((42 + PAD) / u), c1 = Math.ceil((58 + PAD) / u);
  let top = N, bottom = -1;
  for (let x = c0; x <= c1; x++) {
    for (let y = 0; y < N; y++) if (inside(x, y)) {
      if (y < top) top = y;
      break;
    }
    for (let y = N - 1; y >= 0; y--) if (inside(x, y)) {
      if (y > bottom) bottom = y;
      break;
    }
  }
  const row = (y) => {
    let l = -1, r = -1;
    for (let x = 0; x < N; x++) if (inside(x, y)) {
      if (l < 0) l = x;
      r = x;
    }
    return [l, r];
  };
  const yT = Math.min(N - 1, top + Math.round(8 / u));
  const [tl, tr] = row(yT);
  let best = -1, bl = 0, br = 0, by = 0;
  for (let y = Math.round((33 + PAD) / u); y <= Math.round((67 + PAD) / u); y++) {
    const [l, r] = row(y);
    if (l >= 0 && r - l > best) {
      best = r - l;
      bl = l;
      br = r;
      by = y;
    }
  }
  const widths = new Float32Array(N);
  const edge = [];
  for (let y = 0; y < N; y++) {
    const [l, r] = row(y);
    widths[y] = l < 0 ? 0 : (r - l + 1) * u;
    if (l >= 0) edge.push([toX(l) - 0.5 * u, toX(y)], [toX(r) + 0.5 * u, toX(y)]);
  }
  const m = {
    top: toX(top) - 0.5 * u,
    topL: toX(tl),
    topR: toX(tr),
    mid: toX(by),
    left: toX(bl) - 0.5 * u,
    right: toX(br) + 0.5 * u,
    bottom: toX(bottom) + 0.5 * u,
    widths,
    step: u,
    hull: hull(edge)
  };
  marks.set(path, m);
  return m;
}
function tilt(v, roll, lean) {
  const cr = Math.cos(roll), sr = Math.sin(roll);
  const x = v[0] * cr - v[1] * sr, y = v[0] * sr + v[1] * cr;
  const cl = Math.cos(lean), sl = Math.sin(lean);
  return [x, y * cl - v[2] * sl, y * sl + v[2] * cl];
}
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const along = (a, d, k) => [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k];
function polyPath(pts, close = true) {
  const p = new Path2D();
  pts.forEach((q, i) => i ? p.lineTo(q[0], q[1]) : p.moveTo(q[0], q[1]));
  if (close) p.closePath();
  return p;
}
function bounds(pts) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of pts) {
    if (x < x0) x0 = x;
    if (y < y0) y0 = y;
    if (x > x1) x1 = x;
    if (y > y1) y1 = y;
  }
  return { x0, y0, x1, y1, cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, w: x1 - x0, h: y1 - y0 };
}
const rnd = (i, k = 0) => {
  let h = i * 2654435761 + k * 40503 | 0;
  h = Math.imul(h ^ h >>> 15, 2246822507);
  h = Math.imul(h ^ h >>> 13, 3266489909);
  return ((h ^ h >>> 16) >>> 0) / 4294967296;
};
let grain = null;
function grainTile() {
  if (grain) return grain;
  const n = 48;
  const c = typeof OffscreenCanvas === "function" ? new OffscreenCanvas(n, n) : typeof document !== "undefined" ? Object.assign(document.createElement("canvas"), { width: n, height: n }) : null;
  const g = c && c.getContext("2d");
  if (!c || !g) return null;
  const img = g.createImageData(n, n);
  for (let i = 0; i < n * n; i++) {
    const x = i % n, y = i / n | 0;
    let v = 0;
    for (let k = -2; k <= 2; k++) v += rnd((x + k + n) % n + (y + k + n) % n * n, 5);
    v /= 5;
    const m = Math.round(255 * (0.82 + 0.36 * v));
    img.data[i * 4] = img.data[i * 4 + 1] = img.data[i * 4 + 2] = Math.min(255, m);
    img.data[i * 4 + 3] = 255;
  }
  g.putImageData(img, 0, 0);
  grain = c;
  return c;
}
const patterns = /* @__PURE__ */ new WeakMap();
function grainOver(ctx, outline, alpha, scaleK = 0.18) {
  const tile = grainTile();
  if (!tile || outline.length < 3) return;
  let byScale = patterns.get(ctx);
  if (!byScale) patterns.set(ctx, byScale = /* @__PURE__ */ new Map());
  let pat = byScale.get(scaleK);
  if (pat === void 0) {
    pat = ctx.createPattern(tile, "repeat");
    pat?.setTransform?.(new DOMMatrix().scale(scaleK));
    byScale.set(scaleK, pat);
  }
  if (!pat) return;
  const b = bounds(outline);
  ctx.save();
  ctx.clip(polyPath(outline));
  ctx.globalCompositeOperation = "multiply";
  ctx.globalAlpha = alpha;
  ctx.fillStyle = pat;
  ctx.fillRect(b.x0 - 1, b.y0 - 1, b.w + 2, b.h + 2);
  ctx.restore();
}
function fuzz(ctx, outline, base, seed, len, px = 2.4) {
  const n = outline.length;
  if (n < 3) return;
  let total = 0;
  const seg = [];
  for (let i = 0; i < n; i++) {
    const a = outline[i], c = outline[(i + 1) % n];
    const d = Math.hypot(c[0] - a[0], c[1] - a[1]);
    seg.push(d);
    total += d;
  }
  const count = Math.min(360, Math.max(24, Math.round(total * Math.min(2.4, px * 0.5))));
  const b = bounds(outline);
  const lightP = new Path2D(), darkP = new Path2D();
  let si = 0, acc = 0;
  for (let k = 0; k < count; k++) {
    const want = (k + rnd(k, seed) * 0.8) * (total / count);
    while (si < n - 1 && acc + seg[si] < want) acc += seg[si++];
    const a = outline[si], c = outline[(si + 1) % n];
    const t = seg[si] ? (want - acc) / seg[si] : 0;
    const x = a[0] + (c[0] - a[0]) * t, y = a[1] + (c[1] - a[1]) * t;
    let ox = x - b.cx, oy = y - b.cy;
    const ol = Math.hypot(ox, oy) || 1;
    ox /= ol;
    oy /= ol;
    const ang = (rnd(k, seed + 1) - 0.5) * 0.9;
    const ca = Math.cos(ang), sa = Math.sin(ang);
    const hx = ox * ca - oy * sa, hy = ox * sa + oy * ca;
    const l = len * (0.35 + rnd(k, seed + 2));
    const p = rnd(k, seed + 3) > 0.5 ? lightP : darkP;
    p.moveTo(x - hx * 0.4, y - hy * 0.4);
    p.lineTo(x + hx * l, y + hy * l);
  }
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineWidth = 0.4;
  ctx.globalAlpha = 0.55;
  ctx.strokeStyle = shade(base, 0.14);
  ctx.stroke(lightP);
  ctx.strokeStyle = shade(base, -0.06);
  ctx.stroke(darkP);
  ctx.restore();
}
function contact(ctx, clip, cx, cy, rx, ry, a) {
  if (!(rx > 0) || !(ry > 0)) return;
  ctx.save();
  if (clip) ctx.clip(clip);
  ctx.translate(cx, cy);
  ctx.scale(1, ry / rx);
  const g = ctx.createRadialGradient(0, 0, 0, 0, 0, rx);
  g.addColorStop(0, `rgba(0,0,0,${a})`);
  g.addColorStop(0.55, `rgba(0,0,0,${a * 0.45})`);
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(0, 0, rx, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}
const EL = 48 * Math.PI / 180;
function lightOf(lx, ly) {
  const le = Math.cos(EL), lz = Math.sin(EL);
  const L = norm([lx * le, ly * le, lz]);
  return { L, H: norm([L[0], L[1], L[2] + 1]) };
}
function pxOf(ctx) {
  const t = ctx.getTransform?.();
  return t ? Math.sqrt(Math.abs(t.a * t.d - t.b * t.c)) || 1 : 2;
}
function viewOf(r, px) {
  const { L, H } = lightOf(r.lx, r.ly);
  return makeView([r.cy, 0, r.sy, r.sy * r.sp, r.cp, -r.cy * r.sp, -r.sy * r.cp, r.sp, r.cy * r.cp], [50, 50, 0], L, H, px);
}
const kept = /* @__PURE__ */ new WeakMap();
let owner = kept;
function keep(key, make) {
  let c = kept.get(owner);
  if (!c) kept.set(owner, c = /* @__PURE__ */ new Map());
  let g = c.get(key);
  if (!g) {
    if (c.size > 240) c.clear();
    g = make();
    c.set(key, g);
  }
  return g;
}
function surface(view, key, surf, nu, nv, inside, smooth2 = true) {
  return projectMesh(view, keep(`${key}|${nu}x${nv}`, () => makeMesh(surf, nu, nv, inside, smooth2)));
}
function tube(view, key, path, up, prof, nu, nv, closed = false) {
  return projectMesh(view, keep(`${key}|${nu}x${nv}`, () => makeTube(path, up, prof, nu, nv, closed)));
}
const rgbOf = (c, fallback = [40, 40, 44]) => parseColor(c) ?? fallback;
const TAU$1 = Math.PI * 2;
const smooth01 = (t) => t <= 0 ? 0 : t >= 1 ? 1 : t * t * (3 - 2 * t);
const spow = (c, e) => Math.sign(c) * Math.pow(Math.abs(c), e);
function sphereMesh(view, key, c, rad, nu, nv) {
  return surface(
    view,
    key,
    (u, v) => {
      const th = u * TAU$1, ph = v * Math.PI;
      return [c[0] + rad * Math.sin(ph) * Math.cos(th), c[1] - rad * Math.cos(ph), c[2] + rad * Math.sin(ph) * Math.sin(th)];
    },
    nu,
    nv,
    c
  );
}
function ringMesh(view, key, place, R, y, t, zScale, nu, nv) {
  return surface(
    view,
    key,
    (u, v) => {
      const th = u * TAU$1, ph = v * TAU$1;
      const rr = R + t * Math.cos(ph);
      return place([rr * Math.cos(th), y - t * Math.sin(ph), rr * zScale * Math.sin(th)]);
    },
    nu,
    nv,
    (u) => place([R * Math.cos(u * TAU$1), y, R * zScale * Math.sin(u * TAU$1)])
  );
}
function offsetPath(path, up, off) {
  return (u) => {
    const e = 1e-3;
    const T = norm(sub(path(Math.min(1, u + e)), path(Math.max(0, u - e))));
    const B = cross(T, up);
    return along(path(u), B, off(u));
  };
}
function yarnBall(ctx, view, c, rad, base, seed) {
  const rgb = rgbOf(base);
  const n = segs(view, TAU$1 * rad, 12, 20);
  const vis = paintLayers(ctx, [{ parts: [sphereMesh(view, `ball${seed}`, c, rad * 0.86, n, Math.max(6, n >> 1))], shade: felt(view, rgb, 0.92) }], view.px);
  const centre = view.toScreen(c);
  const b = bounds(vis);
  const R = Math.max(b.w, b.h) / 2;
  const count = Math.round(Math.min(340, 50 + R * view.px * 5));
  const light = new Path2D(), dark = new Path2D();
  for (let k = 0; k < count; k++) {
    const a = rnd(k, seed) * TAU$1;
    const r0 = R * (0.25 + 0.7 * Math.sqrt(rnd(k, seed + 1)));
    const len = R * (0.25 + 0.35 * rnd(k, seed + 2));
    const x0 = centre[0] + Math.cos(a) * r0, y0 = centre[1] + Math.sin(a) * r0;
    const bend = (rnd(k, seed + 3) - 0.5) * 0.6;
    const ex = Math.cos(a + bend), ey = Math.sin(a + bend);
    const p = rnd(k, seed + 4) > 0.45 ? light : dark;
    p.moveTo(x0, y0);
    p.quadraticCurveTo(x0 + Math.cos(a) * len * 0.5, y0 + Math.sin(a) * len * 0.5, x0 + ex * len, y0 + ey * len);
  }
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineWidth = Math.max(0.3, R * 0.06);
  const L = view.L;
  const g = ctx.createLinearGradient(centre[0] + L[0] * R, centre[1] + L[1] * R, centre[0] - L[0] * R, centre[1] - L[1] * R);
  g.addColorStop(0, shade(base, 0.2, -0.03));
  g.addColorStop(1, shade(base, -0.12, 0.02));
  ctx.strokeStyle = g;
  ctx.globalAlpha = 0.85;
  ctx.stroke(light);
  ctx.strokeStyle = shade(base, -0.18, 0.03);
  ctx.globalAlpha = 0.6;
  ctx.stroke(dark);
  ctx.restore();
}
function drawBeret(ctx, view, m, base, bodyClip) {
  const W = m.right - m.left;
  const R = 0.46 * W, Hh = 0.21 * W, Rb = 0.33 * W, lift = 0.05 * W;
  const C = [50 - 0.07 * W, Math.max(m.top + 0.22 * W, seat(m, 0.5) + 0.1 * W), 0];
  const roll = -0.24, lean = -0.2;
  const place = (p) => add(C, tilt(p, roll, lean));
  const nu = segs(view, TAU$1 * R, 32, 56), nv = segs(view, 1.2 * R, 10, 32, 6);
  const surf = (u, v) => {
    const th = u * TAU$1;
    let rr, hh;
    const E = 0.74;
    if (v <= E) {
      const phi = v / E * (Math.PI / 2);
      rr = R * Math.sin(phi);
      hh = -(lift + Hh * Math.pow(Math.cos(phi), 0.8));
    } else {
      const f = smooth01((v - E) / (1 - E));
      rr = R + (Rb - R) * f;
      hh = -lift * (1 - f);
    }
    const band = Math.sin(Math.min(1, v / E) * Math.PI * 0.9);
    rr *= 1 + 0.035 * Math.sin(th * 7 + 0.4) * band;
    return place([rr * Math.cos(th), hh, rr * 0.93 * Math.sin(th)]);
  };
  const dome = surface(view, "beret:dome", surf, nu, nv, place([0, -lift - Hh * 0.4, 0]));
  const under = surface(
    view,
    "beret:under",
    (u, v) => {
      const th = u * TAU$1, rr = Rb * (1 - v);
      return place([rr * Math.cos(th), -4e-3 * W * v, rr * 0.93 * Math.sin(th)]);
    },
    24,
    2,
    place([0, -lift - Hh * 0.4, 0])
  );
  const rgb = rgbOf(base);
  const bandMid = view.toScreen(place([0, 0.02 * W, 0]));
  contact(ctx, bodyClip, bandMid[0] + 0.02 * W, bandMid[1] + 0.045 * W, Rb * 1.25, 0.08 * W, 0.4);
  const binding = ringMesh(view, "beret:binding", place, Rb, -5e-3 * W, 0.022 * W, 0.93, segs(view, TAU$1 * Rb, 24, 48), 5);
  const leather = rgbOf(shade(base, -0.06));
  const vis = paintLayers(
    ctx,
    [
      { parts: [under], shade: felt(view, rgb, 0.7) },
      { parts: [binding], shade: plastic(view, leather, 0.45) },
      { parts: [dome], shade: felt(view, rgb) }
    ],
    view.px
  );
  const outline = hull(vis);
  grainOver(ctx, outline, 0.5, 0.14);
  fuzz(ctx, outline, base, 11, 0.4, view.px);
  const x0 = 0.02 * W, y0 = -lift - Hh + 6e-3 * W;
  const stalk = tube(
    view,
    "beret:stalk",
    (u) => place([x0 + 0.014 * W * u * u, y0 - 0.045 * W * u, 0]),
    [0, 0, 1],
    (u, ph) => [0.016 * W * (1 - 0.25 * u) * Math.cos(ph), 0.016 * W * (1 - 0.25 * u) * Math.sin(ph)],
    8,
    3
  );
  paintLayers(ctx, [{ parts: [stalk], shade: felt(view, rgb, 0.95) }], view.px);
  sphereSprite(ctx, view, place([x0 + 0.014 * W, y0 - 0.047 * W, 0]), 0.019 * W, scale(rgb, 1.15));
}
function drawBeanie(ctx, view, m, base, bodyClip) {
  const W = m.right - m.left;
  const headW = Math.max(0.5 * W, m.topR - m.topL);
  const Rd = 0.53 * headW + 0.08 * W, Hd = 0.32 * W, cuffH = 0.1 * W, Rc = Rd * 1.05;
  const C = [50, seat(m, 0.72) + 0.06 * W, 0];
  const roll = 0, lean = -0.14;
  const place = (p) => add(C, tilt(p, roll, lean));
  const domeSurf = (u, v) => {
    const th = u * TAU$1, phi = v * (Math.PI / 2);
    const rr = Rd * Math.sin(phi) * (1 - 0.03 * (1 - v));
    const hh = -(cuffH + Hd * Math.cos(phi));
    return place([rr * Math.cos(th), hh, rr * 0.92 * Math.sin(th)]);
  };
  const inDome = place([0, -cuffH - Hd * 0.4, 0]);
  const dome = surface(view, "beanie:dome", domeSurf, segs(view, TAU$1 * Rd, 28, 48), segs(view, 1.3 * Hd, 9, 24, 6), inDome);
  const cuffSurf = (u, v) => {
    const th = u * TAU$1;
    const e = 2 * v - 1;
    const rr = Rc * (1 - 0.07 * e * e * e * e) * 1.01;
    return place([rr * Math.cos(th), -cuffH * (1 - v), rr * 0.92 * Math.sin(th)]);
  };
  const cuff = surface(view, "beanie:cuff", cuffSurf, segs(view, TAU$1 * Rc, 28, 56), 4, place([0, -cuffH / 2, 0]));
  const rgb = rgbOf(base), cuffRgb = rgbOf(shade(base, 0.03));
  const bb = boundsOf([cuff]);
  contact(ctx, bodyClip, bb.cx, bb.y1 - 0.01 * W, bb.w * 0.5, 0.06 * W, 0.34);
  const visDome = paintLayers(ctx, [{ parts: [dome], shade: felt(view, rgb, 0.96) }], view.px);
  const NU = 40, NV = 11;
  const cell = Rd * TAU$1 / NU * view.px;
  if (cell >= 2.2) {
    const grid = surface(view, "beanie:stitch", domeSurf, NU, NV, inDome, false);
    const dark = new Path2D(), light = new Path2D();
    const GW = NU + 1;
    for (let c = 0; c < NU * NV; c++) {
      if (grid.f[c * 3 + 2] < 0.12) continue;
      const k = Math.floor(c / NU) * GW + c % NU;
      const A = [grid.x[k], grid.y[k]], B = [grid.x[k + 1], grid.y[k + 1]];
      const Cq = [grid.x[k + GW + 1], grid.y[k + GW + 1]], D = [grid.x[k + GW], grid.y[k + GW]];
      const at = (s, t) => {
        const top = [A[0] + (B[0] - A[0]) * s, A[1] + (B[1] - A[1]) * s];
        const bot = [D[0] + (Cq[0] - D[0]) * s, D[1] + (Cq[1] - D[1]) * s];
        return [top[0] + (bot[0] - top[0]) * t, top[1] + (bot[1] - top[1]) * t];
      };
      for (const [p, dx] of [[dark, 0], [light, -0.06]]) {
        const a = at(0.14 + dx, 0.12), b = at(0.5 + dx, 0.88), c2 = at(0.86 + dx, 0.12);
        p.moveTo(a[0], a[1]);
        p.lineTo(b[0], b[1]);
        p.lineTo(c2[0], c2[1]);
      }
    }
    const cw = Rd * TAU$1 / NU;
    ctx.save();
    ctx.clip(polyPath(hull(visDome)));
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.lineWidth = Math.max(0.35, cw * 0.2);
    ctx.strokeStyle = shade(base, -0.22, 0.03);
    ctx.globalAlpha = 0.5;
    ctx.stroke(dark);
    ctx.lineWidth = Math.max(0.3, cw * 0.12);
    ctx.strokeStyle = shade(base, 0.2, -0.03);
    ctx.globalAlpha = 0.35;
    ctx.stroke(light);
    ctx.restore();
  }
  const visCuff = paintLayers(ctx, [{ parts: [cuff], shade: felt(view, cuffRgb, 0.98) }], view.px);
  const ribGap = TAU$1 * Rc / 30;
  if (ribGap * view.px >= 3) {
    const groove = new Path2D(), ridge = new Path2D();
    for (let k = 0; k < 60; k++) {
      const u = k / 60;
      const th = u * TAU$1;
      if (view.dir(tilt([Math.cos(th), 0, 0.92 * Math.sin(th)], roll, lean))[2] < 0.08) continue;
      const a = view.toScreen(cuffSurf(u, 0.1)), b = view.toScreen(cuffSurf(u, 0.9));
      const p = k % 2 ? groove : ridge;
      p.moveTo(a[0], a[1]);
      p.lineTo(b[0], b[1]);
    }
    ctx.save();
    ctx.lineCap = "round";
    ctx.lineWidth = ribGap * 0.34;
    ctx.strokeStyle = shade(base, -0.2, 0.03);
    ctx.globalAlpha = 0.45;
    ctx.stroke(groove);
    ctx.lineWidth = ribGap * 0.22;
    ctx.strokeStyle = shade(base, 0.16, -0.02);
    ctx.globalAlpha = 0.4;
    ctx.stroke(ridge);
    ctx.restore();
  }
  const outline = hull(visDome.concat(visCuff));
  fuzz(ctx, outline, base, 23, 0.35, view.px);
  yarnBall(ctx, view, place([0, -cuffH - Hd - 0.055 * W, 0]), 0.085 * W, shade(base, 0.06), 31);
}
function drawParty(ctx, view, m, base, bodyClip) {
  const W = m.right - m.left;
  const C = [50 + 0.06 * W, seat(m, 0.3) + 0.04 * W, 0];
  const R = 0.18 * W, H = Math.max(0.3 * W, Math.min(0.46 * W, C[1] + 31 - 0.1 * W));
  const roll = 0.18, lean = -0.12;
  const place = (p) => add(C, tilt(p, roll, lean));
  const coneAt = (u, v, grow = 1) => {
    const th = u * TAU$1;
    const rr = R * (1 - v) * (1 + 0.04 * Math.sin(v * Math.PI)) * grow;
    return place([rr * Math.cos(th), -H * v, rr * Math.sin(th)]);
  };
  const inCone = (_u, v) => place([0, -H * Math.min(0.97, v), 0]);
  const nAround = segs(view, TAU$1 * R, 20, 40), nUp = segs(view, H, 10, 24, 6);
  const cone = surface(view, "party:cone", (u, v) => coneAt(u, v), nAround, nUp, inCone);
  const a = rgbOf(base), b = [252, 246, 234];
  const bb = boundsOf([cone]);
  contact(ctx, bodyClip, bb.cx, bb.y1, R * 1.15, 0.04 * W, 0.3);
  const trim = ringMesh(view, "party:trim", place, R * 1.02, 4e-3 * W, 0.016 * W, 1, segs(view, TAU$1 * R, 20, 40), 5);
  const stripes = [];
  const K = 6, twist = 3.2;
  for (let k = 0; k < K; k += 1) {
    stripes.push(surface(view, `party:stripe${k}`, (s, t) => coneAt((k + 0.5 * t - twist * s * 0.97) / K, s * 0.97, 1.004), nUp + 3, 1, inCone));
  }
  const vis = paintLayers(
    ctx,
    [
      { parts: [cone], shade: paper(view, b) },
      { parts: [trim], shade: gold(view) }
    ],
    view.px
  );
  paintLayers(ctx, [{ parts: stripes, shade: paper(view, a) }], view.px);
  grainOver(ctx, hull(vis), 0.18, 0.1);
  yarnBall(ctx, view, place([0, -H - 0.035 * W, 0]), 0.06 * W, "#f4efe6", 37);
}
function drawCrown(ctx, view, m, bodyClip) {
  const W = m.right - m.left;
  const headW = Math.max(0.45 * W, m.topR - m.topL);
  const R = 0.44 * headW + 0.07 * W, bandH = 0.11 * W, pointH = 0.15 * W, ez = 0.9;
  const C = [50, seat(m, 0.42) + 0.05 * W, 0];
  const roll = 0.05, lean = -0.22;
  const place = (p) => add(C, tilt(p, roll, lean));
  const dirOf = (v) => norm(tilt(v, roll, lean));
  const on = (th, rr, y) => place([rr * Math.cos(th), y, rr * ez * Math.sin(th)]);
  const axis = (y) => place([0, y, 0]);
  const nu = segs(view, TAU$1 * R, 32, 64);
  const band = surface(view, "crown:band", (u, v) => on(u * TAU$1, R * (1 - 0.018 * Math.sin(v * Math.PI)), -bandH * (1 - v)), nu, 2, (_u, v) => axis(-bandH * (1 - v)));
  const rimT = 0.017 * W;
  const rimTop = ringMesh(view, "crown:rimTop", place, R + 4e-3 * W, -bandH, rimT, ez, nu, 5);
  const rimBot = ringMesh(view, "crown:rimBot", place, R + 4e-3 * W, 0, rimT * 1.15, ez, nu, 5);
  const capBase = -bandH * 0.55;
  const capH = Math.max(0.12 * W, C[1] - m.top + 0.04 * W + capBase);
  const P0 = Math.PI / 2;
  const cap = surface(
    view,
    "crown:cap",
    (u, v) => {
      const th = u * TAU$1, phi = v * (Math.PI / 2);
      const puff = 0.08 * Math.sin(2 * phi) * (0.55 + 0.45 * Math.cos(5 * (th - P0 - Math.PI / 5)));
      return on(th, R * (0.9 * Math.sin(phi) + puff), capBase - capH * Math.cos(phi));
    },
    segs(view, TAU$1 * R, 24, 48),
    segs(view, 1.4 * capH, 8, 20, 6),
    axis(capBase - capH * 0.4)
  );
  const points = [], buds = [];
  const pearls = [], budTops = [];
  const plate = (key, a0, w, h, ridge, flare, out) => {
    out.push(
      surface(
        view,
        key,
        (u, v) => {
          const s = u * 2 - 1;
          const th = a0 + s * w * Math.pow(1 - v, 1.3);
          const rr = R + 4e-3 * W + ridge * (1 - Math.abs(s)) * (1 - 0.5 * v) + flare * v * v;
          return on(th, rr, -bandH - h * v);
        },
        2,
        segs(view, h, 3, 5),
        (_u, v) => on(a0, R * 0.5, -bandH - h * v),
        false
      )
    );
  };
  for (let i = 0; i < 5; i++) {
    const a0 = P0 + i / 5 * TAU$1;
    plate(`crown:point${i}`, a0, TAU$1 / 5 * 0.34, pointH, 0.05 * W, 0.03 * W, points);
    pearls.push(on(a0, R + 4e-3 * W + 0.03 * W + 0.02 * W, -bandH - pointH - 0.018 * W));
    const b0 = a0 + Math.PI / 5;
    plate(`crown:bud${i}`, b0, TAU$1 / 5 * 0.12, pointH * 0.36, 0.02 * W, 0.01 * W, buds);
    budTops.push(on(b0, R + 4e-3 * W + 0.012 * W, -bandH - pointH * 0.36 - 0.012 * W));
  }
  const bb = boundsOf([band]);
  contact(ctx, bodyClip, bb.cx, bb.y1, bb.w * 0.5, 0.045 * W, 0.34);
  const mid = view.depth(axis(capBase));
  const g = gold(view);
  const facet = metal(view, [255, 196, 80], [92, 54, 10], [255, 222, 150]);
  const goldBoth = (n) => facet(n[2] < 0 ? [-n[0], -n[1], -n[2]] : n);
  const pearlR = 0.024 * W, budR = 0.019 * W;
  for (const p of pearls) if (view.depth(p) < mid) sphereSprite(ctx, view, p, pearlR, "pearl");
  for (const p of budTops) if (view.depth(p) < mid) sphereSprite(ctx, view, p, budR, "gold");
  paintLayers(
    ctx,
    [
      { parts: [cap], shade: velvet(view, [150, 22, 44]) },
      { parts: [band], shade: g, bias: 0.01 * W },
      { parts: [rimTop], shade: g, bias: 0.012 * W },
      { parts: [rimBot], shade: g, bias: 0.012 * W },
      { parts: points, shade: goldBoth, cull: false, flat: true },
      { parts: buds, shade: goldBoth, cull: false, flat: true }
    ],
    view.px
  );
  const stones = [[208, 20, 60], [30, 150, 92], [36, 90, 220], [36, 90, 220], [30, 150, 92]];
  for (let i = 0; i < 5; i++) {
    for (const [th, kind] of [[P0 + i / 5 * TAU$1, "stone"], [P0 + i / 5 * TAU$1 + Math.PI / 5, "stud"]]) {
      const N = dirOf([Math.cos(th), 0, ez * Math.sin(th)]);
      if (view.dir(N)[2] < 0.12) continue;
      const c = on(th, R + 0.01 * W, -bandH * 0.5);
      if (kind === "stud") {
        sphereSprite(ctx, view, c, 0.013 * W, "gold");
        continue;
      }
      const size = (i === 0 ? 0.034 : 0.027) * W;
      sphereSprite(ctx, view, c, size * 1.3, "gold");
      const T = dirOf([-Math.sin(th), 0, ez * Math.cos(th)]);
      cutGem(ctx, view, along(c, N, 6e-3 * W), N, T, size, stones[i]);
    }
  }
  for (const p of pearls) if (view.depth(p) >= mid) sphereSprite(ctx, view, p, pearlR, "pearl");
  for (const p of budTops) if (view.depth(p) >= mid) sphereSprite(ctx, view, p, budR, "gold");
  const orbR = 0.034 * W;
  const orb = axis(capBase - capH - orbR * 0.75);
  const up = dirOf([0, -1, 0]);
  const bar = (key, a, b) => tube(view, key, (u) => [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u], dirOf([0, 0, 1]), section(9e-3 * W, 9e-3 * W, 0.5), 2, 6);
  const c0 = along(orb, up, orbR * 0.8), c1 = along(orb, up, orbR * 0.8 + 0.075 * W);
  const cm = along(orb, up, orbR * 0.8 + 0.05 * W), side = dirOf([1, 0, 0]);
  sphereSprite(ctx, view, orb, orbR, "gold");
  paintLayers(
    ctx,
    [
      { parts: [bar("crown:barV", c0, c1)], shade: g },
      { parts: [bar("crown:barH", along(cm, side, -0.026 * W), along(cm, side, 0.026 * W))], shade: g }
    ],
    view.px
  );
}
function phonesOf(view, m, base) {
  const W = m.right - m.left;
  const rc = 0.15 * W, oval = 1.1, pad = 0.05 * W, tc = 0.068 * W;
  const yc = m.mid - 0.07 * W;
  const tilt2 = 0.5;
  const shellRgb = rgbOf(base), padRgb = rgbOf(shade(base, -0.12)), plateRgb = rgbOf(shade(base, 0.08));
  const nu = segs(view, TAU$1 * rc, 20, 40);
  const layers = [];
  const pivots = [];
  const tops = [];
  const seats = [];
  const g = silver(view);
  for (const s of [-1, 1]) {
    const A = [s * Math.cos(tilt2), 0, Math.sin(tilt2)];
    const E1 = [0, 1, 0];
    const E2 = [-Math.sin(tilt2), 0, s * Math.cos(tilt2)];
    const O = [(s < 0 ? m.left : m.right) - s * 0.02 * W, yc, 0.01 * W];
    const at = (t, rho, th) => {
      const c = Math.cos(th) * oval * rho, sn = Math.sin(th) * rho;
      return [O[0] + A[0] * t + E1[0] * c + E2[0] * sn, O[1] + A[1] * t + E1[1] * c + E2[1] * sn, O[2] + A[2] * t + E1[2] * c + E2[2] * sn];
    };
    const axis = (t) => along(O, A, t);
    seats.push({ c: O, ry: rc * oval });
    const rho0 = rc * 0.9 - pad * 0.5;
    const creases = nu >= 48 ? 1 : 0;
    const cushion = surface(
      view,
      `phones:cushion${s}`,
      (u, v) => {
        const th = u * TAU$1, a = v * Math.PI;
        const crease = 1 + 6e-3 * creases * Math.sin(th * 24) * Math.sin(a);
        return at(pad * 0.5 * (1 - Math.cos(a)), (rho0 + pad * 0.62 * Math.sin(a)) * crease, th);
      },
      nu,
      5,
      (_u, v) => axis(pad * 0.5 * (1 - Math.cos(v * Math.PI)))
    );
    const shell = surface(
      view,
      `phones:shell${s}`,
      (u, v) => {
        const a = v * (Math.PI / 2);
        return at(pad + tc * Math.pow(Math.sin(a), 0.55), rc * (0.7 + 0.3 * Math.pow(Math.cos(a), 0.55)), u * TAU$1);
      },
      nu,
      5,
      axis(pad + tc * 0.5)
    );
    const ringR = 0.66 * rc, ringT = 0.017 * W;
    const ring = surface(
      view,
      `phones:ring${s}`,
      (u, v) => at(pad + tc + ringT * 0.45 * Math.sin(v * TAU$1), ringR + ringT * Math.cos(v * TAU$1), u * TAU$1),
      nu,
      5,
      (u) => at(pad + tc, ringR, u * TAU$1)
    );
    const plateR = ringR - ringT * 0.5;
    const plate = surface(view, `phones:plate${s}`, (u, v) => at(pad + tc + 0.013 * W * (1 - v * v), plateR * v, u * TAU$1), nu, 3, axis(pad + tc - 0.02 * W));
    const ty = pad + tc * 0.45, ry = rc * 1.04 + 0.022 * W;
    const yokeAt = (u) => {
      const a = u * Math.PI;
      const c = -Math.sin(a) * oval * ry, sn = Math.cos(a) * ry;
      return [O[0] + A[0] * ty + E1[0] * c + E2[0] * sn, O[1] + A[1] * ty + E1[1] * c + E2[1] * sn, O[2] + A[2] * ty + E1[2] * c + E2[2] * sn];
    };
    const yoke = tube(view, `phones:yoke${s}`, yokeAt, A, section(0.013 * W, 8e-3 * W, 0.6), segs(view, Math.PI * ry, 12, 24), 6);
    pivots.push(yokeAt(0), yokeAt(1));
    tops.push(yokeAt(0.5));
    layers.push(
      { parts: [cushion], shade: plastic(view, padRgb, 0.35) },
      { parts: [shell], shade: plastic(view, shellRgb, 0.8) },
      { parts: [ring], shade: g, bias: 0.02 * W },
      { parts: [plate], shade: plastic(view, plateRgb, 1.3), bias: 0.01 * W },
      { parts: [yoke], shade: g }
    );
  }
  const [PL, PR] = tops;
  const zb = (PL[2] + PR[2]) / 2, cx = (PL[0] + PR[0]) / 2, ax = (PR[0] - PL[0]) / 2, yEnd = (PL[1] + PR[1]) / 2;
  const padT = (u) => 5e-3 * W + 0.022 * W * Math.pow(Math.sin(Math.PI * u), 0.35);
  const e = 2 / 2.2;
  const topAt = (x) => {
    let y = Infinity;
    const h = m.hull;
    for (let i = 0; i < h.length; i++) {
      const p = h[i], q = h[(i + 1) % h.length];
      if (p[0] === q[0] || x < Math.min(p[0], q[0]) || x > Math.max(p[0], q[0])) continue;
      y = Math.min(y, p[1] + (q[1] - p[1]) * (x - p[0]) / (q[0] - p[0]));
    }
    return y;
  };
  let lift = yEnd - m.top;
  for (let i = 1; i < 64; i++) {
    const t = Math.PI * (1 - i / 64);
    const s = Math.pow(Math.abs(Math.sin(t)), e);
    const yt = topAt(cx + ax * spow(Math.cos(t), e));
    if (s > 0.05 && Number.isFinite(yt)) lift = Math.max(lift, (yEnd - yt + 2 * padT(i / 64) + 6e-3 * W) / s);
  }
  const arch = (u) => {
    const t = Math.PI * (1 - u);
    return [cx + ax * spow(Math.cos(t), e), yEnd - lift * Math.pow(Math.abs(Math.sin(t)), e), zb];
  };
  const part = (u0, u1) => (u) => arch(u0 + (u1 - u0) * u);
  const Z = [0, 0, 1];
  const span = ax * 2 + lift * 2;
  const nb = segs(view, span, 24, 48);
  const us = 0.09;
  const shellOff = 0.016 * W;
  const shellPath = offsetPath(part(us, 1 - us), Z, () => shellOff);
  const taper = (u) => smooth01(Math.min(u, 1 - u) / 0.07);
  const bandShell = tube(
    view,
    "phones:bandShell",
    shellPath,
    Z,
    (u, ph) => {
      const k = taper(u);
      return [(0.024 + 0.016 * k) * W * spow(Math.cos(ph), 0.4), (8e-3 + 8e-3 * k) * W * spow(Math.sin(ph), 0.4)];
    },
    nb,
    6
  );
  const cushionPath = offsetPath(part(us + 0.02, 1 - us - 0.02), Z, (u) => -padT(us + 0.02 + (1 - 2 * us - 0.04) * u));
  const bandPad = tube(
    view,
    "phones:bandPad",
    cushionPath,
    Z,
    (u, ph) => [0.034 * W * spow(Math.cos(ph), 0.5), padT(us + 0.02 + (1 - 2 * us - 0.04) * u) * spow(Math.sin(ph), 0.7)],
    nb,
    6
  );
  const sliders = [part(0, us + 0.03), part(1 - us - 0.03, 1)].map((p, i) => tube(view, `phones:slider${i}`, p, Z, section(0.022 * W, 6e-3 * W, 0.4), 4, 6));
  layers.push(
    { parts: [bandShell], shade: plastic(view, shellRgb, 0.9) },
    { parts: [bandPad], shade: plastic(view, padRgb, 0.3) },
    { parts: sliders, shade: g }
  );
  const stitch = [];
  for (let i = 0; i <= 48; i++) stitch.push(along(cushionPath(0.04 + 0.92 * i / 48), Z, 0.034 * W + 1e-3 * W));
  return { layers, pivots, pivotR: 0.017 * W, stitch, seats };
}
let lastPhones = null;
function drawPhones(ctx, view, m, base, pass, bodyClip) {
  const key = `${view.m.join()}|${view.px}|${view.L.join()}|${base}`;
  if (!lastPhones || lastPhones.m !== m || lastPhones.key !== key) lastPhones = { m, key, ph: phonesOf(view, m, base) };
  const ph = lastPhones.ph;
  const W = m.right - m.left;
  const near = (d) => pass === "front" ? d >= 0 : d < 0;
  if (pass === "front") {
    for (const s of ph.seats) {
      if (view.depth(s.c) < 0) continue;
      const c = view.toScreen(s.c);
      contact(ctx, bodyClip, c[0], c[1] + 0.01 * W, 0.06 * W, s.ry * 1.05, 0.26);
    }
  }
  paintLayers(
    ctx,
    ph.layers.map((l) => ({ ...l, keep: near })),
    view.px
  );
  for (const p of ph.pivots) if (near(view.depth(p))) sphereSprite(ctx, view, p, ph.pivotR, "silver");
  if (pass === "front" && W * view.px > 120 && view.dir([0, 0, 1])[2] > 0.3) {
    const pts = ph.stitch.map((p) => view.toScreen(p));
    ctx.save();
    ctx.setLineDash([0.022 * W, 0.014 * W]);
    ctx.lineCap = "round";
    ctx.lineWidth = 45e-4 * W;
    ctx.strokeStyle = shade(base, 0.12);
    ctx.globalAlpha = 0.55;
    ctx.stroke(polyPath(pts, false));
    ctx.restore();
  }
}
function drawBowTie(ctx, view, r, m, base, bodyZ, bodyClip, face, fabricBow = false) {
  if (r.facing < 0.05) return;
  const W = m.right - m.left;
  const clear = face.y + 22 * face.scale;
  const cyB = Math.max(clear, m.bottom - 0.15 * W);
  const room = Math.max(0, m.bottom - 3 - cyB);
  const k = Math.min(W, 100) * Math.max(0.5, Math.min(1, room / 15));
  const zFront = bodyZ(50, cyB);
  const Q = (x, y, z) => [50 + x * k, cyB + y * k, zFront + z * k];
  const rgb = rgbOf(base);
  const corners = [Q(-0.36, -0.18, 0), Q(0.36, 0.18, 0), Q(-0.36, 0.18, 0), Q(0.36, -0.18, 0)].map((p) => view.toScreen(p));
  const bb = bounds(corners);
  const sh = view.toScreen(Q(0.01, 0.08, 0));
  contact(ctx, bodyClip, sh[0], sh[1], bb.w * 0.52, bb.h * 0.5, 0.3);
  const nu = segs(view, 0.32 * k, 8, 16), nv = segs(view, 0.3 * k, 8, 16, 6);
  const at = `${zFront.toFixed(2)},${cyB.toFixed(2)},${k.toFixed(2)}`;
  const soft2 = !!fabricBow;
  const wingShade = soft2 ? felt(view, rgb) : satin(view, rgb);
  const layers = [];
  for (const dir of [-1, 1]) {
    const wing = (u, v) => {
      const x = 0.058 + 0.3 * u - 0.035 * Math.pow(u, 4) * Math.sin(Math.PI * v);
      const h = 0.05 + 0.12 * Math.pow(u, 0.8);
      const y = -h * Math.cos(Math.PI * v) + 0.012 * u * u;
      const bulge = 0.018 + 0.058 * Math.sin(Math.PI * (0.12 + 0.88 * u));
      const fold = 0.016 * Math.sin(3 * Math.PI * v) * Math.pow(1 - u, 1.3);
      return Q(dir * x, y, (bulge + fold) * Math.sin(Math.PI * v) + 4e-3);
    };
    layers.push({ parts: [surface(view, `bow:wing${dir}@${at}`, wing, nu, nv, (u) => Q(dir * (0.058 + 0.3 * u), 0, -0.03))], shade: wingShade });
    layers.push({
      parts: [surface(
        view,
        `bow:end${dir}@${at}`,
        (s, t) => {
          const p = wing(1, t);
          return [p[0], p[1], p[2] - (p[2] - zFront) * s];
        },
        1,
        nv,
        Q(dir * 0.2, 0, -0.03)
      )],
      shade: wingShade
    });
  }
  const knot = surface(
    view,
    `bow:knot@${at}`,
    (u, v) => {
      const x = 0.068 * (2 * u - 1) * (1 - 0.12 * Math.sin(Math.PI * v));
      const y = 0.075 * (2 * v - 1);
      const zx = Math.pow(Math.max(0, 1 - Math.pow(Math.abs(2 * u - 1), 3)), 0.5);
      const zy = Math.pow(Math.max(0, 1 - Math.pow(Math.abs(2 * v - 1), 5)), 0.4);
      return Q(x, y, 0.012 + 0.075 * zx * zy);
    },
    6,
    6,
    Q(0, 0, -0.02)
  );
  layers.push({ parts: [knot], shade: soft2 ? felt(view, rgb, 0.9) : satin(view, scale(rgb, 0.9)) });
  if (!soft2) {
    paintLayers(ctx, layers, view.px);
    return;
  }
  for (let i = 0; i < layers.length; i += 2) {
    const part = layers.slice(i, i + 2);
    const outline = hull(paintLayers(ctx, part, view.px));
    grainOver(ctx, outline, 0.5, 0.1);
    fuzz(ctx, outline, base, 41 + i, 0.3, view.px);
  }
}
const FRAMES = {
  round: { rx: 10.8, ry: 10.8, n: 2, tr: 1.15, td: 1.2 },
  square: { rx: 12, ry: 9.6, n: 4.2, tr: 1.45, td: 1.4 },
  shades: { rx: 12.6, ry: 10, n: 3.4, tr: 1.6, td: 1.5 }
};
function drawGlasses(ctx, kind, eyes, light, pass, turn = { yaw: 0, pitch: 0 }) {
  if (kind === "none") return;
  owner = FRAMES;
  const F = FRAMES[kind];
  const cy = Math.cos(turn.yaw), sy = Math.sin(turn.yaw), cp = Math.cos(turn.pitch), sp = Math.sin(turn.pitch);
  const { L: lightL, H: lightH } = lightOf(light.lx, light.ly);
  const view = makeView([cy, 0, sy, sp * sy, cp, -sp * cy, -cp * sy, sp, cp * cy], [0, 0, 0], lightL, lightH, pxOf(ctx));
  const zPlane = Math.max(eyes[0].fz, eyes[1].fz) + 3.2;
  const zAt = (x) => zPlane - 45e-4 * x * x;
  const e2 = 2 / F.n;
  const rimAt = (e, grow) => (u) => {
    const t = u * TAU$1;
    const x = e.ox + (F.rx + grow) * spow(Math.cos(t), e2), y = e.oy + (F.ry + grow) * spow(Math.sin(t), e2);
    return [x, y, zAt(x)];
  };
  const ring = (e, grow, n = 48) => {
    const f = rimAt(e, grow);
    const pts = [];
    for (let i = 0; i < n; i++) pts.push(view.toScreen(f(i / n)));
    return pts;
  };
  const vis = eyes.map((e) => Math.max(0, Math.min(1, (e.z - 0.18) * 3.2)));
  if (pass === "shadow") {
    for (let i = 0; i < 2; i++) {
      if (vis[i] <= 0) continue;
      ctx.save();
      ctx.globalAlpha = 0.22 * vis[i];
      ctx.translate(-light.lx * 1.6, -light.ly * 1.6 + 0.8);
      ctx.lineWidth = F.tr * 2 + 0.6;
      ctx.lineJoin = "round";
      ctx.strokeStyle = "#000";
      ctx.stroke(polyPath(ring(eyes[i], 0)));
      ctx.restore();
    }
    return;
  }
  const frame2 = [22, 22, 26];
  const rimShade = plastic(view, frame2, 1.1);
  for (let i = 0; i < 2; i++) {
    if (vis[i] <= 0) continue;
    const lens = polyPath(ring(eyes[i], -F.tr * 0.5));
    const b = bounds(ring(eyes[i], 0, 16));
    ctx.save();
    ctx.globalAlpha = vis[i];
    if (kind === "shades") {
      const g = ctx.createLinearGradient(b.cx, b.y0, b.cx, b.y1);
      g.addColorStop(0, "rgba(58,60,78,0.97)");
      g.addColorStop(0.55, "rgba(18,18,26,0.98)");
      g.addColorStop(1, "rgba(30,26,44,0.98)");
      ctx.fillStyle = g;
    } else {
      const g = ctx.createLinearGradient(b.cx, b.y0, b.cx, b.y1);
      g.addColorStop(0, "rgba(236,244,255,0.26)");
      g.addColorStop(0.45, "rgba(214,230,255,0.08)");
      g.addColorStop(1, "rgba(214,230,255,0.14)");
      ctx.fillStyle = g;
    }
    ctx.fill(lens);
    ctx.clip(lens);
    const w = b.w, h = b.h;
    const streak = (x0, width, a) => {
      ctx.globalAlpha = vis[i] * a;
      ctx.fillStyle = "#fff";
      ctx.beginPath();
      ctx.moveTo(b.x0 + w * x0, b.y1 + h * 0.1);
      ctx.lineTo(b.x0 + w * (x0 + 0.55), b.y0 - h * 0.1);
      ctx.lineTo(b.x0 + w * (x0 + 0.55 + width), b.y0 - h * 0.1);
      ctx.lineTo(b.x0 + w * (x0 + width), b.y1 + h * 0.1);
      ctx.closePath();
      ctx.fill();
    };
    const k = kind === "shades" ? 1.4 : 1;
    streak(-0.28, 0.26, 0.16 * k);
    streak(0.06, 0.07, 0.3 * k);
    ctx.restore();
  }
  const nr = segs(view, TAU$1 * F.rx * 1.1, 28, 56, 8);
  const rim = (e) => tube(view, `rim:${kind}:${e.ox},${e.oy},${e.fz.toFixed(2)}`, rimAt(e, 0), [0, 0, 1], section(F.td, F.tr, 0.55), nr, 6, true);
  const [L, R] = eyes;
  const bridgeAt = (u) => {
    const round = kind === "round";
    const a = [L.ox + F.rx * (round ? 0.9 : 0.98), L.oy - F.ry * (round ? 0.32 : 0.42), 0];
    const b = [R.ox - F.rx * (round ? 0.9 : 0.98), R.oy - F.ry * (round ? 0.32 : 0.42), 0];
    const c = [0, (L.oy + R.oy) / 2 - F.ry * (round ? 0.8 : 0.6), 0];
    const m1 = 1 - u;
    const x = m1 * m1 * a[0] + 2 * m1 * u * c[0] + u * u * b[0], y = m1 * m1 * a[1] + 2 * m1 * u * c[1] + u * u * b[1];
    return [x, y, zAt(x) + 0.3];
  };
  const bridge = tube(view, `bridge:${kind}:${L.ox},${L.oy},${R.ox},${zPlane.toFixed(2)}`, bridgeAt, [0, 0, 1], section(F.td * 0.8, F.tr * 0.75, 0.7), 8, 6);
  const parts = [
    { part: rim(L), a: vis[0], d: view.depth([L.ox, L.oy, zPlane]) },
    { part: rim(R), a: vis[1], d: view.depth([R.ox, R.oy, zPlane]) },
    { part: bridge, a: Math.min(vis[0], vis[1]), d: view.depth([0, L.oy, zPlane]) }
  ].sort((p, q) => p.d - q.d);
  for (const p of parts) {
    if (p.a <= 0) continue;
    ctx.save();
    ctx.globalAlpha = p.a;
    paintLayers(ctx, [{ parts: [p.part], shade: rimShade }], view.px);
    ctx.restore();
  }
  for (const [e, s, a] of [[L, -1, vis[0]], [R, 1, vis[1]]]) {
    if (a <= 0) continue;
    ctx.save();
    ctx.globalAlpha = a;
    const x = e.ox + s * F.rx * 0.93;
    sphereSprite(ctx, view, [x, e.oy - F.ry * 0.5, zAt(x) + F.td], 0.75, "silver");
    ctx.restore();
  }
}
function drawWearBehind(ctx, path, r, wear) {
  if (!wear.headphones) return;
  const m = marksFor(path);
  if (!m) return;
  owner = m;
  drawPhones(ctx, viewOf(r, pxOf(ctx)), m, wear.color, "behind", null);
}
function drawWearFront(ctx, path, r, wear, bodyClip, bodyZ, face) {
  const m = marksFor(path);
  if (!m) return;
  owner = m;
  const view = viewOf(r, pxOf(ctx));
  if (wear.bowTie) drawBowTie(ctx, view, r, m, wear.color, bodyZ, bodyClip, face, wear.fabric);
  if (wear.headphones) drawPhones(ctx, view, m, wear.color, "front", bodyClip);
  switch (wear.hat) {
    case "beret":
      drawBeret(ctx, view, m, wear.color, bodyClip);
      break;
    case "beanie":
      drawBeanie(ctx, view, m, wear.color, bodyClip);
      break;
    case "party":
      drawParty(ctx, view, m, wear.color, bodyClip);
      break;
    case "crown":
      drawCrown(ctx, view, m, bodyClip);
      break;
  }
}
const OVERSCAN = 1.5;
const RISE = 0.1;
const SLICES = 17;
const HALF_DEPTH = 15;
const CAP = 0.9;
const profile = (z, cap) => cap + (1 - cap) * Math.sqrt(Math.max(0, 1 - z * z));
const EYE_GAP = 25;
const EYE_RX = 6.3;
const EYE_Y = { eyes: 1, mouth: -3.5 };
const paletteCache = /* @__PURE__ */ new Map();
function palette(color, shadow, highlight) {
  const key = `${color}|${shadow}|${highlight}`;
  let p = paletteCache.get(key);
  if (!p) {
    const far = shade(color, -0.3 * shadow, 0.05 * shadow);
    const near = shade(color, -0.12 * shadow, 0.03 * shadow);
    const crispMix = [], smoothMix = [];
    for (let j = 0; j < SLICES; j++) {
      const t = j / (SLICES - 1);
      crispMix.push(t > 0.6 ? "" : mixCss(far, near, t / 0.6));
      smoothMix.push(t >= 0.5 ? color : mixCss(far, color, t / 0.5));
    }
    p = {
      base: color,
      far,
      near,
      light: shade(color, 0.04 * highlight),
      dark: shade(color, -0.3 * shadow, 0.05 * shadow),
      capTop: shade(color, 0.035 * highlight),
      capBottom: shade(color, -0.035 * shadow),
      crispMix,
      smoothMix,
      grad: null
    };
    if (paletteCache.size > 200) paletteCache.clear();
    paletteCache.set(key, p);
  }
  return p;
}
const hslNums = (c) => (c.startsWith("hsl(") ? c : shade(c, 0)).match(/[\d.]+/g).map(Number);
function mixCss(a, b, t) {
  const pa = hslNums(a);
  const pb = hslNums(b);
  const m = pa.map((v, i) => v + (pb[i] - v) * t);
  return `hsl(${m[0].toFixed(1)} ${m[1].toFixed(1)}% ${m[2].toFixed(1)}%)`;
}
const WHIRL_SEGMENTS = 34;
const WHIRL_SPAN = Math.PI * 1.55;
const WHIRL_RX = 57;
const WHIRL_RATIO = 0.4;
const WHIRL_TILT = -0.28;
const whirlInkCache = /* @__PURE__ */ new Map();
function whirlInk(color) {
  let w = whirlInkCache.get(color);
  if (!w) {
    w = { base: shade(color, 0.1, 0.02), light: shade(color, 0.3, 0.04), dark: shade(color, -0.22, 0.08), halo: shade(color, 0.2) };
    if (whirlInkCache.size > 200) whirlInkCache.clear();
    whirlInkCache.set(color, w);
  }
  return w;
}
const withAlpha = (hsl, a) => hsl.replace(")", ` / ${Math.max(0, Math.min(1, a)).toFixed(3)})`);
function drawWhirl(ctx, pose, color, lx, ly, near, knobs) {
  const strength = knobs?.strength ?? 0;
  const k = Math.min(1, pose.whirl * strength);
  if (k <= 0.01) return;
  const sizeK = knobs?.size ?? 1, widthK = knobs?.width ?? 1, lengthK = knobs?.length ?? 1, tiltK = knobs?.tilt ?? 1;
  const span = WHIRL_SPAN * lengthK;
  const ink = whirlInk(color);
  const head = -pose.whirlAngle;
  const rx = WHIRL_RX * sizeK;
  const ry = rx * WHIRL_RATIO * tiltK * (near ? 1.14 : 0.86);
  const lightA = Math.atan2(ly, lx) - WHIRL_TILT;
  ctx.save();
  ctx.rotate(WHIRL_TILT);
  ctx.translate(0, 5);
  ctx.lineCap = "butt";
  const seg = (a0, a1, width, style, dy) => {
    ctx.strokeStyle = style;
    ctx.lineWidth = width;
    ctx.beginPath();
    ctx.ellipse(0, dy, rx, ry, 0, a0, a1, false);
    ctx.stroke();
  };
  if (near) {
    for (let i = 0; i < WHIRL_SEGMENTS; i++) {
      const f = i / WHIRL_SEGMENTS;
      const a1 = head + f * span, a0 = a1 + span / WHIRL_SEGMENTS + 0.012;
      if (Math.sin((a0 + a1) / 2) <= 0) continue;
      const fade = Math.pow(1 - f, 1.3);
      seg(a1, a0, (2 + 8 * fade) * 1.5 * widthK, `rgba(0,0,0,${(0.2 * k * fade).toFixed(3)})`, 3.5);
    }
  }
  for (let i = 0; i < WHIRL_SEGMENTS; i++) {
    const f = i / WHIRL_SEGMENTS;
    const a1 = head + f * span, a0 = a1 + span / WHIRL_SEGMENTS + 0.012;
    const mid = (a0 + a1) / 2;
    if (Math.sin(mid) > 0 !== near) continue;
    const depth = 0.6 + 0.4 * Math.sin(mid);
    const fade = Math.pow(1 - f, 1.3);
    const puff = 1 + 0.18 * Math.sin(f * 9 + 1.2);
    const width = (2 + 8 * fade) * depth * widthK * puff;
    const a = k * (0.3 + 0.7 * fade) * depth;
    const facing = 0.5 + 0.5 * Math.cos(mid - lightA);
    seg(a1, a0, width * 2.6, withAlpha(ink.halo, a * 0.2), 0);
    seg(a1, a0, width * 0.8, withAlpha(ink.dark, a * 0.45), width * 0.32);
    seg(a1, a0, width, withAlpha(ink.base, a * 0.72), 0);
    seg(a1, a0, width * 0.62, withAlpha(ink.light, a * 0.78 * (0.4 + 0.6 * facing)), -width * 0.16);
    seg(a1, a0, width * 0.24, `rgba(255,255,255,${(a * 0.9 * (0.15 + 0.85 * facing * facing)).toFixed(3)})`, -width * 0.3);
  }
  ctx.restore();
}
const backOf = (deg) => {
  if (deg === void 0 || !Number.isFinite(deg)) return {};
  const a = deg * Math.PI / 180;
  return { bx: Math.sin(a), by: -Math.cos(a) };
};
const LIGHT_DEFAULTS = {
  fabric: { shadow: 1.15, highlight: 1.45, light: 295, rim: 0.6, spread: 1.6 },
  other: { shadow: 0.35, highlight: 1.3, light: 300, rim: 0.5, spread: 1.55 }
};
function draw(ctx, box, pose, cfg) {
  const full = box * OVERSCAN;
  ctx.clearRect(0, 0, full, full);
  const S = box / 100;
  let dpr, base;
  if (cfg.dpr !== void 0) {
    dpr = cfg.dpr;
    base = [dpr, 0, 0, dpr, 0, 0];
  } else if (ctx.getTransform) {
    const t = ctx.getTransform();
    base = [t.a, t.b, t.c, t.d, t.e, t.f];
    dpr = t.a || 1;
  } else {
    dpr = 1;
    base = [1, 0, 0, 1, 0, 0];
  }
  const lit = LIGHT_DEFAULTS[cfg.shading === "fabric" ? "fabric" : "other"];
  const shadow = cfg.shadow ?? lit.shadow, highlight = cfg.highlight ?? lit.highlight;
  const halfDepth = HALF_DEPTH * (cfg.depth ?? 0.65);
  const rimK = cfg.rim ?? lit.rim;
  const cap = 1 - (1 - CAP) * rimK;
  const spread = cfg.spread ?? lit.spread;
  const la = (cfg.light ?? lit.light) * Math.PI / 180;
  const lx = Math.sin(la), ly = -Math.cos(la);
  const bodyPal = palette(cfg.color, shadow, highlight);
  const cy0 = Math.cos(pose.yaw), sy = Math.sin(pose.yaw);
  const cp0 = Math.cos(pose.pitch), sp = Math.sin(pose.pitch);
  const facing = cy0 * cp0;
  const floor = (v) => Math.abs(v) < 0.22 ? v < 0 ? -0.22 : 0.22 : v;
  const cy = floor(cy0), cp = floor(cp0);
  const cr = Math.cos(pose.roll), sr = Math.sin(pose.roll), kx = pose.sx * S, ky = pose.sy * S;
  const lift = 50 * (1 - pose.sy) * S;
  const body = mulAffine(base, [cr * kx, sr * kx, -sr * ky, cr * ky, full / 2 + pose.x * S - sr * lift, full / 2 + RISE * box + pose.y * S + cr * lift]);
  ctx.save();
  ctx.setTransform(body[0], body[1], body[2], body[3], body[4], body[5]);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const mode = cfg.shading;
  const drawSolid = (path, key2, halfDepth2, plush = true, pal = bodyPal) => {
    let lit2 = pal.near;
    let capFill = pal.base;
    if (mode === "crisp") {
      if (!pal.grad || pal.grad.lx !== lx || pal.grad.ly !== ly) {
        const g = ctx.createLinearGradient(lx * 56, ly * 56, -lx * 56, -ly * 56);
        g.addColorStop(0, pal.light);
        g.addColorStop(0.45, pal.near);
        g.addColorStop(1, pal.dark);
        const c = ctx.createLinearGradient(lx * 46, ly * 46, -lx * 46, -ly * 46);
        c.addColorStop(0, pal.capTop);
        c.addColorStop(1, pal.capBottom);
        pal.grad = { lx, ly, lit: g, cap: c };
      }
      lit2 = pal.grad.lit;
      capFill = pal.grad.cap;
    }
    let plasticDone2 = false;
    if (mode === "plastic" || mode === "fabric") {
      plasticDone2 = drawPlasticCap(
        ctx,
        { ...cfg, path, typeKey: key2, shading: plush ? mode : "plastic" },
        { cy, sy, cp, sp, facing, roll: pose.roll, halfDepth: halfDepth2, lx, ly, ...backOf(cfg.backLight), dev: box * dpr, ctm: body, still: cfg.still, round: cfg.roundness ?? 1 },
        pal,
        null,
        { shadow, highlight, spread, rim: rimK, vivid: cfg.vivid ?? 0, front: Math.min(85, Math.max(0, cfg.lightFront ?? 32)) * Math.PI / 180, shine: cfg.shine ?? 0, sheen: cfg.sheen ?? 0, backSoft: cfg.backSoftness ?? 1 }
      );
    }
    const mode2 = (mode === "plastic" || mode === "fabric") && !plasticDone2 ? "smooth" : mode;
    const soft2 = mode2 === "smooth";
    const union = soft2 && typeof Path2D === "function" ? new Path2D() : null;
    const order = facing >= 0 ? 1 : -1;
    const [ca, cb, cc, cd, ce, cf] = body;
    let fill = null;
    let pa = 1, pb = 0, pc = 0, pd = 1, pe = 0, pf = 0;
    for (let j = 0; j < SLICES && !plasticDone2; j++) {
      const k = order > 0 ? j : SLICES - 1 - j;
      const z = -1 + 2 * k / (SLICES - 1);
      const s = profile(z, cap);
      const near = j / (SLICES - 1);
      const m0 = cy * s, m1 = sy * sp * s, m3 = cp * s;
      const e = z * sy * halfDepth2 - 50 * m0, fo = -z * cy * sp * halfDepth2 - 50 * m1 - 50 * m3;
      const det = pa * pd - pb * pc;
      const ia = pd / det, ib = -pb / det, ic = -pc / det, id = pa / det, ie = (pc * pf - pd * pe) / det, jf = (pb * pe - pa * pf) / det;
      ctx.transform(ia * m0 + ic * m1, ib * m0 + id * m1, ic * m3, id * m3, ia * e + ic * fo + ie, ib * e + id * fo + jf);
      pa = m0;
      pb = m1;
      pc = 0;
      pd = m3;
      pe = e;
      pf = fo;
      let style;
      if (soft2) style = pal.smoothMix[j];
      else if (j === SLICES - 1) style = capFill;
      else if (near > 0.6) style = lit2;
      else style = pal.crispMix[j];
      if (style !== fill) ctx.fillStyle = fill = style;
      ctx.fill(path);
      if (union) union.addPath(path, { a: m0, b: m1, c: 0, d: m3, e, f: fo });
    }
    if (!plasticDone2) ctx.setTransform(ca, cb, cc, cd, ce, cf);
    if (union && mode2 === "smooth") {
      ctx.save();
      ctx.clip(union);
      const sa = Math.min(1, 0.34 * shadow);
      const sg = ctx.createRadialGradient(-lx * 45, -ly * 45, 4 * spread, -lx * 45, -ly * 45, 84 * spread);
      sg.addColorStop(0, `rgba(0,0,0,${sa})`);
      sg.addColorStop(0.5, `rgba(0,0,0,${sa * 0.35})`);
      sg.addColorStop(1, "rgba(0,0,0,0)");
      ctx.globalCompositeOperation = "multiply";
      ctx.fillStyle = sg;
      ctx.fillRect(-120, -120, 240, 240);
      ctx.globalCompositeOperation = "source-over";
      const ha = Math.min(1, 0.22 * highlight);
      const hg = ctx.createRadialGradient(lx * 37, ly * 37, 0, lx * 37, ly * 37, 62 * spread);
      hg.addColorStop(0, `rgba(255,255,255,${ha})`);
      hg.addColorStop(0.6, `rgba(255,255,255,${ha * 0.23})`);
      hg.addColorStop(1, "rgba(255,255,255,0)");
      ctx.fillStyle = hg;
      ctx.fillRect(-120, -120, 240, 240);
      ctx.restore();
    }
    return plasticDone2;
  };
  drawWhirl(ctx, pose, cfg.color, lx, ly, false, cfg.whirl);
  const wear = cfg.wear;
  const worn = !!wear && (wear.hat !== "none" || wear.glasses !== "none" || wear.headphones || wear.bowTie);
  const wr = { cy, sy, cp, sp, facing, lx, ly };
  if (worn) drawWearBehind(ctx, cfg.path, wr, wear);
  const key = cfg.typeKey ?? "custom";
  const parts = cfg.parts ? Array.isArray(cfg.parts) ? cfg.parts : [cfg.parts] : [];
  const partPals = /* @__PURE__ */ new Map();
  const partPal = (i) => {
    const color = cfg.partsColors?.[i];
    if (!color) return bodyPal;
    let p = partPals.get(color);
    if (!p) {
      p = palette(color, shadow, highlight);
      partPals.set(color, p);
    }
    return p;
  };
  for (let i = 0; i < parts.length; i++) drawSolid(parts[i], Array.isArray(cfg.parts) ? `${key}:parts${i}/${parts.length}` : `${key}:parts`, halfDepth * (cfg.partsDepth ?? 0.4), false, partPal(i));
  const plasticDone = drawSolid(cfg.path, key, halfDepth);
  const zf = facing >= 0 ? 1 : -1;
  const round = cfg.roundness ?? 1;
  const cushion = plasticDone;
  const sf = cushion ? 1 : profile(zf, cap);
  const surface2 = (x, y) => cushion ? surfaceAt(key, halfDepth, x, y, round) ?? 0.8 * halfDepth : halfDepth * sf;
  const zFace = cushion ? 0.55 * surface2(cfg.faceX, cfg.faceY) : halfDepth;
  const fm0 = cy * sf, fm1 = sy * sp * sf, fm3 = cp * sf;
  const fe = zf * sy * zFace - 50 * fm0;
  const ffo = -zf * cy * sp * zFace - 50 * fm1 - 50 * fm3;
  let frontClip = null;
  if (worn && typeof Path2D === "function") {
    frontClip = new Path2D();
    frontClip.addPath(cfg.path, { a: fm0, b: fm1, c: 0, d: fm3, e: fe, f: ffo });
  }
  if (facing > -0.2) {
    ctx.save();
    {
      const [ca, cb, cc, cd, ce, cf] = body;
      ctx.setTransform(ca * fm0 + cc * fm1, cb * fm0 + cd * fm1, cc * fm3, cd * fm3, ca * fe + cc * ffo + ce, cb * fe + cd * ffo + cf);
      ctx.clip(cfg.path);
      ctx.setTransform(ca, cb, cc, cd, ce, cf);
    }
    ctx.translate(cfg.faceX - 50, cfg.faceY - 50);
    ctx.scale(cfg.faceScale, cfg.faceScale);
    if (mode === "plastic" && plasticDone) ctx.globalAlpha = 0.93;
    const eyes = drawFace(ctx, pose, cfg);
    ctx.globalAlpha = 1;
    const glasses = wear && wear.glasses !== "none" ? wear.glasses : null;
    const turn = { yaw: pose.yaw, pitch: pose.pitch };
    if (glasses) drawGlasses(ctx, glasses, eyes, { lx, ly }, "shadow", turn);
    ctx.restore();
    if (glasses) {
      ctx.save();
      ctx.translate(cfg.faceX - 50, cfg.faceY - 50);
      ctx.scale(cfg.faceScale, cfg.faceScale);
      drawGlasses(ctx, glasses, eyes, { lx, ly }, "frame", turn);
      ctx.restore();
    }
  }
  if (worn) drawWearFront(ctx, cfg.path, wr, wear, frontClip, surface2, { y: cfg.faceY, scale: cfg.faceScale });
  drawWhirl(ctx, pose, cfg.color, lx, ly, true, cfg.whirl);
  ctx.restore();
}
const FACE_R = 30;
function onSphere(x, y, yaw, pitch) {
  const lon = Math.asin(Math.max(-1, Math.min(1, x / FACE_R))) + yaw;
  const lat = Math.asin(Math.max(-1, Math.min(1, -y / FACE_R))) + pitch;
  const cl = Math.cos(lat);
  return {
    x: FACE_R * Math.sin(lon) * cl,
    y: -FACE_R * Math.sin(lat),
    sx: Math.cos(lon),
    sy: cl,
    z: Math.cos(lon) * cl
  };
}
const EYE_STEPS = 8;
const eyePaths = /* @__PURE__ */ new Map();
function eyePath(x0, y0, cy) {
  const qx = Math.round(x0 * 50), qy = Math.round(y0 * 50), qc = Math.round(cy * 50);
  const key = qx + 2e3 * qy + 4e6 * qc;
  let p = eyePaths.get(key);
  if (!p) {
    const ax = qx / 50, ay = qy / 50, ac = qc / 50;
    let d = `M${-ax} ${ay}`;
    for (let i = 1; i <= EYE_STEPS; i++) {
      const t = i / EYE_STEPS, mt = 1 - t;
      d += ` L${(mt * mt * -ax + t * t * ax).toFixed(3)} ${((mt * mt + t * t) * ay + 2 * mt * t * ac).toFixed(3)}`;
    }
    p = new Path2D(d);
    if (eyePaths.size > 256) eyePaths.clear();
    eyePaths.set(key, p);
  }
  return p;
}
const MOUTH_SIN = Math.sin(0.684), MOUTH_COS = Math.cos(0.684);
const mouthPaths = /* @__PURE__ */ new Map();
function mouthPath(m) {
  const q = (v) => Math.round(v * 50) / 50;
  const hw = q(m.hw), t0 = q(m.t0), a = q(m.a), yt = q(m.yt), ab = q(m.ab), yb = q(m.yb);
  const key = `${hw},${t0},${a},${yt},${ab},${yb}`;
  let p = mouthPaths.get(key);
  if (p) return p;
  const f = (v) => v.toFixed(3);
  const nx = t0 * MOUTH_SIN, ny = t0 * MOUTH_COS;
  const ltx = -hw + nx, lty = -ny, rtx = hw - nx, rty = -ny;
  const lbx = -hw - nx, lby = ny, rbx = hw + nx, rby = ny;
  const cx = 4 / 3 * t0 * MOUTH_COS, cy = 4 / 3 * t0 * MOUTH_SIN;
  const d = `M${f(ltx)} ${f(lty)}C${f(-hw + a * hw)} ${f(yt - t0)} ${f(hw - a * hw)} ${f(yt - t0)} ${f(rtx)} ${f(rty)}C${f(rtx + cx)} ${f(rty - cy)} ${f(rbx + cx)} ${f(rby - cy)} ${f(rbx)} ${f(rby)}C${f(hw - ab * hw)} ${f(yb + t0)} ${f(-hw + ab * hw)} ${f(yb + t0)} ${f(lbx)} ${f(lby)}C${f(lbx - cx)} ${f(lby - cy)} ${f(ltx - cx)} ${f(lty - cy)} ${f(ltx)} ${f(lty)}Z`;
  p = new Path2D(d);
  if (mouthPaths.size > 256) mouthPaths.clear();
  mouthPaths.set(key, p);
  return p;
}
function drawFace(ctx, pose, cfg) {
  const [wd, ww, ws] = pose.w;
  const ink = cfg.ink;
  const ey = EYE_Y[cfg.face];
  const half = EYE_GAP / 2;
  const lx = pose.lookX, ly = pose.lookY;
  const { yaw, pitch } = pose;
  const at = (x, y, fn, alpha = 1) => {
    const q = onSphere(x, y, yaw, pitch);
    if (q.z <= 0.02 || alpha <= 0.01) return;
    ctx.save();
    ctx.globalAlpha = alpha * Math.min(1, q.z * 5);
    ctx.translate(q.x, q.y);
    ctx.scale(Math.max(0.02, q.sx), Math.max(0.02, q.sy));
    fn();
    ctx.restore();
  };
  const past = (v, d) => Math.abs(v) <= d ? 0 : Math.sign(v) * (Math.abs(v) - d) / (1 - d);
  const clamp1 = (v) => Math.max(-1, Math.min(1, v));
  const up = past(clamp1(-pose.pitch / 0.26 - pose.lookY / 7), 0.34);
  const side = Math.abs(past(clamp1(pose.lookX / 4.5), 0.4));
  const tall = Math.max(0.3, 1 + 0.55 * up - 0.1 * side);
  const wide = 1 - 0.05 * up + 0.12 * side;
  const open = wd + ww * (1 - pose.laugh);
  const laugh = ww * pose.laugh;
  const lift = Math.max(0, -pose.y) / 26;
  const sag = 0.5 + 0.5 * pose.breath;
  for (const side2 of [-1, 1]) {
    const lid = side2 < 0 ? pose.blinkL : pose.blinkR;
    const e = Math.max(0, Math.min(1, pose.eyeOpen * (1 - lid)));
    const kOpen = open * e, kShut = open * (1 - e), kLaugh = laugh, kSleep = ws;
    const x0 = kOpen * 0.01 + kShut * 5.4 + kLaugh * 6.2 + kSleep * 6;
    const y0 = kOpen * 1.1 * tall + kShut * 0.6 + kLaugh * (2.2 - lift * 1.5) + kSleep * (-1.4 + sag);
    const cy = kOpen * -3.3 * tall + kShut * 0.6 + kLaugh * (-11.4 - 4 * lift) + kSleep * (5.4 + 2 * sag);
    const w = kOpen * EYE_RX * 2 * wide + kShut * 2.8 + kLaugh * 4.4 + kSleep * 4;
    const dx = lx * (kOpen + 0.5 * (kShut + kLaugh)), dy = ly * (kOpen + 0.5 * kShut);
    at(side2 * half + dx, ey + dy, () => {
      ctx.strokeStyle = ink;
      ctx.lineWidth = w;
      ctx.stroke(eyePath(x0, y0, cy));
    });
  }
  if (cfg.face === "mouth") {
    const mx = lx * 0.35;
    const kd = (0.6 + 0.4 * wd) * (1 + 0.06 * pose.breath);
    const kw = (0.6 + 0.4 * ww) * (1 + 0.25 * Math.max(0, -pose.y) / 26);
    const r = 2.7 * ws * (1 + 0.25 * pose.breath);
    const b = (d, w, s) => wd * d + ww * w + ws * s;
    const m = {
      hw: b(6.5 * kd, 9.5 * kw, r),
      t0: b(1.9, 0, 0),
      a: b(2 / 3, 2 / 3, 0),
      yt: b(3.53 * kd, 1.6 * kw, -4 * r / 3),
      ab: b(2 / 3, 0, 0),
      yb: b(3.53 * kd, 17.3 * kw, 4 * r / 3)
    };
    at(mx, b(12.5, 11.6, 15.5), () => {
      ctx.fillStyle = ink;
      ctx.fill(mouthPath(m));
    });
  }
  const spot = (x) => {
    const q = onSphere(x, ey, yaw, pitch);
    return { x: q.x, y: q.y, sx: q.sx, sy: q.sy, z: q.z, ox: x, oy: ey, fz: Math.sqrt(Math.max(0, FACE_R * FACE_R - x * x - ey * ey)) };
  };
  return [spot(-half), spot(half)];
}
const STATES = ["default", "working", "sleeping"];
const HOP_T = 0.68;
const HOP_SPIN_H = 26;
const FLIP_PRE = 0.2;
const JUMP_DEFAULTS = { height: HOP_SPIN_H, time: HOP_T, stretch: 1, squash: 1.15, squashTime: 0.37, squashEase: "pulse", groundTime: 0.11, groundEase: "pulse", riseTime: 0.33, riseEase: "pulse", clickSquashTime: 0.24, spin: 1, lean: 6, every: 8, land: 0 };
const flipPre = (j, poked) => poked ? j.clickSquashTime : FLIP_PRE * j.time;
const flipDuration = (j, poked) => flipPre(j, poked) + j.time + SQUASH_PEAK[j.squashEase] * (poked ? j.clickSquashTime : j.squashTime) + Math.max(0, j.groundTime) + j.riseTime + Math.max(0, j.land) + 0.05;
const SQUASH_PEAK = { sharp: 0, pulse: 2 / 7, soft: 0.5, bouncy: 0.144 };
const risePulse = (v, ease) => {
  if (v <= 0) return 1;
  if (v >= 1) return 0;
  switch (ease) {
    case "sharp":
      return (1 - v) * (1 - v);
    case "soft":
      return 0.5 + 0.5 * Math.cos(Math.PI * v);
    case "bouncy":
      return Math.exp(-3.2 * v) * Math.cos(5.4 * v) - v * v * v * 0.026;
    default: {
      const k = 4.2 * v;
      return (1 + k) * Math.exp(-k) - v * v * v * 0.078;
    }
  }
};
const groundShape = (v, ease) => 1 + 0.25 * squashPulse(v, ease);
const squashPulse = (u, ease) => {
  if (u <= 0 || u >= 1) return 0;
  let x;
  switch (ease) {
    case "sharp":
      x = (1 - u) * (1 - u);
      break;
    case "soft":
      x = Math.sin(Math.PI * u) ** 2;
      break;
    case "bouncy":
      x = Math.exp(-3.15 * u) * Math.sin(8.43 * u) / 0.596;
      break;
    default: {
      const k = 7 * u;
      x = k * k * Math.exp(2 - k) / 4;
    }
  }
  const tail = u > 0.85 ? 1 - (u - 0.85) / 0.15 : 1;
  return x * tail * tail * (3 - 2 * tail);
};
const hopSquash = (a) => Math.exp(-Math.pow(Math.min(Math.abs(a), Math.abs(a - 1)) / 0.11, 2));
const DEG = Math.PI / 180;
const TAU = Math.PI * 2;
const SWITCH_TO = { default: 1.2, working: 0.7, sleeping: 1.4 };
const SWITCH_FROM_SLEEP = 1;
function rng(seed) {
  let a = seed * 2654435761 >>> 0 || 1;
  return () => {
    a = a + 1831565813 >>> 0;
    let t = a;
    t = Math.imul(t ^ t >>> 15, t | 1);
    t ^= t + Math.imul(t ^ t >>> 7, t | 61);
    return ((t ^ t >>> 14) >>> 0) / 4294967296;
  };
}
function approach(cur, target, rate, dt) {
  return cur + (target - cur) * (1 - Math.exp(-rate * dt));
}
const easeInOut = (p) => p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2;
const easeSine = (p) => 0.5 - 0.5 * Math.cos(Math.PI * p);
class Wander {
  /* A channel that picks a new target now and then and moves to it. The
     head channels move as a lightly damped spring — the turn starts and
     ends softly, the way a head does — while the eyes dart with an
     exponential approach, the way eyes do. */
  constructor(rand, amp, holdMin, holdMax, rate, spring = false) {
    this.rand = rand;
    this.amp = amp;
    this.holdMin = holdMin;
    this.holdMax = holdMax;
    this.rate = rate;
    this.spring = spring;
  }
  rand;
  amp;
  holdMin;
  holdMax;
  rate;
  spring;
  value = 0;
  vel = 0;
  target = 0;
  next = 0;
  update(t, dt) {
    if (t >= this.next) {
      this.target = (this.rand() * 2 - 1) * this.amp;
      this.next = t + this.holdMin + this.rand() * (this.holdMax - this.holdMin);
    }
    if (this.spring) {
      const w = this.rate * 1.6, z = 0.9;
      this.vel += (w * w * (this.target - this.value) - 2 * z * w * this.vel) * dt;
      this.value += this.vel * dt;
    } else this.value = approach(this.value, this.target, this.rate, dt);
  }
  /** aim at a value and stay there: the rig picks the next one */
  aim(v) {
    this.target = v;
    this.next = Infinity;
  }
  set(amp, holdMin, holdMax, rate) {
    this.amp = amp;
    this.holdMin = holdMin;
    this.holdMax = holdMax;
    this.rate = rate;
    this.next = 0;
  }
}
class Event {
  constructor(duration) {
    this.duration = duration;
  }
  duration;
  p = -1;
  fire() {
    this.p = 0;
  }
  get active() {
    return this.p >= 0;
  }
  update(dt) {
    if (this.p < 0) return;
    this.p += dt / this.duration;
    if (this.p >= 1) this.p = -1;
  }
}
const GAZE_YAW = 35 * DEG;
const GAZE_PITCH = 14 * DEG;
const GAZE_ROLL = 3.2 * DEG;
const GAZE_HOLD_MIN = 2.6;
const GAZE_HOLD_MAX = 4.4;
const REST = {
  default: { pitch: 0, roll: 0, y: 0, lookX: 0, lookY: 0 },
  working: { pitch: 5 * DEG, roll: 0, y: 0, lookX: 0, lookY: 0 },
  sleeping: { pitch: -16 * DEG, roll: 6 * DEG, y: 3, lookX: 0, lookY: 1 }
};
class Sim {
  pose = { yaw: 0, pitch: 0, roll: 0, x: 0, y: 0, sx: 1, sy: 1, eyeOpen: 1, blinkL: 0, blinkR: 0, lookX: 0, lookY: 0, breath: 0, laugh: 0, whirl: 0, whirlAngle: 0, w: [1, 0, 0] };
  state = "default";
  rand;
  t = 0;
  /* a state change: the weights it started from and its progress */
  wFrom = [1, 0, 0];
  tr = 1;
  trDuration = 1.2;
  yawW;
  pitchW;
  rollW;
  lookXW;
  lookYW;
  blink = new Event(0.17);
  blinkAt;
  blinkAgain = false;
  /* −1 left eye only, 1 right eye only, 0 both */
  dart = new Event(0.12);
  dartAt;
  dartX = 0;
  dartY = 0;
  flip = new Event(flipDuration(JUMP_DEFAULTS, false));
  flipPoked = false;
  jump = { ...JUMP_DEFAULTS };
  flipAt;
  flipSide = 1;
  nod = new Event(1.7);
  nodAt;
  hopPhase = 0;
  hopCount = 0;
  /** the hops' gain: the working weight while in the state, then held so
      a hop under way finishes whole when the state is left */
  hopGain = 0;
  laughEv = new Event(0.8);
  laughAt;
  /* the jelly: a damped spring driven by how fast the head turns, so a
     sweep stretches the body and it wobbles back */
  prevYaw = 0;
  jelly = 0;
  jellyV = 0;
  /** how far the eyes run ahead of a head turn */
  gazeLead = 0;
  /** the idle gaze: where the head is looking, and when it moves on */
  gazeDir = [0, 0];
  gazeAt = 0;
  /** how far it turns to the side, 1 as the gaze has it */
  turnK = 1;
  /** the breathing cycle's phase, in turns */
  breathPhase = 0;
  /* the pointer, as an offset from the head in head-widths, and how much
     to follow it — both smoothed */
  ptrX = 0;
  ptrY = 0;
  ptrS = 0;
  ptrTargetX = 0;
  ptrTargetY = 0;
  ptrTargetS = 0;
  /* the smoothed yaw the head is turning to on its own */
  baseYaw = 0;
  constructor(seed, state2 = "default") {
    this.rand = rng(Math.floor(seed * 1e6) + 1);
    const r = this.rand;
    this.yawW = new Wander(r, 36 * DEG, 1.1, 2.6, 3, true);
    this.pitchW = new Wander(r, 10 * DEG, 1.1, 2.6, 2.6, true);
    this.rollW = new Wander(r, 5 * DEG, 1.6, 3.2, 2, true);
    this.lookXW = new Wander(r, 3.6, 0.5, 2, 14);
    this.lookYW = new Wander(r, 2.4, 0.5, 2, 14);
    this.t = r() * 10;
    this.hopPhase = r();
    this.breathPhase = r();
    this.blinkAt = this.t + 1 + r() * 3;
    this.flipAt = this.nextFlip(this.t, 1);
    this.nodAt = this.t + 3 + r() * 4;
    this.dartAt = this.t + 1 + r() * 2;
    this.laughAt = this.t + 0.6 + r() * 1.5;
    this.setState(state2, true);
  }
  setState(next, immediate = false) {
    if (next === this.state && !immediate) return;
    const from = this.state;
    this.state = next;
    const w = this.pose.w;
    if (immediate) {
      for (let i = 0; i < 3; i++) w[i] = STATES[i] === next ? 1 : 0;
      this.tr = 1;
    } else {
      this.wFrom = [w[0], w[1], w[2]];
      this.tr = 0;
      this.trDuration = from === "sleeping" ? SWITCH_FROM_SLEEP : SWITCH_TO[next];
    }
    switch (next) {
      case "default":
        this.yawW.set(GAZE_YAW, 2.6, 5.4, 2);
        this.pitchW.set(GAZE_PITCH, 2.8, 5.8, 1.8);
        this.rollW.set(GAZE_ROLL, 3.4, 6.6, 1.5);
        this.gazeAt = 0;
        this.gazeDir = [0, 0];
        this.lookXW.set(3.6, 0.6, 2.2, 13);
        this.lookYW.set(2.4, 0.6, 2.2, 13);
        this.flipAt = this.nextFlip(this.t, 0.6);
        break;
      case "working":
        this.yawW.set(16 * DEG, 0.9, 1.8, 4);
        this.pitchW.set(3 * DEG, 1.2, 2.4, 3);
        this.rollW.set(0, 1, 2, 3);
        this.lookXW.set(2, 0.5, 1.2, 12);
        this.lookYW.set(1, 0.5, 1.2, 12);
        this.hopPhase = 0;
        this.hopCount = 0;
        this.laughAt = this.t + 0.5 + this.rand() * 1.2;
        break;
      case "sleeping":
        this.yawW.set(7 * DEG, 3, 6, 0.7);
        this.pitchW.set(3 * DEG, 3, 6, 0.7);
        this.rollW.set(2 * DEG, 3, 6, 0.6);
        this.lookXW.set(0, 2, 4, 2);
        this.lookYW.set(0, 2, 4, 2);
        this.nodAt = this.t + 2.5 + this.rand() * 4;
        break;
    }
  }
  /** Where the pointer is, relative to the head (−1 … 1 across a head
      width), and how strongly to follow it (0 lets go). */
  setPointer(x, y, strength) {
    this.ptrTargetX = Math.max(-1.2, Math.min(1.2, x));
    this.ptrTargetY = Math.max(-1.2, Math.min(1.2, y));
    this.ptrTargetS = Math.max(0, Math.min(1, strength));
  }
  /** A hop and a full turn, right now, whatever the state. */
  poke() {
    if (this.flip.active && this.flip.p < 0.6) return;
    this.flipPoked = true;
    this.flip.duration = flipDuration(this.jump, true);
    this.flipSide = this.rand() < 0.5 ? -1 : 1;
    this.flip.fire();
    this.flipAt = this.nextFlip(this.t, 1.1);
  }
  /** How far the head turns to the side while idle: 1 as the gaze has
      it, 0 keeps it facing forward. */
  setTurn(k) {
    const next = Math.max(0, k);
    if (next === this.turnK) return;
    this.turnK = next;
    if (this.state === "default") this.gazeAt = 0;
  }
  /** The jump's numbers; any subset. */
  setJump(j) {
    const every = this.jump.every;
    Object.assign(this.jump, j);
    if (j.every !== void 0 && j.every !== every) this.flipAt = this.nextFlip(this.t, 1);
  }
  /* Where the head looks next. From a corner it mostly swings straight
     across to the opposite one — top right, stay, bottom left — now and
     then only sideways, or back to the middle for a beat. */
  nextGaze() {
    const r = this.rand;
    const [px, py] = this.gazeDir;
    if (px !== 0 || py !== 0) {
      const p = r();
      if (p < 0.66) return [-px, -py];
      if (p < 0.85) return [-px, py];
      return [0, 0];
    }
    const corners = [
      [1, -1],
      [-1, 1],
      [-1, -1],
      [1, 1]
    ];
    return corners[Math.floor(r() * corners.length)];
  }
  /** when the next idle jump is due: `every` seconds, give or take 40 % */
  nextFlip(t, k) {
    const every = this.jump.every;
    return every > 0 ? t + every * k * (0.625 + this.rand() * 0.75) : Infinity;
  }
  /** Advance by `dt` seconds (already scaled by the speed). */
  update(dt) {
    dt = Math.min(dt, 0.05);
    this.t += dt;
    const t = this.t;
    const p = this.pose;
    const w = p.w;
    if (this.tr < 1) {
      this.tr = Math.min(1, this.tr + dt / this.trDuration);
      const e = easeSine(this.tr);
      for (let i = 0; i < 3; i++) {
        const target = STATES[i] === this.state ? 1 : 0;
        w[i] = this.wFrom[i] + (target - this.wFrom[i]) * e;
      }
    }
    const [wd, ww, ws] = w;
    const rest = { pitch: 0, roll: 0, y: 0, lookX: 0, lookY: 0 };
    for (let i = 0; i < 3; i++) {
      const r = REST[STATES[i]];
      rest.pitch += r.pitch * w[i];
      rest.roll += r.roll * w[i];
      rest.y += r.y * w[i];
      rest.lookX += r.lookX * w[i];
      rest.lookY += r.lookY * w[i];
    }
    if (this.state === "default" && t >= this.gazeAt) {
      const [gx, gy] = this.nextGaze();
      this.gazeDir = [gx, gy];
      const reach2 = 0.84 + this.rand() * 0.16;
      this.yawW.aim(gx * GAZE_YAW * reach2 * this.turnK);
      this.pitchW.aim(gy * GAZE_PITCH * reach2);
      this.rollW.aim(gx * GAZE_ROLL * reach2 * this.turnK);
      this.gazeAt = t + GAZE_HOLD_MIN + this.rand() * (GAZE_HOLD_MAX - GAZE_HOLD_MIN);
    }
    this.yawW.update(t, dt);
    this.pitchW.update(t, dt);
    this.rollW.update(t, dt);
    this.lookXW.update(t, dt);
    this.lookYW.update(t, dt);
    this.ptrS = approach(this.ptrS, this.ptrTargetS, 8, dt);
    this.ptrX = approach(this.ptrX, this.ptrTargetX, 14, dt);
    this.ptrY = approach(this.ptrY, this.ptrTargetY, 14, dt);
    const ps = this.ptrS;
    const quiet = 1 - 0.75 * ps;
    this.baseYaw = approach(this.baseYaw, this.yawW.value * quiet + 22 * DEG * this.ptrX * ps, 5, dt);
    const basePitch = rest.pitch + this.pitchW.value * quiet - 12 * DEG * this.ptrY * ps;
    const baseRoll = rest.roll + this.rollW.value * quiet;
    const baseY = rest.y;
    const baseLookX = rest.lookX + this.lookXW.value * quiet + 4.5 * this.ptrX * ps;
    const baseLookY = rest.lookY + this.lookYW.value * quiet + 3 * this.ptrY * ps;
    let spin = 0, hopY = 0, sx = 1, sy = 1, pitchAdd = 0, rollAdd = 0, blinkClose = 0, lookXAdd = 0, lookYAdd = 0, laugh = 0;
    let whirl = 0, whirlAngle = 0;
    const smooth2 = (a, b, v) => {
      const x = Math.min(1, Math.max(0, (v - a) / (b - a)));
      return x * x * (3 - 2 * x);
    };
    const envelope = (q) => smooth2(0.1, 0.26, q) * (1 - smooth2(0.66, 0.9, q));
    const ringAngle = (q) => TAU * (1.5 * q + 0.9 * easeInOut(q));
    if (t >= this.blinkAt && !this.blink.active && wd + ww > 0.5) {
      this.blink.fire();
      this.blinkAgain = !this.blinkAgain && this.rand() < 0.22;
      this.blinkAt = t + (this.blinkAgain ? 0.28 : 2.2 + this.rand() * 2.6);
    }
    this.blink.update(dt);
    if (this.blink.active) blinkClose = Math.sin(Math.PI * this.blink.p);
    if (t >= this.dartAt && !this.dart.active && wd + ww > 0.5) {
      this.dart.fire();
      this.dartX = (this.rand() * 2 - 1) * 4;
      this.dartY = (this.rand() * 2 - 1) * 2;
      this.dart.duration = 0.25 + this.rand() * 0.45;
      this.dartAt = t + 1.2 + this.rand() * 2.6;
    }
    this.dart.update(dt);
    if (this.dart.active) {
      const q = this.dart.p;
      const hold = q < 0.15 ? q / 0.15 : q > 0.8 ? (1 - q) / 0.2 : 1;
      lookXAdd += this.dartX * hold * (wd + ww);
      lookYAdd += this.dartY * hold * (wd + ww);
    }
    if (this.state === "default" && t >= this.flipAt && !this.flip.active) {
      this.flipPoked = false;
      this.flip.duration = flipDuration(this.jump, false);
      this.flipSide = this.rand() < 0.5 ? -1 : 1;
      this.flip.fire();
      this.flipAt = this.nextFlip(t, 1);
    }
    this.flip.update(dt);
    if (this.flip.active) {
      const J2 = this.jump;
      const preS = flipPre(J2, this.flipPoked);
      const a = (this.flip.p * this.flip.duration - preS) / J2.time;
      const q = Math.min(1, Math.max(0, a));
      const arc = Math.sin(Math.PI * q);
      spin += TAU * J2.spin * easeInOut(q);
      hopY -= J2.height * arc;
      const tl = (a - 1) * J2.time - J2.land;
      const squashTime = this.flipPoked ? J2.clickSquashTime : J2.squashTime;
      const crouch = (u) => this.flipPoked ? u * u * (3 - 2 * u) : hopSquash(u - 1);
      const hold = Math.max(0, J2.groundTime);
      const peakT = SQUASH_PEAK[J2.squashEase] * squashTime;
      const rise = tl - peakT - hold;
      const depth = tl <= peakT ? squashPulse(tl / squashTime, J2.squashEase) : rise <= 0 ? groundShape((tl - peakT) / hold, J2.groundEase) : risePulse(rise / J2.riseTime, J2.riseEase);
      const land2 = (a < 0 ? crouch(Math.max(0, 1 + a * J2.time / preS)) : tl > 0 ? depth : a < 0.2 ? hopSquash(a) : 0) * J2.squash;
      sx += 0.16 * land2 - 0.06 * arc * J2.stretch;
      sy += -0.18 * land2 + 0.09 * arc * J2.stretch;
      rollAdd += this.flipSide * J2.lean * DEG * arc;
      if (J2.spin > 0) {
        laugh = Math.max(laugh, arc);
        whirl = Math.max(whirl, envelope(q));
        whirlAngle = ringAngle(q);
      }
    }
    const J = this.jump;
    const exitHold = Math.max(0, J.groundTime);
    const exitFor = exitHold + J.riseTime;
    if (this.state === "working") this.hopGain = ww;
    if (this.hopGain > 0.02 && (this.state === "working" || this.hopPhase > 0)) {
      this.hopPhase += dt / HOP_T;
      if (this.hopPhase >= 1) {
        if (this.state === "working") {
          this.hopPhase -= 1;
          this.hopCount += 1;
        } else if ((this.hopPhase - 1) * HOP_T >= exitFor) {
          this.hopPhase = 0;
          this.hopGain = 0;
        }
      }
      const g = this.hopGain;
      const q = Math.min(1, this.hopPhase);
      const arc = Math.sin(Math.PI * q);
      const spinning = this.hopCount % 3 === 2;
      const h = spinning ? HOP_SPIN_H : 18;
      hopY -= h * arc * g;
      const exitT = this.state !== "working" && this.hopPhase > 1 ? (this.hopPhase - 1) * HOP_T : -1;
      const land2 = exitT < 0 ? hopSquash(this.hopPhase) : exitT < exitHold ? groundShape(exitT / exitHold, J.groundEase) : risePulse((exitT - exitHold) / J.riseTime, J.riseEase);
      sx += (0.16 * land2 - 0.06 * arc) * g;
      sy += (-0.18 * land2 + 0.09 * arc) * g;
      if (spinning) {
        spin += TAU * easeInOut(q) * g;
        laugh = Math.max(laugh, arc * g);
        if (envelope(q) * g > whirl) {
          whirl = envelope(q) * g;
          whirlAngle = ringAngle(q);
        }
      }
      rollAdd += (this.hopCount % 2 === 0 ? 1 : -1) * 6 * DEG * arc * g;
    }
    if (this.state === "working" && t >= this.laughAt && !this.laughEv.active) {
      this.laughEv.fire();
      this.laughEv.duration = 0.6 + this.rand() * 0.5;
      this.laughAt = t + 1.6 + this.rand() * 2.2;
    }
    this.laughEv.update(dt);
    if (this.laughEv.active) {
      const q = this.laughEv.p;
      laugh = Math.max(laugh, q < 0.18 ? q / 0.18 : q > 0.78 ? (1 - q) / 0.22 : 1);
    }
    if (this.state === "sleeping" && t >= this.nodAt && !this.nod.active) {
      this.nod.fire();
      this.nodAt = t + 4 + this.rand() * 4;
    }
    this.nod.update(dt);
    if (this.nod.active) {
      const q = this.nod.p;
      const dip = q < 0.72 ? easeInOut(q / 0.72) : 1 - easeInOut((q - 0.72) / 0.28);
      pitchAdd -= 13 * DEG * dip * ws;
    }
    this.breathPhase += dt / (3.6 + 1.2 * ws);
    const breath = Math.sin(this.breathPhase * TAU);
    p.breath = breath;
    sx += breath * (8e-3 + 0.014 * ws);
    sy += breath * (0.012 + 0.02 * ws);
    const bob = Math.sin(t * TAU / 3.4) * 2 * (1 - ws);
    p.yaw = this.baseYaw + spin;
    let dyaw = this.baseYaw - this.prevYaw;
    dyaw = ((dyaw + Math.PI) % TAU + TAU) % TAU - Math.PI;
    this.prevYaw = this.baseYaw;
    const rate = dt > 0 ? Math.abs(dyaw) / dt : 0;
    const leadTarget = dt > 0 ? Math.max(-2.2, Math.min(2.2, dyaw / dt * 2.4)) : 0;
    this.gazeLead = approach(this.gazeLead, leadTarget, 9, dt);
    const jellyTarget = Math.min(0.22, 0.055 * rate);
    const omega = 16, zeta = 0.45;
    this.jellyV += (omega * omega * (jellyTarget - this.jelly) - 2 * zeta * omega * this.jellyV) * dt;
    this.jelly += this.jellyV * dt;
    const jelly = Math.max(-0.08, Math.min(0.28, this.jelly)) * 0.6;
    sx *= 1 + jelly;
    sy *= 1 - 0.55 * jelly;
    p.pitch = basePitch + pitchAdd;
    p.roll = baseRoll + rollAdd;
    p.x = 0;
    p.y = baseY + hopY + bob;
    p.sx = sx;
    p.sy = sy;
    p.eyeOpen = 1;
    p.laugh = approach(p.laugh, laugh, 30, dt);
    p.blinkL = blinkClose;
    p.blinkR = blinkClose;
    p.lookX = baseLookX + lookXAdd + this.gazeLead;
    p.lookY = baseLookY + lookYAdd;
    p.whirl = whirl;
    p.whirlAngle = whirlAngle;
  }
}
function restPose(state2) {
  const r = REST[state2];
  return {
    yaw: 0,
    pitch: r.pitch,
    roll: r.roll,
    x: 0,
    y: r.y,
    sx: 1,
    sy: 1,
    eyeOpen: 1,
    blinkL: 0,
    blinkR: 0,
    lookX: r.lookX,
    lookY: r.lookY,
    breath: 0,
    laugh: 0,
    whirl: 0,
    whirlAngle: 0,
    w: STATES.map((s) => s === state2 ? 1 : 0)
  };
}
const botAvatarPresets = {
  clover: { label: "Clover", color: "#41C4FF", face: "eyes", faceX: 50, faceY: 50, faceScale: 1, brightness: 1.2, saturation: 1.59 },
  flower: { label: "Flower", color: "#2FCB7A", face: "eyes", faceX: 50, faceY: 51, faceScale: 0.95 },
  triangle: { label: "Triangle", color: "#DC48FF", face: "eyes", faceX: 50, faceY: 61, faceScale: 0.9 },
  square: { label: "Square", color: "#35B8FF", face: "eyes", faceX: 50, faceY: 50, faceScale: 1 },
  blob: { label: "Blob", color: "#2FCB7A", face: "eyes", faceX: 49.5, faceY: 50, faceScale: 1 },
  ghost: { label: "Ghost", color: "#F4F2FA", face: "eyes", faceX: 50, faceY: 48, faceScale: 0.95 },
  circle: { label: "Circle", color: "#9A62FF", face: "eyes", faceX: 50, faceY: 50, faceScale: 1 },
  drop: { label: "Drop", color: "#1ED3C6", face: "eyes", faceX: 50, faceY: 62, faceScale: 0.9 },
  star: { label: "Star", color: "#EFDB8E", face: "eyes", faceX: 50, faceY: 52, faceScale: 0.82, brightness: 1.1, saturation: 1.84 },
  droid: { label: "Droid", color: "#D5DBEA", face: "eyes", faceX: 50, faceY: 60, faceScale: 0.95 },
  mech: { label: "Mech", color: "#95A6C4", face: "eyes", faceX: 50, faceY: 59, faceScale: 1 },
  alien: { label: "Alien", color: "#9CD66A", face: "eyes", faceX: 50, faceY: 45, faceScale: 1.05 },
  hexagon: { label: "Hexagon", color: "#EB7575", face: "eyes", faceX: 50, faceY: 50, faceScale: 0.95 },
  cat: { label: "Cat", color: "#EAA06F", face: "eyes", faceX: 50, faceY: 58, faceScale: 1, brightness: 1.22, saturation: 1.88 },
  cloud: { label: "Cloud", color: "#CFD5E4", face: "eyes", faceX: 50, faceY: 58, faceScale: 0.95 },
  pill: { label: "Pill", color: "#ACAAF3", face: "eyes", faceX: 50, faceY: 50, faceScale: 0.9 },
  pebble: { label: "Pebble", color: "#ABC793", face: "eyes", faceX: 50, faceY: 50, faceScale: 0.95 },
  puddle: { label: "Puddle", color: "#EE8BDB", face: "eyes", faceX: 50, faceY: 50, faceScale: 0.95 }
};
const botAvatarTypes = Object.keys(botAvatarPresets);
Object.fromEntries(
  botAvatarTypes.map((t) => [t, botAvatarPresets[t].color])
);
const stateLabels = {
  default: "idle",
  working: "working",
  sleeping: "sleeping"
};
const SHAPE_PATHS = {
  clover: "M26.53 22.38A25 25 0 0 1 73.47 22.38A7 7 0 0 0 77.62 26.53A25 25 0 0 1 77.62 73.47A7 7 0 0 0 73.47 77.62A25 25 0 0 1 26.53 77.62A7 7 0 0 0 22.38 73.47A25 25 0 0 1 22.38 26.53A7 7 0 0 0 26.53 22.38Z",
  flower: "M31.2 19.82A20.5 20.5 0 0 1 68.8 19.82A5 5 0 0 0 72.9 22.8A20.5 20.5 0 0 1 84.51 58.55A5 5 0 0 0 82.95 63.37A20.5 20.5 0 0 1 52.54 85.47A5 5 0 0 0 47.46 85.47A20.5 20.5 0 0 1 17.05 63.37A5 5 0 0 0 15.49 58.55A20.5 20.5 0 0 1 27.1 22.8A5 5 0 0 0 31.2 19.82Z",
  triangle: "M38.75 27.43A13 13 0 0 1 61.25 27.43L82.7 64.49A13 13 0 0 1 71.45 84L28.55 84A13 13 0 0 1 17.3 64.49L38.75 27.43Z",
  square: "M93 50C93 53.66 92.96 58.23 92.88 60.97C92.8 63.71 92.67 64.81 92.51 66.44C92.35 68.08 92.15 69.44 91.9 70.77C91.66 72.1 91.37 73.3 91.04 74.44C90.72 75.58 90.35 76.63 89.94 77.63C89.53 78.63 89.07 79.55 88.58 80.43C88.08 81.31 87.54 82.13 86.96 82.9C86.37 83.67 85.75 84.39 85.07 85.07C84.39 85.75 83.67 86.37 82.9 86.96C82.13 87.54 81.31 88.08 80.43 88.58C79.55 89.07 78.63 89.53 77.63 89.94C76.63 90.35 75.58 90.72 74.44 91.04C73.3 91.37 72.1 91.66 70.77 91.9C69.44 92.15 68.08 92.35 66.44 92.51C64.81 92.67 63.71 92.8 60.97 92.88C58.23 92.96 53.66 93 50 93C46.34 93 41.77 92.96 39.03 92.88C36.29 92.8 35.19 92.67 33.56 92.51C31.92 92.35 30.56 92.15 29.23 91.9C27.9 91.66 26.7 91.37 25.56 91.04C24.42 90.72 23.37 90.35 22.37 89.94C21.37 89.53 20.45 89.07 19.57 88.58C18.69 88.08 17.87 87.54 17.1 86.96C16.33 86.37 15.61 85.75 14.93 85.07C14.25 84.39 13.63 83.67 13.04 82.9C12.46 82.13 11.92 81.31 11.42 80.43C10.93 79.55 10.47 78.63 10.06 77.63C9.65 76.63 9.28 75.58 8.96 74.44C8.63 73.3 8.34 72.1 8.1 70.77C7.85 69.44 7.65 68.08 7.49 66.44C7.33 64.81 7.2 63.71 7.12 60.97C7.04 58.23 7 53.66 7 50C7 46.34 7.04 41.77 7.12 39.03C7.2 36.29 7.33 35.19 7.49 33.56C7.65 31.92 7.85 30.56 8.1 29.23C8.34 27.9 8.63 26.7 8.96 25.56C9.28 24.42 9.65 23.37 10.06 22.37C10.47 21.37 10.93 20.45 11.42 19.57C11.92 18.69 12.46 17.87 13.04 17.1C13.63 16.33 14.25 15.61 14.93 14.93C15.61 14.25 16.33 13.63 17.1 13.04C17.87 12.46 18.69 11.92 19.57 11.42C20.45 10.93 21.37 10.47 22.37 10.06C23.37 9.65 24.42 9.28 25.56 8.96C26.7 8.63 27.9 8.34 29.23 8.1C30.56 7.85 31.92 7.65 33.56 7.49C35.19 7.33 36.29 7.2 39.03 7.12C41.77 7.04 46.34 7 50 7C53.66 7 58.23 7.04 60.97 7.12C63.71 7.2 64.81 7.33 66.44 7.49C68.08 7.65 69.44 7.85 70.77 8.1C72.1 8.34 73.3 8.63 74.44 8.96C75.58 9.28 76.63 9.65 77.63 10.06C78.63 10.47 79.55 10.93 80.43 11.42C81.31 11.92 82.13 12.46 82.9 13.04C83.67 13.63 84.39 14.25 85.07 14.93C85.75 15.61 86.37 16.33 86.96 17.1C87.54 17.87 88.08 18.69 88.58 19.57C89.07 20.45 89.53 21.37 89.94 22.37C90.35 23.37 90.72 24.42 91.04 25.56C91.37 26.7 91.66 27.9 91.9 29.23C92.15 30.56 92.35 31.92 92.51 33.56C92.67 35.19 92.8 36.29 92.88 39.03C92.96 41.77 93 46.34 93 50Z",
  blob: "M93.92 50C94.18 51.9 94.24 53.9 93.99 55.79C93.73 57.68 93.18 59.61 92.39 61.36C91.6 63.1 90.48 64.78 89.27 66.27C88.07 67.76 86.58 69.08 85.14 70.29C83.7 71.49 82.1 72.5 80.62 73.5C79.14 74.49 77.64 75.34 76.24 76.24C74.84 77.15 73.51 78 72.2 78.93C70.88 79.86 69.66 80.83 68.36 81.8C67.06 82.76 65.78 83.82 64.39 84.74C63 85.67 61.55 86.63 60.01 87.35C58.47 88.08 56.82 88.73 55.15 89.1C53.48 89.47 51.71 89.64 50 89.57C48.29 89.5 46.55 89.16 44.91 88.68C43.27 88.21 41.67 87.47 40.16 86.72C38.65 85.96 37.24 85.02 35.86 84.13C34.48 83.24 33.19 82.29 31.89 81.37C30.58 80.46 29.32 79.56 28.03 78.63C26.74 77.71 25.44 76.82 24.16 75.84C22.88 74.86 21.57 73.87 20.36 72.74C19.15 71.62 17.95 70.42 16.91 69.1C15.88 67.79 14.92 66.35 14.15 64.85C13.38 63.35 12.76 61.73 12.29 60.1C11.81 58.48 11.52 56.78 11.29 55.1C11.06 53.41 10.98 51.71 10.91 50C10.83 48.29 10.83 46.59 10.83 44.84C10.84 43.1 10.85 41.34 10.93 39.53C11.02 37.72 11.09 35.86 11.34 33.99C11.59 32.12 11.88 30.16 12.43 28.31C12.99 26.46 13.69 24.56 14.66 22.88C15.64 21.21 16.86 19.58 18.27 18.27C19.69 16.96 21.4 15.84 23.17 15.03C24.93 14.22 26.95 13.71 28.89 13.43C30.83 13.15 32.9 13.21 34.82 13.36C36.74 13.5 38.66 13.92 40.43 14.28C42.2 14.65 43.87 15.16 45.46 15.54C47.06 15.93 48.52 16.31 50 16.58C51.48 16.85 52.87 17.02 54.32 17.17C55.77 17.32 57.22 17.36 58.71 17.49C60.2 17.62 61.74 17.7 63.28 17.95C64.81 18.19 66.39 18.5 67.91 18.98C69.43 19.46 70.95 20.08 72.38 20.83C73.81 21.58 75.19 22.5 76.5 23.5C77.81 24.5 79.03 25.63 80.22 26.81C81.41 27.99 82.54 29.25 83.65 30.57C84.76 31.89 85.85 33.26 86.89 34.72C87.93 36.18 88.98 37.69 89.9 39.31C90.82 40.92 91.73 42.64 92.4 44.42C93.07 46.2 93.66 48.1 93.92 50Z",
  ghost: "M17 50C17 31.78 31.78 17 50 17C68.22 17 83 31.78 83 50V81.5Q72 93.5 61 81.5Q50 93.5 39 81.5Q28 93.5 17 81.5Z",
  circle: "M50 8A42 42 0 1 1 50 92A42 42 0 1 1 50 8Z",
  drop: "M50 8.5C52.2 8.5 53.4 10.3 56.4 15.2C62.9 25.5 84 44.6 84 61.5C84 80.3 68.8 92 50 92C31.2 92 16 80.3 16 61.5C16 44.6 37.1 25.5 43.6 15.2C46.6 10.3 47.8 8.5 50 8.5Z",
  star: "M45.77 9.7A5 5 0 0 1 54.23 9.7L65.02 26.81A4 4 0 0 0 67.42 28.55L87.02 33.53A5 5 0 0 1 89.63 41.57L76.7 57.12A4 4 0 0 0 75.78 59.94L77.11 80.12A5 5 0 0 1 70.26 85.09L51.48 77.59A4 4 0 0 0 48.52 77.59L29.74 85.09A5 5 0 0 1 22.89 80.12L24.22 59.94A4 4 0 0 0 23.3 57.12L10.37 41.57A5 5 0 0 1 12.98 33.53L32.58 28.55A4 4 0 0 0 34.98 26.81L45.77 9.7Z",
  droid: "M16 50C16 38.95 24.95 30 36 30H64C75.05 30 84 38.95 84 50V70C84 81.05 75.05 90 64 90H36C24.95 90 16 81.05 16 70ZM4 62A7 7 0 1 1 18 62A7 7 0 1 1 4 62ZM82 62A7 7 0 1 1 96 62A7 7 0 1 1 82 62Z",
  mech: "M10 48C10 38.06 18.06 30 28 30H72C81.94 30 90 38.06 90 48V70C90 79.94 81.94 88 72 88H28C18.06 88 10 79.94 10 70ZM3 54C3 51.79 4.79 50 7 50H11V72H7C4.79 72 3 70.21 3 68ZM89 50H93C95.21 50 97 51.79 97 54V68C97 70.21 95.21 72 93 72H89Z",
  alien: "M50 10C70 10 83 27 83 46C83 65 64 92 50 92C36 92 17 65 17 46C17 27 30 10 50 10Z",
  hexagon: "M91.4 45.5A9 9 0 0 1 91.4 54.5L74.6 83.61A9 9 0 0 1 66.8 88.11L33.2 88.11A9 9 0 0 1 25.4 83.61L8.6 54.5A9 9 0 0 1 8.6 45.5L25.4 16.39A9 9 0 0 1 33.2 11.89L66.8 11.89A9 9 0 0 1 74.6 16.39L91.4 45.5Z",
  cat: "M50 20A36 36 0 1 1 50 92A36 36 0 1 1 50 20ZM24.72 44.64A4.5 4.5 0 0 1 17.35 40.67L20.21 16.66A4.5 4.5 0 0 1 26.86 13.26L42.3 21.83A4.5 4.5 0 0 1 43.02 29.21L24.72 44.64ZM82.65 40.67A4.5 4.5 0 0 1 75.28 44.64L56.98 29.21A4.5 4.5 0 0 1 57.7 21.83L73.14 13.26A4.5 4.5 0 0 1 79.79 16.66L82.65 40.67Z",
  cloud: "M19 44A25 25 0 1 1 69 44A25 25 0 1 1 19 44ZM47 50A21 21 0 1 1 89 50A21 21 0 1 1 47 50ZM7 68A17 17 0 1 1 41 68A17 17 0 1 1 7 68ZM32 73A18 18 0 1 1 68 73A18 18 0 1 1 32 73ZM60 70A16 16 0 1 1 92 70A16 16 0 1 1 60 70Z",
  pill: "M28 28H72C84.15 28 94 37.85 94 50C94 62.15 84.15 72 72 72H28C15.85 72 6 62.15 6 50C6 37.85 15.85 28 28 28Z",
  pebble: "M94.6 50C94.71 51.92 94.56 53.93 94.17 55.82C93.79 57.7 93.13 59.6 92.28 61.33C91.44 63.06 90.32 64.72 89.1 66.19C87.87 67.67 86.41 69.01 84.92 70.16C83.42 71.31 81.75 72.29 80.11 73.1C78.47 73.92 76.73 74.55 75.06 75.06C73.39 75.58 71.7 75.91 70.1 76.2C68.5 76.48 66.93 76.62 65.45 76.76C63.96 76.9 62.55 76.94 61.2 77.03C59.84 77.11 58.57 77.16 57.31 77.26C56.05 77.36 54.86 77.48 53.64 77.63C52.42 77.79 51.24 77.98 50 78.18C48.76 78.39 47.52 78.64 46.2 78.85C44.89 79.06 43.53 79.3 42.11 79.46C40.69 79.61 39.2 79.75 37.67 79.77C36.14 79.78 34.54 79.74 32.95 79.54C31.35 79.33 29.7 79.03 28.1 78.55C26.49 78.07 24.86 77.45 23.33 76.67C21.8 75.88 20.29 74.94 18.92 73.85C17.54 72.77 16.23 71.51 15.08 70.16C13.94 68.8 12.9 67.29 12.04 65.72C11.17 64.16 10.45 62.46 9.91 60.74C9.36 59.03 8.98 57.22 8.76 55.43C8.55 53.64 8.5 51.8 8.6 50C8.71 48.2 8.98 46.39 9.38 44.65C9.79 42.91 10.35 41.19 11.02 39.56C11.7 37.92 12.52 36.34 13.43 34.85C14.34 33.37 15.39 31.96 16.5 30.66C17.61 29.36 18.84 28.15 20.11 27.07C21.39 25.98 22.76 25 24.15 24.15C25.54 23.3 27.01 22.56 28.48 21.95C29.94 21.33 31.46 20.85 32.95 20.46C34.44 20.08 35.95 19.83 37.43 19.66C38.91 19.48 40.38 19.43 41.81 19.43C43.24 19.43 44.64 19.53 46.01 19.66C47.37 19.78 48.7 19.98 50 20.18C51.3 20.39 52.57 20.63 53.83 20.87C55.1 21.11 56.34 21.37 57.6 21.62C58.87 21.87 60.13 22.12 61.43 22.39C62.74 22.66 64.07 22.93 65.45 23.24C66.83 23.56 68.25 23.88 69.72 24.3C71.19 24.72 72.71 25.17 74.25 25.75C75.78 26.34 77.37 27 78.91 27.81C80.46 28.63 82.04 29.56 83.5 30.66C84.97 31.75 86.43 33 87.7 34.38C88.98 35.77 90.19 37.32 91.17 38.97C92.14 40.62 92.98 42.43 93.56 44.27C94.13 46.1 94.5 48.08 94.6 50Z",
  puddle: "M85.34 50C85.65 51.54 85.9 53.13 86.01 54.74C86.11 56.35 86.11 58 85.95 59.63C85.8 61.26 85.51 62.93 85.07 64.52C84.62 66.12 84.03 67.72 83.3 69.23C82.58 70.74 81.7 72.21 80.72 73.57C79.74 74.94 78.62 76.23 77.43 77.43C76.24 78.62 74.93 79.72 73.58 80.73C72.23 81.74 70.79 82.65 69.33 83.48C67.86 84.31 66.34 85.04 64.79 85.71C63.24 86.37 61.65 86.95 60.04 87.46C58.42 87.96 56.77 88.4 55.1 88.75C53.43 89.09 51.72 89.37 50 89.54C48.28 89.71 46.52 89.81 44.76 89.76C43.01 89.72 41.22 89.58 39.47 89.29C37.72 89 35.95 88.58 34.26 88C32.57 87.42 30.89 86.69 29.33 85.8C27.77 84.92 26.26 83.87 24.91 82.7C23.56 81.53 22.3 80.19 21.23 78.77C20.16 77.35 19.22 75.79 18.47 74.2C17.71 72.61 17.13 70.91 16.7 69.23C16.26 67.55 16.02 65.81 15.88 64.13C15.75 62.45 15.78 60.76 15.88 59.14C15.97 57.52 16.21 55.94 16.45 54.42C16.7 52.89 17.03 51.43 17.34 50C17.65 48.57 18 47.2 18.32 45.83C18.63 44.46 18.94 43.13 19.24 41.76C19.53 40.39 19.79 39.03 20.08 37.61C20.36 36.18 20.62 34.74 20.95 33.23C21.28 31.72 21.61 30.16 22.07 28.57C22.52 26.98 23.02 25.32 23.69 23.69C24.35 22.06 25.11 20.38 26.05 18.79C26.99 17.21 28.08 15.62 29.33 14.2C30.58 12.78 32 11.41 33.54 10.26C35.08 9.12 36.8 8.11 38.57 7.35C40.34 6.6 42.27 6.04 44.17 5.73C46.08 5.43 48.08 5.37 50 5.54C51.92 5.71 53.87 6.15 55.69 6.75C57.52 7.36 59.3 8.22 60.94 9.19C62.57 10.15 64.11 11.33 65.51 12.56C66.91 13.78 68.17 15.15 69.33 16.52C70.48 17.89 71.5 19.34 72.44 20.76C73.38 22.18 74.19 23.62 74.97 25.03C75.75 26.43 76.44 27.83 77.12 29.19C77.8 30.56 78.42 31.89 79.05 33.23C79.67 34.57 80.28 35.87 80.87 37.21C81.46 38.55 82.05 39.88 82.6 41.27C83.14 42.65 83.68 44.05 84.14 45.51C84.6 46.96 85.03 48.46 85.34 50Z"
};
const SHAPE_PART_COLORS = {
  ghost: ["#8f8f8f", "#4ade80"]
};
const SHAPE_PARTS = {
  droid: "M47.5 14H52.5V32H47.5ZM43 11A7 7 0 1 1 57 11A7 7 0 1 1 43 11Z",
  ghost: "M47.5 9H52.5V20H47.5ZM44 7A6 6 0 1 1 56 7A6 6 0 1 1 44 7Z",
  mech: "M19.5 32L24.5 32L17 13L12 13ZM75.5 32L80.5 32L88 13L83 13ZM10 11.5A4.5 4.5 0 1 1 19 11.5A4.5 4.5 0 1 1 10 11.5ZM81 11.5A4.5 4.5 0 1 1 90 11.5A4.5 4.5 0 1 1 81 11.5Z"
};
const pointer = { x: NaN, y: NaN };
const subs = /* @__PURE__ */ new Set();
let raf = 0;
let last = 0;
function frame(now) {
  raf = 0;
  const dt = last ? Math.min(0.1, (now - last) / 1e3) : 0;
  last = now;
  subs.forEach((fn) => fn(dt));
  if (subs.size) raf = requestAnimationFrame(frame);
}
function start() {
  if (raf || typeof document === "undefined" || document.hidden) return;
  last = 0;
  raf = requestAnimationFrame(frame);
}
let wired = false;
function wire() {
  if (wired || typeof document === "undefined") return;
  wired = true;
  document.addEventListener("pointermove", (e) => {
    pointer.x = e.clientX;
    pointer.y = e.clientY;
  }, { passive: true });
  document.addEventListener("pointerleave", () => {
    pointer.x = NaN;
    pointer.y = NaN;
  });
  window.addEventListener("blur", () => {
    pointer.x = NaN;
    pointer.y = NaN;
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    } else if (subs.size) start();
  });
}
function subscribe(fn) {
  wire();
  subs.add(fn);
  start();
  return () => {
    subs.delete(fn);
    if (!subs.size && raf) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
  };
}
var _tmpl$$1 = /* @__PURE__ */ web.template(`<canvas>`);
function hashSeed(id) {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
  return (h >>> 0) % 1e3 / 1e3;
}
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : 1));
const pathCache = /* @__PURE__ */ new Map();
function bodyPath(d) {
  let p = pathCache.get(d);
  if (!p) {
    p = new Path2D(d);
    pathCache.set(d, p);
  }
  return p;
}
const partsCache = /* @__PURE__ */ new Map();
function partPaths(d) {
  let p = partsCache.get(d);
  if (!p) {
    p = d.split(/(?=M)/).filter((s) => s.trim()).map((s) => new Path2D(s));
    partsCache.set(d, p);
  }
  return p;
}
const SHOW_WITHIN = 4e3;
const waiters = /* @__PURE__ */ new Map();
function showReady() {
  if (bakesPending()) return;
  for (const w of waiters.values()) if (!w.ready()) return;
  const all = [...waiters.values()];
  waiters.clear();
  for (const w of all) w.show();
}
const reducedMotion = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
function shadingModeOf(shading) {
  if (shading === true) return "crisp";
  if (shading === false) return "flat";
  return shading ?? "fabric";
}
function BotAvatar(props) {
  const [local, rest] = solidJs.splitProps(props, ["type", "path", "face", "state", "size", "color", "ink", "brightness", "saturation", "speed", "paused", "seed", "shading", "hat", "glasses", "headphones", "bowTie", "accessoryColor", "shadow", "highlight", "depth", "roundness", "furLength", "furDensity", "furFuzz", "furClumps", "furCurl", "furGravity", "light", "rim", "spread", "backLight", "lightFront", "shine", "sheen", "backSoftness", "pose", "interactive", "turn", "theme", "whirl", "whirlSize", "whirlWidth", "whirlLength", "whirlTilt", "jumpHeight", "jumpTime", "jumpStretch", "jumpSpin", "jumpLean", "jumpEvery", "jumpLand", "jumpSquash", "jumpSquashTime", "jumpSquashEase", "jumpGroundTime", "jumpGroundEase", "jumpRiseTime", "jumpRiseEase", "jumpClickSquashTime", "class", "className", "style", "aria-label", "ref", "onClick"]);
  const instanceId = solidJs.createUniqueId();
  const startsShown = !isMaterial(shadingModeOf(local.shading));
  const [shown, setShown] = solidJs.createSignal(startsShown);
  let shownLive = startsShown;
  let canvasEl;
  let sim = null;
  let cfg = null;
  let cssSize = 0;
  let speedNow = 1;
  let interactiveLive = true;
  let heldNow;
  let stateKey = "default";
  let sizeNow = 64;
  let themeMode = "auto";
  let frozenNow = false;
  let stillPaint = null;
  let waiting = null;
  let showPaint = null;
  let watch = null;
  const me = {};
  let ready = false;
  let lastLook = null;
  let holding = null;
  const markShown = (value) => {
    shownLive = value;
    setShown(value);
  };
  const model = solidJs.createMemo(() => {
    const type = local.type ?? "clover";
    const preset = botAvatarPresets[type] ?? botAvatarPresets.clover;
    const faceKind = local.face ?? preset.face;
    const state2 = local.state ?? "default";
    const stateKeyNow = state2 in stateLabels ? state2 : "default";
    const size = local.size ?? 64;
    const picked = local.color ?? preset.color;
    const brightness = local.brightness ?? preset.brightness ?? 1;
    const saturation = local.saturation ?? preset.saturation ?? 1.5;
    const vivid = clamp(saturation, 1.5, 2.5) - 1.5;
    const body = richer(brightness === 1 && saturation === 1 ? picked : shade(picked, (Math.min(2, Math.max(0, brightness)) - 1) * 0.35, (Math.min(1.5, Math.max(0, saturation)) - 1) * 0.5), vivid);
    const speed = local.speed ?? 1;
    const frozen2 = (local.paused ?? false) || !(speed > 0);
    const shadingMode = shadingModeOf(local.shading);
    const customPath = typeof local.path === "string" && local.path.trim() ? local.path.trim() : null;
    const outlineKey = customPath ? `path:${hashSeed(customPath)}:${customPath.length}` : type;
    const lit = LIGHT_DEFAULTS[shadingMode === "fabric" ? "fabric" : "other"];
    const q = (v, lo, hi) => Math.round(clamp(v, lo, hi) * 20) / 20;
    const fur = {
      length: q(local.furLength ?? 1, 0.3, 2.5),
      density: q(local.furDensity ?? 1.6, 0.3, 2),
      fuzz: q(local.furFuzz ?? 0.9, 0, 1),
      clumps: q(local.furClumps ?? 0.4, 0, 1),
      curl: q(local.furCurl ?? 0.7, 0, 1),
      gravity: q(local.furGravity ?? 0.9, 0, 1)
    };
    const furId = `${fur.length},${fur.density},${fur.fuzz},${fur.clumps},${fur.curl},${fur.gravity}`;
    const depth = local.depth ?? 0.65;
    const part = !customPath ? SHAPE_PARTS[type] : void 0;
    const drawCfg = {
      path: typeof Path2D === "undefined" ? null : bodyPath(customPath ?? SHAPE_PATHS[type] ?? SHAPE_PATHS.clover),
      face: faceKind,
      faceX: preset.faceX,
      faceY: preset.faceY,
      faceScale: preset.faceScale,
      color: body,
      ink: local.ink ?? autoInk(body),
      shading: shadingMode,
      shadow: clamp(local.shadow ?? lit.shadow, 0, 2),
      highlight: clamp(local.highlight ?? lit.highlight, 0, 2),
      depth: clamp(depth, 0.2, 2),
      roundness: clamp(local.roundness ?? 1, 0, 1),
      fur,
      vivid,
      backLight: local.backLight,
      lightFront: local.lightFront,
      shine: local.shine,
      sheen: local.sheen,
      backSoftness: local.backSoftness,
      light: local.light ?? lit.light,
      rim: clamp(local.rim ?? lit.rim, 0, 2),
      spread: clamp(local.spread ?? lit.spread, 0.4, 2.5),
      typeKey: outlineKey,
      still: frozen2 || reducedMotion(),
      furOnIdle: true,
      whirl: {
        strength: clamp(local.whirl ?? 0, 0, 2),
        size: clamp(local.whirlSize ?? 1, 0.6, 1.6),
        width: clamp(local.whirlWidth ?? 1, 0.4, 2),
        length: clamp(local.whirlLength ?? 1, 0.4, 1.6),
        tilt: clamp(local.whirlTilt ?? 1, 0.5, 1.8)
      },
      partsColors: !customPath ? SHAPE_PART_COLORS[type] : void 0,
      parts: typeof Path2D !== "undefined" && part ? partPaths(part) : void 0,
      wear: {
        hat: local.hat ?? "none",
        glasses: local.glasses ?? "none",
        headphones: local.headphones ?? false,
        bowTie: local.bowTie ?? false,
        color: local.accessoryColor ?? "#27272b",
        fabric: shadingMode === "fabric"
      }
    };
    const jump = {
      height: local.jumpHeight ?? 26,
      time: Math.max(0.2, local.jumpTime ?? 0.68),
      stretch: local.jumpStretch ?? 1,
      spin: Math.max(0, Math.round(local.jumpSpin ?? 1)),
      lean: local.jumpLean ?? 6,
      every: local.jumpEvery ?? 8,
      land: local.jumpLand ?? 0,
      squash: local.jumpSquash ?? 1.15,
      squashTime: Math.max(0.05, local.jumpSquashTime ?? 0.37),
      squashEase: local.jumpSquashEase ?? "pulse",
      groundTime: Math.max(0, local.jumpGroundTime ?? 0.11),
      groundEase: local.jumpGroundEase ?? "pulse",
      riseTime: Math.max(0.05, local.jumpRiseTime ?? 0.33),
      riseEase: local.jumpRiseEase ?? "pulse",
      clickSquashTime: Math.max(0.05, local.jumpClickSquashTime ?? 0.24)
    };
    return {
      type,
      preset,
      faceKind,
      stateKey: stateKeyNow,
      size,
      seedValue: Math.min(1, Math.max(0, local.seed ?? hashSeed(instanceId))),
      speed,
      frozen: frozen2,
      shadingMode,
      material: isMaterial(shadingMode),
      outlineKey,
      fur,
      furId,
      depth,
      backLight: local.backLight,
      lightFront: local.lightFront,
      light: local.light,
      lookKey: `${shadingMode}|${outlineKey}|${size}`,
      theme: local.theme ?? "auto",
      turn: local.turn ?? 1,
      interactive: local.interactive ?? true,
      heldPose: local.pose,
      draw: drawCfg,
      jump
    };
  });
  const warmKey = solidJs.createMemo(() => {
    const m = model();
    return `${m.shadingMode}|${m.outlineKey}|${String(m.size)}|${m.depth}|${m.furId}|${m.light ?? ""}|${m.backLight ?? ""}|${m.lightFront ?? ""}`;
  });
  const resolveTheme = (el) => {
    if (themeMode !== "auto") return themeMode;
    const host = el?.closest("[data-theme], .dark, .light");
    if (host) {
      const v = host.getAttribute("data-theme");
      if (v === "dark" || v === "light") return v;
      if (host.classList.contains("dark")) return "dark";
      if (host.classList.contains("light")) return "light";
    }
    return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  };
  function paint() {
    const canvas = canvasEl;
    const c = cfg;
    if (!canvas || !canvas.isConnected || !c || !c.path) return;
    const px = canvas.clientWidth / OVERSCAN || cssSize || (typeof sizeNow === "number" ? sizeNow : 64);
    if (!px) return;
    const dpr = Math.min(2, typeof devicePixelRatio === "number" && devicePixelRatio || 1);
    const want = Math.round(px * OVERSCAN * dpr);
    if (canvas.width !== want || canvas.height !== want) {
      canvas.width = want;
      canvas.height = want;
    }
    cssSize = px;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.dpr = dpr;
    const held = heldNow;
    const rest0 = restPose(stateKey);
    const pose = held ? {
      ...rest0,
      yaw: held.yaw ?? 0,
      pitch: held.pitch ?? rest0.pitch,
      roll: held.roll ?? rest0.roll
    } : sim ? sim.pose : rest0;
    draw(ctx, px, pose, c);
  }
  function paintStill() {
    const canvas = canvasEl;
    if (!canvas || !canvas.isConnected || !cfg || !cfg.path) return;
    const px = canvas.clientWidth / OVERSCAN || cssSize || (typeof sizeNow === "number" ? sizeNow : 64);
    if (!px) return;
    cssSize = px;
    const dpr = Math.min(2, typeof devicePixelRatio === "number" && devicePixelRatio || 1);
    canvas.width = canvas.height = Math.round(px * OVERSCAN * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    cfg.theme = resolveTheme(canvas);
    cfg.dpr = dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw(ctx, px, restPose(stateKey), cfg);
  }
  function stopWaiting() {
    if (waiting) waiting();
    waiting = null;
  }
  function settle(paintNow) {
    takeMissed();
    paintNow();
    if (!takeMissed()) stopWaiting();
    else if (!waiting) {
      waiting = onLanded(() => {
        const p = stillPaint;
        if (canvasEl && p) settle(p);
        else stopWaiting();
      });
    }
  }
  function stopWatch() {
    if (watch) watch();
    watch = null;
  }
  function showWhenBaked(paintNow) {
    showPaint = paintNow;
    takeMissed();
    paintNow();
    ready = !takeMissed();
    if (ready && !bakesPending()) {
      showReady();
      return;
    }
    if (!watch) {
      watch = onLanded(() => {
        const p = showPaint;
        if (canvasEl && p && !shownLive) showWhenBaked(p);
        else stopWatch();
        showReady();
      });
    }
  }
  function warm() {
    const m = solidJs.untrack(model);
    if (!m.material || !cfg?.path) return;
    const path = cfg.path;
    const dev = (typeof m.size === "number" ? m.size : 64) * Math.min(2, typeof devicePixelRatio === "number" && devicePixelRatio || 1);
    const lightAt = cfg.light ?? 295;
    const a = lightAt * Math.PI / 180;
    const b = (m.backLight ?? 0) * Math.PI / 180;
    const lights = stockLights(Math.sin(a), -Math.cos(a));
    if (m.backLight !== void 0) [lights.bx, lights.by] = [Math.sin(b), -Math.cos(b)];
    if (m.lightFront !== void 0) lights.front = Math.min(85, Math.max(0, m.lightFront)) * Math.PI / 180;
    warmPlastic(m.outlineKey, path, dev, m.depth, m.shadingMode === "fabric", m.fur, lightAt, lights);
  }
  function hold() {
    warm();
    if (!bakesPending()) return;
    hurry(true);
    const release = () => {
      if (!holding) return;
      holding();
      layoutNow();
    };
    const off = onLanded(() => {
      if (!bakesPending()) release();
    });
    const t = window.setTimeout(release, SHOW_WITHIN);
    holding = () => {
      off();
      window.clearTimeout(t);
      hurry(false);
      holding = null;
    };
  }
  let attachWait = 0;
  let disposed = false;
  function layoutWhenAttached() {
    if (attachWait || disposed || typeof requestAnimationFrame !== "function") return;
    attachWait = requestAnimationFrame(() => {
      attachWait = 0;
      if (disposed || !canvasEl) return;
      if (canvasEl.isConnected) layoutNow();
      else layoutWhenAttached();
    });
  }
  function layoutNow() {
    const m = model();
    cfg = m.draw;
    speedNow = m.speed;
    interactiveLive = m.interactive && !m.heldPose;
    heldNow = m.heldPose;
    stateKey = m.stateKey;
    sizeNow = m.size;
    themeMode = m.theme;
    frozenNow = m.frozen;
    shownLive = shown();
    if (!sim) sim = new Sim(m.seedValue, m.stateKey);
    else sim.setState(m.stateKey);
    sim.setTurn(clamp(m.turn, 0, 2));
    if (shownLive && m.material && lastLook !== null && lastLook !== m.lookKey && !holding) hold();
    lastLook = m.lookKey;
    sim.setJump(m.jump);
    if (holding) return;
    if (canvasEl && !canvasEl.isConnected) {
      layoutWhenAttached();
      return;
    }
    if (reducedMotion()) {
      stillPaint = paintStill;
      if (shownLive) settle(paintStill);
      else showWhenBaked(paintStill);
      return;
    }
    if (cfg && canvasEl) cfg.theme = resolveTheme(canvasEl);
    if (!shownLive) {
      stillPaint = frozenNow ? paint : null;
      showWhenBaked(paint);
    } else if (frozenNow) {
      stillPaint = paint;
      settle(paint);
    } else {
      stillPaint = null;
      stopWaiting();
      paint();
    }
  }
  solidJs.createEffect(layoutNow);
  solidJs.createEffect(() => {
    warmKey();
    const m = solidJs.untrack(model);
    if (!m.material || !cfg?.path) return;
    let idle2 = false;
    const id = typeof requestIdleCallback === "function" ? (idle2 = true, requestIdleCallback(() => warm())) : window.setTimeout(() => warm(), 1);
    solidJs.onCleanup(() => {
      if (idle2 && typeof cancelIdleCallback === "function") cancelIdleCallback(id);
      else window.clearTimeout(id);
    });
  });
  const frozen = solidJs.createMemo(() => model().frozen);
  solidJs.createEffect(() => {
    if (frozen() || reducedMotion()) return;
    const canvas = canvasEl;
    if (!canvas) return;
    let unsub = null;
    const REACH = 3;
    const tick = (dt) => {
      const s = sim;
      if (!s) return;
      if (interactiveLive && !Number.isNaN(pointer.x)) {
        const r = canvas.getBoundingClientRect();
        const box = r.width / OVERSCAN || 1;
        const dx = (pointer.x - (r.left + r.width / 2)) / box;
        const dy = (pointer.y - (r.top + r.height / 2 + RISE * box)) / box;
        const d = Math.hypot(dx, dy);
        const strength = d < 1 ? 1 : d > REACH ? 0 : 1 - (d - 1) / (REACH - 1);
        s.setPointer(dx / Math.max(1, d), dy / Math.max(1, d), strength);
      } else s.setPointer(0, 0, 0);
      s.update(dt * speedNow);
      if (shownLive && !holding) paint();
    };
    const run = () => {
      if (!unsub) unsub = subscribe(tick);
    };
    const stop = () => {
      if (unsub) unsub();
      unsub = null;
    };
    let io = null;
    if (typeof IntersectionObserver === "function") {
      io = new IntersectionObserver((entries) => {
        if (entries[0]?.isIntersecting ?? true) run();
        else stop();
      });
      io.observe(canvas);
    } else run();
    solidJs.onCleanup(() => {
      stop();
      io?.disconnect();
    });
  });
  solidJs.createEffect(() => {
    if (shown()) return;
    waiters.set(me, {
      ready: () => ready,
      show: () => markShown(true)
    });
    showReady();
    hurry(true);
    const t = window.setTimeout(() => markShown(true), SHOW_WITHIN);
    solidJs.onCleanup(() => {
      hurry(false);
      window.clearTimeout(t);
      stopWatch();
      waiters.delete(me);
      showReady();
    });
  });
  solidJs.onCleanup(() => {
    disposed = true;
    if (attachWait) cancelAnimationFrame(attachWait);
    stopWaiting();
    holding?.();
  });
  const avatarClass = () => {
    const extra = local.class ?? local.className;
    return extra ? `ba ${extra}` : "ba";
  };
  const canvasStyle = () => {
    const size = model().size;
    const dim = typeof size === "number" ? `${size * OVERSCAN}px` : `calc(${size} * ${OVERSCAN})`;
    const pull = (k) => typeof size === "number" ? `${-size * k}px` : `calc(${size} * ${-k})`;
    const side = (OVERSCAN - 1) / 2;
    const base = {
      display: "inline-block",
      "vertical-align": "middle",
      width: dim,
      height: dim,
      "margin-left": pull(side),
      "margin-right": pull(side),
      "margin-top": pull(side + RISE),
      "margin-bottom": pull(side - RISE),
      flex: "none",
      opacity: shown() ? 1 : 0,
      transition: "opacity 0.3s ease-out"
    };
    const extra = local.style;
    return extra && typeof extra === "object" ? {
      ...base,
      ...extra
    } : base;
  };
  const handleClick = (event) => {
    const m = model();
    if (m.interactive && !m.frozen && !m.heldPose) sim?.poke();
    const handler = local.onClick;
    if (typeof handler === "function") handler(event);
    else if (handler) handler[0](handler[1], event);
  };
  const setRef = (el) => {
    canvasEl = el;
    const ref = local.ref;
    if (typeof ref === "function") ref(el);
  };
  return (() => {
    var _el$ = _tmpl$$1();
    web.use(setRef, _el$);
    web.spread(_el$, web.mergeProps(rest, {
      get ["class"]() {
        return avatarClass();
      },
      get ["data-bot-avatar"]() {
        return model().type;
      },
      get ["data-face"]() {
        return model().faceKind;
      },
      get ["data-state"]() {
        return model().stateKey;
      },
      "role": "img",
      get ["aria-label"]() {
        return local["aria-label"] ?? `${model().preset.label} bot, ${stateLabels[model().stateKey]}`;
      },
      get style() {
        return canvasStyle();
      },
      "onClick": handleClick
    }), false, false);
    return _el$;
  })();
}
function isMaterial(mode) {
  return mode === "plastic" || mode === "fabric";
}
const MARKS = [
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
  { id: "kimi", aliases: ["kimi", "moonshot", "moonshotai"], type: "pill", color: "#A5B4FC" }
];
function agentMarkFor(label) {
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
function hash32(id) {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
  return h >>> 0;
}
const FABRIC = {
  shading: "fabric",
  furLength: 1,
  furDensity: 1.6,
  furFuzz: 0.9
};
function EmptyBot(props) {
  return web.createComponent(BotAvatar, web.mergeProps(FABRIC, {
    type: "ghost",
    get state() {
      return props.mood === "sleeping" ? "sleeping" : "default";
    },
    get size() {
      return props.size ?? 64;
    },
    jumpEvery: 0,
    paused: true,
    get turn() {
      return props.mood === "lost" ? 1 : 0.35;
    },
    "aria-hidden": "true"
  }));
}
function AgentBot(props) {
  const key = () => props.agent?.trim() || "agent";
  const mark = () => agentMarkFor(key());
  const type = () => mark()?.type ?? botAvatarTypes[hash32(key()) % botAvatarTypes.length] ?? "clover";
  const working = () => props.status === "PENDING" || props.status === "RUNNING";
  const failed = () => ["FAILED", "CANCELLED", "TIMED_OUT"].includes(props.status);
  return web.createComponent(BotAvatar, web.mergeProps(FABRIC, {
    get type() {
      return type();
    },
    get color() {
      return mark()?.color;
    },
    get state() {
      return web.memo(() => !!working())() ? "working" : failed() ? "sleeping" : "default";
    },
    get size() {
      return props.size ?? 28;
    },
    get seed() {
      return hash32(props.seed) % 1e3 / 1e3;
    },
    "aria-hidden": "true"
  }, () => working() ? {} : {
    jumpEvery: 0,
    turn: 0.35,
    paused: true
  }));
}
const ACTIVE = /* @__PURE__ */ new Set(["PENDING", "RUNNING"]);
const FAILED = /* @__PURE__ */ new Set(["FAILED", "CANCELLED", "TIMED_OUT", "ORPHANED"]);
const STATUS_LABELS = {
  PENDING: "Queued",
  RUNNING: "Running",
  COMPLETED: "Done",
  FAILED: "Failed",
  CANCELLED: "Cancelled",
  TIMED_OUT: "Timed out",
  ORPHANED: "Interrupted"
};
function runLink(id) {
  return `dispatch://run/${encodeURIComponent(id)}`;
}
function firstLine(text, max) {
  const line = String(text || "").split("\n").find((part) => part.trim()) || "";
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}
function statusLabel(run) {
  if (run.mergedAt) return "Merged";
  return STATUS_LABELS[run.status] || run.status || "Unknown";
}
function formatTokens(tokens) {
  if (typeof tokens !== "number" || tokens <= 0) return "";
  return tokens >= 1e3 ? `${(tokens / 1e3).toFixed(tokens >= 1e5 ? 0 : 1)}k tokens` : `${tokens} tokens`;
}
function formatDuration(ms) {
  if (!ms) return "";
  const seconds = Math.round(ms / 1e3);
  return seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
}
function runDetail(run) {
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
var _tmpl$ = /* @__PURE__ */ web.template(`<div class=dispatch-settings><p class=dispatch-muted></p><div>`), _tmpl$2 = /* @__PURE__ */ web.template(`<p class=dispatch-danger>`), _tmpl$3 = /* @__PURE__ */ web.template(`<pre class=dispatch-log-body>`), _tmpl$4 = /* @__PURE__ */ web.template(`<button class=dispatch-log-follow type=button>`), _tmpl$5 = /* @__PURE__ */ web.template(`<div class=dispatch-log-foot><span class=dispatch-muted>`), _tmpl$6 = /* @__PURE__ */ web.template(`<div class="dispatch-panel dispatch-panel-log"><div class=dispatch-log-head><span class=dispatch-run-bot></span><span class=dispatch-run-body><span class=dispatch-run-title></span><span></span></span></div><div class=dispatch-log-wrap>`), _tmpl$7 = /* @__PURE__ */ web.template(`<p class=dispatch-log-empty>`), _tmpl$8 = /* @__PURE__ */ web.template(`<span class=dispatch-muted> · `), _tmpl$9 = /* @__PURE__ */ web.template(`<ul class=dispatch-runs>`), _tmpl$0 = /* @__PURE__ */ web.template(`<div class=dispatch-panel><div class=dispatch-panel-head><span class=dispatch-panel-switch></span><span class=dispatch-panel-title></span><span class=dispatch-panel-actions>`), _tmpl$1 = /* @__PURE__ */ web.template(`<div class=dispatch-empty><p class=dispatch-empty-title></p><p class=dispatch-muted>`), _tmpl$10 = /* @__PURE__ */ web.template(`<li><button class=dispatch-run type=button><span class=dispatch-run-bot></span><span class=dispatch-run-body><span class=dispatch-run-title></span><span>`);
const POLL_ACTIVE_MS = 3e3;
const POLL_IDLE_MS = 15e3;
const LOG_POLL_MS = 2e3;
const FOLLOW_SLACK = 48;
const CSS = `
.dispatch-panel { display: flex; flex-direction: column; gap: 4px; padding: 8px; overflow: auto; height: 100%; box-sizing: border-box; }
.dispatch-panel-log { overflow: hidden; }
.dispatch-log-head { display: flex; align-items: center; gap: 8px; padding: 4px 4px 8px; }
.dispatch-log-head .dispatch-run-body { flex: 1; }
.dispatch-log-wrap { display: flex; flex-direction: column; flex: 1; min-height: 0; gap: 6px; }
.dispatch-log-body { flex: 1; min-height: 0; margin: 0; padding: 8px; overflow: auto; border-radius: 8px; background: var(--surface-base-hover); color: var(--text-base); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
.dispatch-log-empty { flex: 1; display: grid; place-items: center; margin: 0; color: var(--text-weak); font-size: 12px; }
.dispatch-log-foot { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 0 4px; }
.dispatch-log-follow { border: 0; border-radius: 999px; padding: 4px 10px; background: var(--surface-base-hover); color: var(--text-base); font: inherit; font-size: 12px; cursor: pointer; }
.dispatch-log-follow:hover, .dispatch-log-follow:focus-visible { background: var(--surface-base-hover); color: var(--text-strong); outline: none; }
.dispatch-panel-head { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: 8px; padding: 4px 4px 8px 8px; }
.dispatch-panel-switch { display: flex; align-items: center; justify-self: start; }
.dispatch-panel-title { color: var(--text-strong); font-weight: 500; text-align: center; white-space: nowrap; }
.dispatch-panel-actions { display: flex; align-items: center; justify-self: end; }
.dispatch-runs { display: flex; flex-direction: column; gap: 2px; list-style: none; margin: 0; padding: 0; }
.dispatch-run { display: flex; gap: 10px; align-items: center; width: 100%; padding: 8px; border: 0; border-radius: 8px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.dispatch-run:hover, .dispatch-run:focus-visible { background: var(--surface-base-hover); outline: none; }
.dispatch-run-bot { flex: none; display: grid; place-items: center; width: 28px; height: 28px; }
.dispatch-run-body { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.dispatch-run-title { overflow: hidden; color: var(--text-base); text-overflow: ellipsis; white-space: nowrap; }
.dispatch-muted { color: var(--text-weak); font-size: 12px; }
.dispatch-danger { color: var(--text-error); font-size: 12px; }
.dispatch-empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; padding: 24px; text-align: center; }
.dispatch-empty-title { margin-top: 8px; color: var(--text-strong); font-size: 15px; font-weight: 600; }
.dispatch-empty .dispatch-muted { max-width: 280px; line-height: 1.45; }
.dispatch-settings { display: flex; flex-direction: column; gap: 16px; }
`;
const definition = gui.Plugin.define({
  i18n: {
    en: {
      title: "Dispatch",
      "switch.label": "Delegate through Dispatch",
      "switch.description": "Replaces OpenCode's subagents with Dispatch runs: each task goes to the agent and model Dispatch routes it to, in its own worktree, and lists under this conversation in the Dispatch app.",
      "switch.on": "Dispatch delegation is on",
      "switch.off": "Dispatch delegation is off",
      "binary.missing": "The dispatch command was not found at {{binary}}. Install Dispatch, then run dispatch setup opencode.",
      "binary.found": "Runs with {{binary}}",
      "command.toggle.on": "Turn Dispatch delegation off",
      "command.toggle.off": "Turn Dispatch delegation on",
      "command.panel": "Show Dispatch runs",
      "command.app": "Open Dispatch",
      "panel.title": "Dispatch",
      "panel.runs": "Runs",
      "panel.empty.title": "No runs yet",
      "panel.empty.on": "When the agent hands work to another agent, the run shows up here.",
      "panel.empty.off.title": "Delegation is off",
      "panel.empty.off": "This conversation uses OpenCode's own subagents. Turn Dispatch on with the switch above.",
      "panel.unreachable": "Dispatch did not answer: {{error}}",
      "panel.back": "Back to runs",
      "log.empty": "Waiting for output…",
      "log.none": "This run left no output.",
      "log.failed": "Could not read the run: {{error}}",
      "log.following": "Following the end",
      "log.paused": "Not following new lines",
      "log.follow": "Follow",
      "log.open": "Open in Dispatch",
      "pill.on": "Dispatch",
      "pill.off": "Dispatch off",
      // The check beside the runs pill: the label stays Delegate, and the check
      // and the success tone say whether this conversation must delegate.
      "pill.force.on": "Delegate",
      "pill.force.off": "Delegate",
      "pill.force.title": "Force delegation: this conversation must hand its work to Dispatch",
      // The pill beside it, while the check is on: the agent and model each
      // forced run goes to, or Auto.
      "pill.target.auto": "Auto",
      "pill.target.auto.row": "Auto (Dispatch decides)",
      "pill.target.title": "Agent and model every forced run goes to",
      "pill.target.loading": "Loading agents…",
      "pill.target.empty": "No agents to pick",
      "pill.target.failed": "Could not read the agents: {{error}}",
      "runs.one": "{{count}} run",
      "runs.other": "{{count}} runs",
      "runs.running": "{{count}} running",
      "settings.open": "Open Dispatch",
      "open.failed": "Could not open Dispatch"
    }
  },
  setup(ctx) {
    const rpc = () => ctx.client.rpc(Dispatch);
    const [status, setStatus] = solidJs.createSignal({
      enabled: true,
      binary: "",
      available: true,
      loaded: false
    });
    const [runs, setRuns] = store.createStore({});
    const [errors, setErrors] = store.createStore({});
    const [forced, setForced] = store.createStore({});
    const [targets, setTargets] = solidJs.createSignal(void 0);
    const [targetsError, setTargetsError] = solidJs.createSignal("");
    let targetsLoading = false;
    let targetsTried = 0;
    const loadState = async () => {
      try {
        setStatus({
          ...await rpc().state({}),
          loaded: true
        });
      } catch {
        setStatus((prev) => ({
          ...prev,
          loaded: true
        }));
      }
    };
    const setEnabled = async (enabled) => {
      try {
        setStatus({
          ...await rpc().setEnabled({
            enabled
          }),
          loaded: true
        });
        ctx.toast.show({
          variant: "success",
          title: ctx.t(enabled ? "switch.on" : "switch.off")
        });
      } catch (error) {
        ctx.toast.show({
          variant: "error",
          title: ctx.t("title"),
          description: String(error?.message || error)
        });
      }
    };
    const toggle = () => setEnabled(!status().enabled);
    const forcedView = (answer) => ({
      force: answer.force === true,
      agent: answer.agent || "",
      model: answer.model || ""
    });
    const ensureTargets = async () => {
      if (targets() !== void 0 || targetsLoading) return;
      if (Date.now() - targetsTried < 15e3) return;
      targetsTried = Date.now();
      targetsLoading = true;
      try {
        const answer = await rpc().targets({});
        setTargets(Array.isArray(answer.agents) ? answer.agents : []);
        setTargetsError(answer.error || "");
      } catch (error) {
        setTargetsError(String(error?.message || error));
      } finally {
        targetsLoading = false;
      }
    };
    const refresh = async (session) => {
      if (!session || session.pending) return;
      try {
        const [answer, flag] = await Promise.all([rpc().runs({
          sessionID: session.id,
          directory: session.directory || ""
        }), rpc().force({
          sessionID: session.id,
          directory: session.directory || ""
        }).catch(() => void 0)]);
        setRuns(session.key, store.reconcile(answer.runs || [], {
          key: "id"
        }));
        setErrors(session.key, answer.error || "");
        if (flag) {
          const view = forcedView(flag);
          setForced(session.key, view);
          if (view.force) void ensureTargets();
        }
      } catch (error) {
        setErrors(session.key, String(error?.message || error));
      }
    };
    const toggleForce = async (session) => {
      const force = forced[session.key]?.force !== true;
      try {
        const answer = await rpc().setForce({
          sessionID: session.id,
          directory: session.directory || "",
          force
        });
        setForced(session.key, forcedView(answer));
        if (answer.force === true) void ensureTargets();
      } catch (error) {
        ctx.toast.show({
          variant: "error",
          title: ctx.t("title"),
          description: String(error?.message || error)
        });
      }
    };
    const pickTarget = async (session, agent, model) => {
      const force = forced[session.key]?.force === true;
      try {
        const answer = await rpc().setForce({
          sessionID: session.id,
          directory: session.directory || "",
          force,
          agent,
          model
        });
        setForced(session.key, forcedView(answer));
      } catch (error) {
        ctx.toast.show({
          variant: "error",
          title: ctx.t("title"),
          description: String(error?.message || error)
        });
      }
    };
    const sessionRuns = (session) => session ? runs[session.key] || [] : [];
    const openLink = async (url) => {
      try {
        const answer = await rpc().open({
          url
        });
        if (!answer.opened) throw new Error(answer.error || "Dispatch did not open");
      } catch (error) {
        ctx.toast.show({
          variant: "error",
          title: ctx.t("open.failed"),
          description: String(error?.message || error)
        });
      }
    };
    const openRun = (run) => void openLink(runLink(run.id));
    const openApp = () => void openLink("dispatch://");
    void loadState();
    let timer;
    let polling = false;
    let disposed = false;
    const tick = async () => {
      clearTimeout(timer);
      timer = void 0;
      if (polling || disposed) return;
      polling = true;
      try {
        const session = ctx.sessions.current();
        if (document.visibilityState === "visible") {
          await loadState();
          if (session) await refresh(session);
        }
      } finally {
        polling = false;
      }
      if (disposed) return;
      const active = sessionRuns(ctx.sessions.current()).some((run) => ACTIVE.has(run.status));
      timer = setTimeout(tick, active ? POLL_ACTIVE_MS : POLL_IDLE_MS);
    };
    timer = setTimeout(tick, 500);
    let offChanged = () => {
    };
    let offStarted = () => {
    };
    let offForced = () => {
    };
    try {
      offChanged = rpc().events.on("changed", (event) => setStatus((prev) => ({
        ...prev,
        enabled: event.data.enabled
      })));
      offStarted = rpc().events.on("started", () => void tick());
      offForced = rpc().events.on("forced", () => void tick());
    } catch {
    }
    ctx.add(gui.Style, CSS);
    ctx.add(gui.SettingsPage, {
      id: "dispatch",
      icon: "fork",
      get title() {
        return ctx.t("title");
      },
      get entries() {
        return [{
          id: "delegation",
          title: ctx.t("switch.label"),
          keywords: "dispatch subagent delegate route"
        }];
      },
      render: () => (() => {
        var _el$ = _tmpl$(), _el$2 = _el$.firstChild, _el$3 = _el$2.nextSibling;
        web.insert(_el$, web.createComponent(ctx.ui.Switch, {
          get checked() {
            return status().enabled;
          },
          onChange: (checked) => void setEnabled(checked),
          get label() {
            return ctx.t("switch.label");
          },
          get description() {
            return ctx.t("switch.description");
          }
        }), _el$2);
        web.insert(_el$2, (() => {
          var _c$ = web.memo(() => !!status().available);
          return () => _c$() ? ctx.t("binary.found", {
            binary: status().binary
          }) : ctx.t("binary.missing", {
            binary: status().binary || "dispatch"
          });
        })());
        web.insert(_el$3, web.createComponent(ctx.ui.Button, {
          variant: "neutral",
          icon: "square-arrow-top-right",
          onClick: openApp,
          get children() {
            return ctx.t("settings.open");
          }
        }));
        return _el$;
      })()
    });
    ctx.add(gui.Command, () => ({
      id: "toggle",
      title: ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off"),
      keywords: "dispatch subagent delegate",
      slash: {
        name: "dispatch"
      },
      run: () => toggle()
    }));
    ctx.add(gui.Command, () => ({
      id: "panel",
      title: ctx.t("command.panel"),
      keywords: "dispatch runs subagents",
      enabled: !!ctx.sessions.current(),
      run: () => {
        const session = ctx.sessions.current();
        if (session) ctx.layout.open(`${ctx.id}:runs`, session);
      }
    }));
    ctx.add(gui.Command, () => ({
      id: "app",
      title: ctx.t("command.app"),
      keywords: "dispatch",
      run: openApp
    }));
    ctx.add(gui.MenuItem, () => {
      const session = ctx.sessions.current();
      const count = sessionRuns(session).length;
      return {
        menu: "session.panel",
        id: "runs",
        title: ctx.t("panel.title"),
        icon: "fork",
        keybind: `${ctx.id}.panel`,
        description: count ? ctx.plural("runs", count) : void 0,
        opened: !!session && ctx.layout.state(`${ctx.id}:runs`, session) !== "closed",
        run: () => {
          if (session) ctx.layout.open(`${ctx.id}:runs`, session);
        }
      };
    });
    const openPanel = (session) => {
      if (session) ctx.layout.open(`${ctx.id}:runs`, session);
    };
    ctx.add(gui.ComposerPill, {
      id: "runs",
      // Always there: a click opens the runs panel, whose header holds the switch.
      show: (input) => {
        const list = sessionRuns(input.session);
        const on = status().enabled;
        const running = list.filter((run) => ACTIVE.has(run.status)).length;
        let label = ctx.t(on ? "pill.on" : "pill.off");
        if (running) label = `${label} · ${ctx.t("runs.running", {
          count: running
        })}`;
        else if (list.length) label = `${label} · ${ctx.plural("runs", list.length)}`;
        return {
          icon: running ? "status-active" : on ? "fork" : "circle-ban-sign",
          label,
          tone: on ? "success" : void 0,
          title: ctx.t("command.panel"),
          run: () => openPanel(input.session)
        };
      }
    });
    ctx.add(gui.ComposerPill, {
      id: "force",
      // The Delegate check beside it: ticked, the conversation must hand its
      // work to Dispatch. It goes with delegation, so it is hidden while no
      // dispatch_run can take the work.
      show: (input) => {
        if (!status().enabled) return void 0;
        const on = forced[input.session.key]?.force === true;
        return {
          icon: on ? "circle-check" : void 0,
          label: ctx.t(on ? "pill.force.on" : "pill.force.off"),
          tone: on ? "success" : void 0,
          title: ctx.t("pill.force.title"),
          run: () => void toggleForce(input.session)
        };
      }
    });
    ctx.add(gui.ComposerPill, {
      id: "target",
      // The agent and model every forced run goes to, beside the Delegate
      // check while it is on: Auto, or a model of one of the agents here. The
      // menu is read from the CLIs the first time a conversation needs it.
      show: (input) => {
        if (!status().enabled || forced[input.session.key]?.force !== true) return void 0;
        void ensureTargets();
        const pick = forced[input.session.key];
        const chosen = (agent, model) => pick?.agent === agent && (pick?.model || "") === model;
        const rows = [{
          id: "target:auto",
          label: ctx.t("pill.target.auto.row"),
          ...chosen("", "") ? {
            icon: "circle-check",
            tone: "success"
          } : {},
          run: () => void pickTarget(input.session, "", "")
        }];
        const list = targets();
        if (!list) {
          rows.push({
            id: "target:loading",
            label: targetsError() ? ctx.t("pill.target.failed", {
              error: targetsError()
            }) : ctx.t("pill.target.loading")
          });
        } else {
          for (const agent of list) {
            for (const model of agent.models) {
              const on = chosen(agent.name, model);
              rows.push({
                id: `target:${agent.name}:${model}`,
                label: `${agent.name} · ${model}`,
                ...on ? {
                  icon: "circle-check",
                  tone: "success"
                } : {},
                run: () => void pickTarget(input.session, agent.name, model)
              });
            }
          }
          if (!list.length) rows.push({
            id: "target:none",
            label: ctx.t("pill.target.empty")
          });
        }
        return {
          icon: "models",
          label: pick?.agent ? pick.model ? `${pick.agent} · ${pick.model}` : pick.agent : ctx.t("pill.target.auto"),
          title: ctx.t("pill.target.title"),
          rows
        };
      }
    });
    const tab = {
      id: "runs",
      get title() {
        return ctx.t("panel.title");
      },
      closable: "hover"
    };
    const RunLog = (props) => {
      const [log, setLog] = solidJs.createSignal(void 0);
      const [failure, setFailure] = solidJs.createSignal("");
      const [following, setFollowing] = solidJs.createSignal(true);
      let area;
      let timer2;
      let stopped = false;
      const status2 = () => log()?.status || props.run.status;
      const working = () => ACTIVE.has(status2());
      const atEnd = () => {
        const el = area;
        return !el || el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_SLACK;
      };
      const toEnd = () => {
        const el = area;
        if (el) el.scrollTop = el.scrollHeight;
        setFollowing(true);
      };
      const load = async () => {
        try {
          const answer = await rpc().log({
            runID: props.run.id,
            directory: props.session.directory || ""
          });
          if (stopped) return false;
          setLog(answer);
          setFailure(answer.error || "");
          if (following()) toEnd();
          return !answer.error && ACTIVE.has(answer.status);
        } catch (error) {
          if (stopped) return false;
          setFailure(String(error?.message || error));
          return false;
        }
      };
      const tick2 = async () => {
        timer2 = void 0;
        if (stopped || !await load()) return;
        timer2 = setTimeout(tick2, LOG_POLL_MS);
      };
      solidJs.onMount(() => void tick2());
      solidJs.onCleanup(() => {
        stopped = true;
        clearTimeout(timer2);
      });
      return (() => {
        var _el$4 = _tmpl$6(), _el$5 = _el$4.firstChild, _el$6 = _el$5.firstChild, _el$7 = _el$6.nextSibling, _el$8 = _el$7.firstChild, _el$9 = _el$8.nextSibling, _el$10 = _el$5.nextSibling;
        web.insert(_el$5, web.createComponent(ctx.ui.Tooltip, {
          get value() {
            return ctx.t("panel.back");
          },
          get children() {
            return web.createComponent(ctx.ui.IconButton, {
              icon: "arrow-left",
              variant: "ghost",
              size: "small",
              get label() {
                return ctx.t("panel.back");
              },
              get onClick() {
                return props.onBack;
              }
            });
          }
        }), _el$6);
        web.insert(_el$6, web.createComponent(AgentBot, {
          get agent() {
            return props.run.agent;
          },
          get seed() {
            return props.run.id;
          },
          get status() {
            return status2();
          }
        }));
        web.insert(_el$8, () => firstLine(props.run.prompt, 120) || props.run.id);
        web.insert(_el$9, () => runDetail({
          ...props.run,
          status: status2()
        }));
        web.insert(_el$5, web.createComponent(ctx.ui.Tooltip, {
          get value() {
            return ctx.t("log.open");
          },
          get children() {
            return web.createComponent(ctx.ui.IconButton, {
              icon: "square-arrow-top-right",
              variant: "ghost",
              size: "small",
              get label() {
                return ctx.t("log.open");
              },
              onClick: () => openRun(props.run)
            });
          }
        }), null);
        web.insert(_el$4, web.createComponent(solidJs.Show, {
          get when() {
            return props.run.error;
          },
          get children() {
            var _el$0 = _tmpl$2();
            web.insert(_el$0, () => props.run.error);
            return _el$0;
          }
        }), _el$10);
        web.insert(_el$4, web.createComponent(solidJs.Show, {
          get when() {
            return failure();
          },
          get children() {
            var _el$1 = _tmpl$2();
            web.insert(_el$1, () => ctx.t("log.failed", {
              error: failure()
            }));
            return _el$1;
          }
        }), _el$10);
        web.insert(_el$10, web.createComponent(solidJs.Show, {
          get when() {
            return log()?.text;
          },
          get fallback() {
            return (() => {
              var _el$15 = _tmpl$7();
              web.insert(_el$15, () => ctx.t(working() ? "log.empty" : "log.none"));
              return _el$15;
            })();
          },
          get children() {
            var _el$11 = _tmpl$3();
            _el$11.addEventListener("scroll", () => setFollowing(atEnd()));
            web.use((el) => area = el, _el$11);
            web.insert(_el$11, () => log()?.text);
            return _el$11;
          }
        }), null);
        web.insert(_el$10, web.createComponent(solidJs.Show, {
          get when() {
            return working() || !following();
          },
          get children() {
            var _el$12 = _tmpl$5(), _el$13 = _el$12.firstChild;
            web.insert(_el$13, () => ctx.t(following() ? "log.following" : "log.paused"));
            web.insert(_el$12, web.createComponent(solidJs.Show, {
              get when() {
                return !following();
              },
              get children() {
                var _el$14 = _tmpl$4();
                _el$14.$$click = toEnd;
                web.insert(_el$14, () => ctx.t("log.follow"));
                return _el$14;
              }
            }), null);
            return _el$12;
          }
        }), null);
        web.effect(() => web.className(_el$9, FAILED.has(status2()) ? "dispatch-danger" : "dispatch-muted"));
        return _el$4;
      })();
    };
    const RunsView = (props) => {
      const [selected, setSelected] = solidJs.createSignal(void 0);
      const list = () => sessionRuns(props.session);
      const error = () => props.session ? errors[props.session.key] : "";
      const selectedRun = () => {
        const open = selected();
        if (!open) return void 0;
        return list().find((run) => run.id === open.id) || open;
      };
      return web.createComponent(solidJs.Show, {
        get when() {
          return selectedRun();
        },
        get fallback() {
          return (() => {
            var _el$16 = _tmpl$0(), _el$17 = _el$16.firstChild, _el$18 = _el$17.firstChild, _el$19 = _el$18.nextSibling, _el$22 = _el$19.nextSibling;
            web.insert(_el$18, web.createComponent(ctx.ui.Tooltip, {
              get value() {
                return ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off");
              },
              get children() {
                return web.createComponent(ctx.ui.Switch, {
                  get checked() {
                    return status().enabled;
                  },
                  onChange: (checked) => void setEnabled(checked),
                  get ["aria-label"]() {
                    return ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off");
                  }
                });
              }
            }));
            web.insert(_el$19, () => ctx.t("panel.runs"), null);
            web.insert(_el$19, web.createComponent(solidJs.Show, {
              get when() {
                return list().length > 0;
              },
              get children() {
                var _el$20 = _tmpl$8();
                _el$20.firstChild;
                web.insert(_el$20, () => list().length, null);
                return _el$20;
              }
            }), null);
            web.insert(_el$22, web.createComponent(ctx.ui.Tooltip, {
              get value() {
                return ctx.t("command.app");
              },
              get children() {
                return web.createComponent(ctx.ui.IconButton, {
                  icon: "square-arrow-top-right",
                  variant: "ghost",
                  size: "small",
                  get label() {
                    return ctx.t("command.app");
                  },
                  onClick: openApp
                });
              }
            }));
            web.insert(_el$16, web.createComponent(solidJs.Show, {
              get when() {
                return error();
              },
              get children() {
                var _el$23 = _tmpl$2();
                web.insert(_el$23, () => ctx.t("panel.unreachable", {
                  error: error()
                }));
                return _el$23;
              }
            }), null);
            web.insert(_el$16, web.createComponent(solidJs.Show, {
              get when() {
                return list().length > 0;
              },
              get fallback() {
                return (() => {
                  var _el$25 = _tmpl$1(), _el$26 = _el$25.firstChild, _el$27 = _el$26.nextSibling;
                  web.insert(_el$25, web.createComponent(EmptyBot, {
                    get mood() {
                      return status().enabled ? "sleeping" : "lost";
                    }
                  }), _el$26);
                  web.insert(_el$26, () => ctx.t(status().enabled ? "panel.empty.title" : "panel.empty.off.title"));
                  web.insert(_el$27, () => ctx.t(status().enabled ? "panel.empty.on" : "panel.empty.off"));
                  return _el$25;
                })();
              },
              get children() {
                var _el$24 = _tmpl$9();
                web.insert(_el$24, web.createComponent(solidJs.For, {
                  get each() {
                    return list();
                  },
                  children: (run) => (() => {
                    var _el$28 = _tmpl$10(), _el$29 = _el$28.firstChild, _el$30 = _el$29.firstChild, _el$31 = _el$30.nextSibling, _el$32 = _el$31.firstChild, _el$33 = _el$32.nextSibling;
                    _el$29.$$click = () => setSelected(run);
                    web.insert(_el$30, web.createComponent(AgentBot, {
                      get agent() {
                        return run.agent;
                      },
                      get seed() {
                        return run.id;
                      },
                      get status() {
                        return run.status;
                      }
                    }));
                    web.insert(_el$32, () => firstLine(run.prompt, 120) || run.id);
                    web.insert(_el$33, () => runDetail(run));
                    web.effect((_p$) => {
                      var _v$ = run.prompt, _v$2 = FAILED.has(run.status) ? "dispatch-danger" : "dispatch-muted";
                      _v$ !== _p$.e && web.setAttribute(_el$29, "title", _p$.e = _v$);
                      _v$2 !== _p$.t && web.className(_el$33, _p$.t = _v$2);
                      return _p$;
                    }, {
                      e: void 0,
                      t: void 0
                    });
                    return _el$28;
                  })()
                }));
                return _el$24;
              }
            }), null);
            return _el$16;
          })();
        },
        children: (run) => web.createComponent(RunLog, {
          get run() {
            return run();
          },
          get session() {
            return props.session;
          },
          onBack: () => setSelected(void 0)
        })
      });
    };
    ctx.add(gui.Panel, {
      id: "runs",
      region: "side",
      mobile: {
        get title() {
          return ctx.t("panel.title");
        },
        order: 60,
        kind: "menu",
        icon: "fork"
      },
      list: (input) => input.open.includes("runs") ? [tab] : [],
      focus: (input) => void refresh(input.session),
      render: (props) => web.createComponent(RunsView, {
        get session() {
          return props.session;
        }
      })
    });
    return () => {
      disposed = true;
      clearTimeout(timer);
      offChanged();
      offStarted();
      offForced();
    };
  }
});
const helpers = {
  firstLine,
  statusLabel,
  runDetail,
  runLink,
  formatTokens,
  formatDuration
};
web.delegateEvents(["click"]);
exports.Dispatch = Dispatch;
exports.default = definition;
exports.helpers = helpers;
