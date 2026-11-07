import { createBlobMatrices, createIcosphere } from "./math";

export type Rgb = readonly [number, number, number];

export interface BlobFrame {
  /** Seconds of animation time (already scaled by the state's speed). */
  time: number;
  /** Displacement amount, 0..1. */
  amplitude: number;
  /** Uniform scale pulse, 0..1 (speech level). */
  pulse: number;
  /** Fade toward transparent, 0..1. */
  dim: number;
  body: Rgb;
  band: Rgb;
  accent: Rgb;
}

export interface BlobRenderer {
  render(frame: BlobFrame): void;
  resize(width: number, height: number, dpr: number): void;
  dispose(): void;
}

// Classic Perlin 3D noise (Stefan Gustavson, public domain), as in the R3F Blob reference.
const NOISE = /* glsl */ `
vec4 permute(vec4 x) { return mod(((x * 34.0) + 1.0) * x, 289.0); }
vec4 taylorInvSqrt(vec4 r) { return 1.79284291400159 - 0.85373472095314 * r; }
vec3 fade(vec3 t) { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }
float cnoise(vec3 P) {
  vec3 Pi0 = mod(floor(P), 289.0);
  vec3 Pi1 = mod(Pi0 + vec3(1.0), 289.0);
  vec3 Pf0 = fract(P);
  vec3 Pf1 = Pf0 - vec3(1.0);
  vec4 ix = vec4(Pi0.x, Pi1.x, Pi0.x, Pi1.x);
  vec4 iy = vec4(Pi0.yy, Pi1.yy);
  vec4 iz0 = Pi0.zzzz;
  vec4 iz1 = Pi1.zzzz;
  vec4 ixy = permute(permute(ix) + iy);
  vec4 ixy0 = permute(ixy + iz0);
  vec4 ixy1 = permute(ixy + iz1);
  vec4 gx0 = ixy0 / 7.0;
  vec4 gy0 = fract(floor(gx0) / 7.0) - 0.5;
  gx0 = fract(gx0);
  vec4 gz0 = vec4(0.5) - abs(gx0) - abs(gy0);
  vec4 sz0 = step(gz0, vec4(0.0));
  gx0 -= sz0 * (step(0.0, gx0) - 0.5);
  gy0 -= sz0 * (step(0.0, gy0) - 0.5);
  vec4 gx1 = ixy1 / 7.0;
  vec4 gy1 = fract(floor(gx1) / 7.0) - 0.5;
  gx1 = fract(gx1);
  vec4 gz1 = vec4(0.5) - abs(gx1) - abs(gy1);
  vec4 sz1 = step(gz1, vec4(0.0));
  gx1 -= sz1 * (step(0.0, gx1) - 0.5);
  gy1 -= sz1 * (step(0.0, gy1) - 0.5);
  vec3 g000 = vec3(gx0.x, gy0.x, gz0.x);
  vec3 g100 = vec3(gx0.y, gy0.y, gz0.y);
  vec3 g010 = vec3(gx0.z, gy0.z, gz0.z);
  vec3 g110 = vec3(gx0.w, gy0.w, gz0.w);
  vec3 g001 = vec3(gx1.x, gy1.x, gz1.x);
  vec3 g101 = vec3(gx1.y, gy1.y, gz1.y);
  vec3 g011 = vec3(gx1.z, gy1.z, gz1.z);
  vec3 g111 = vec3(gx1.w, gy1.w, gz1.w);
  vec4 norm0 = taylorInvSqrt(vec4(dot(g000, g000), dot(g010, g010), dot(g100, g100), dot(g110, g110)));
  g000 *= norm0.x; g010 *= norm0.y; g100 *= norm0.z; g110 *= norm0.w;
  vec4 norm1 = taylorInvSqrt(vec4(dot(g001, g001), dot(g011, g011), dot(g101, g101), dot(g111, g111)));
  g001 *= norm1.x; g011 *= norm1.y; g101 *= norm1.z; g111 *= norm1.w;
  float n000 = dot(g000, Pf0);
  float n100 = dot(g100, vec3(Pf1.x, Pf0.yz));
  float n010 = dot(g010, vec3(Pf0.x, Pf1.y, Pf0.z));
  float n110 = dot(g110, vec3(Pf1.xy, Pf0.z));
  float n001 = dot(g001, vec3(Pf0.xy, Pf1.z));
  float n101 = dot(g101, vec3(Pf1.x, Pf0.y, Pf1.z));
  float n011 = dot(g011, vec3(Pf0.x, Pf1.yz));
  float n111 = dot(g111, Pf1);
  vec3 f = fade(Pf0);
  vec4 n_z = mix(vec4(n000, n100, n010, n110), vec4(n001, n101, n011, n111), f.z);
  vec2 n_yz = mix(n_z.xy, n_z.zw, f.y);
  return 2.2 * mix(n_yz.x, n_yz.y, f.x);
}
`;

/** Base radius and the most the noise may push it; together they stay inside the unit frame. */
export const BASE_RADIUS = 0.7;
export const MAX_DISPLACEMENT = 0.2;

function shaders(webgl2: boolean) {
  const v = webgl2
    ? { header: "#version 300 es\n", attr: "in", out: "out", inp: "in", frag: "out vec4 outColor;", color: "outColor" }
    : { header: "", attr: "attribute", out: "varying", inp: "varying", frag: "", color: "gl_FragColor" };
  const vertex = /* glsl */ `${v.header}
precision highp float;
${v.attr} vec3 aPosition;
uniform mat4 uProjection;
uniform mat4 uView;
uniform float uTime;
uniform float uAmplitude;
uniform float uPulse;
${v.out} vec3 vNormal;
${v.out} vec3 vViewDir;
${v.out} float vDisplacement;
${v.out} float vLatitude;
${NOISE}
float field(vec3 p) { return cnoise(p * 1.35 + vec3(uTime)); }
// Soft-limited so the surface never plateaus into creases.
float shape(vec3 p) { float n = field(p) * 0.8; return n / (1.0 + 0.35 * abs(n)); }
vec3 displaced(vec3 p) {
  float n = shape(p);
  return p * (${BASE_RADIUS.toFixed(3)} + n * uAmplitude * ${MAX_DISPLACEMENT.toFixed(3)});
}
void main() {
  vec3 p = normalize(aPosition);
  // Recompute the normal of the displaced surface from two nearby points on the sphere.
  vec3 helper = abs(p.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
  vec3 tangent = normalize(cross(helper, p));
  vec3 bitangent = cross(p, tangent);
  const float e = 0.015;
  vec3 q0 = displaced(p);
  vec3 q1 = displaced(normalize(p + tangent * e));
  vec3 q2 = displaced(normalize(p + bitangent * e));
  vec3 n = normalize(cross(q1 - q0, q2 - q0));
  if (dot(n, p) < 0.0) n = -n;
  vDisplacement = shape(p);
  vLatitude = p.y;
  vec4 viewPosition = uView * vec4(q0 * (1.0 + uPulse * 0.035), 1.0);
  vNormal = mat3(uView) * n;
  vViewDir = -viewPosition.xyz;
  gl_Position = uProjection * viewPosition;
}`;
  const fragment = /* glsl */ `${v.header}
precision highp float;
${v.inp} vec3 vNormal;
${v.inp} vec3 vViewDir;
${v.inp} float vDisplacement;
${v.inp} float vLatitude;
uniform float uTime;
uniform float uAmplitude;
uniform float uDim;
uniform vec3 uBody;
uniform vec3 uBand;
uniform vec3 uAccent;
${v.frag}
void main() {
  vec3 N = normalize(vNormal);
  vec3 V = normalize(vViewDir);
  vec3 L = normalize(vec3(-0.45, 0.65, 0.8));
  // The reference's bands: displacement times a latitude wave, mixed toward a light colour.
  float distort = 1.6 * vDisplacement * (0.3 + uAmplitude) * sin(vLatitude * 10.0 + uTime * 1.5);
  vec3 base = mix(uBody, uBand, smoothstep(0.0, 1.0, clamp(distort, 0.0, 1.0)) * 0.85);
  float diffuse = 0.6 + 0.4 * max(dot(N, L), 0.0);
  float fresnel = pow(1.0 - max(dot(N, V), 0.0), 2.5);
  float specular = pow(max(dot(reflect(-L, N), V), 0.0), 48.0);
  vec3 color = base * diffuse + uAccent * fresnel * 0.85 + vec3(specular * 0.35);
  float alpha = 1.0 - uDim;
  ${v.color} = vec4(color * alpha, alpha);
}`;
  return { vertex, fragment };
}

type GL = WebGLRenderingContext | WebGL2RenderingContext;

function compile(gl: GL, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("voice-blob: cannot create shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader) ?? "unknown error";
    gl.deleteShader(shader);
    throw new Error(`voice-blob: shader compile failed: ${log}`);
  }
  return shader;
}

export function createBlobRenderer(canvas: HTMLCanvasElement, detail = 5): BlobRenderer | null {
  const options: WebGLContextAttributes = { alpha: true, premultipliedAlpha: true, antialias: true, depth: true, powerPreference: "low-power" };
  const gl = (canvas.getContext("webgl2", options) ?? canvas.getContext("webgl", options)) as GL | null;
  if (!gl) return null;
  const webgl2 = typeof WebGL2RenderingContext !== "undefined" && gl instanceof WebGL2RenderingContext;
  const { vertex, fragment } = shaders(webgl2);
  let vs: WebGLShader | undefined;
  let fs: WebGLShader | undefined;
  let program: WebGLProgram | null = null;
  try {
    vs = compile(gl, gl.VERTEX_SHADER, vertex);
    fs = compile(gl, gl.FRAGMENT_SHADER, fragment);
    program = gl.createProgram();
    if (!program) throw new Error("voice-blob: cannot create program");
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(`voice-blob: link failed: ${gl.getProgramInfoLog(program)}`);
  } catch (error) {
    console.warn(error)
    if (vs) gl.deleteShader(vs);
    if (fs) gl.deleteShader(fs);
    if (program) gl.deleteProgram(program);
    return null;
  }

  const sphere = createIcosphere(detail);
  if (sphere.indices instanceof Uint32Array && !webgl2) gl.getExtension("OES_element_index_uint");
  const positions = gl.createBuffer();
  const indices = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, positions);
  gl.bufferData(gl.ARRAY_BUFFER, sphere.positions, gl.STATIC_DRAW);
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, indices);
  gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, sphere.indices, gl.STATIC_DRAW);
  const indexType = sphere.indices instanceof Uint32Array ? gl.UNSIGNED_INT : gl.UNSIGNED_SHORT;
  const position = gl.getAttribLocation(program, "aPosition");
  const u = (name: string) => gl.getUniformLocation(program!, name);
  const uniforms = {
    projection: u("uProjection"), view: u("uView"), time: u("uTime"), amplitude: u("uAmplitude"), pulse: u("uPulse"),
    dim: u("uDim"), body: u("uBody"), band: u("uBand"), accent: u("uAccent"),
  };

  gl.useProgram(program);
  gl.enable(gl.DEPTH_TEST);
  gl.enable(gl.CULL_FACE);
  gl.cullFace(gl.BACK);
  gl.disable(gl.BLEND);
  gl.clearColor(0, 0, 0, 0);

  return {
    resize(width, height, dpr) {
      canvas.width = Math.max(1, Math.round(width * dpr));
      canvas.height = Math.max(1, Math.round(height * dpr));
      gl.viewport(0, 0, canvas.width, canvas.height);
      const m = createBlobMatrices(width / Math.max(1, height));
      gl.useProgram(program);
      gl.uniformMatrix4fv(uniforms.projection, false, m.projection);
      gl.uniformMatrix4fv(uniforms.view, false, m.view);
    },
    render(frame) {
      gl.useProgram(program);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.bindBuffer(gl.ARRAY_BUFFER, positions);
      gl.enableVertexAttribArray(position);
      gl.vertexAttribPointer(position, 3, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, indices);
      gl.uniform1f(uniforms.time, frame.time);
      gl.uniform1f(uniforms.amplitude, frame.amplitude);
      gl.uniform1f(uniforms.pulse, frame.pulse);
      gl.uniform1f(uniforms.dim, frame.dim);
      gl.uniform3fv(uniforms.body, frame.body);
      gl.uniform3fv(uniforms.band, frame.band);
      gl.uniform3fv(uniforms.accent, frame.accent);
      gl.drawElements(gl.TRIANGLES, sphere.indices.length, indexType, 0);
    },
    dispose() {
      gl.deleteBuffer(positions);
      gl.deleteBuffer(indices);
      gl.deleteProgram(program);
      if (vs) gl.deleteShader(vs);
      if (fs) gl.deleteShader(fs);
    },
  };
}
