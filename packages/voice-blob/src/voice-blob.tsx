import { createEffect, onCleanup, onMount } from "solid-js";
import type { JSX } from "solid-js";
import { AdaptiveNormalizer, easeParams, makeCurve, stateParams } from "./math";
import type { BlobState, VisualParams } from "./math";
import { createBlobRenderer } from "./webgl";
import type { BlobRenderer } from "./webgl";
import "./voice-blob.css";

export interface VoiceBlobProps {
  state: () => BlobState;
  levels: { mic: number; out: number };
  mode?: "expanded" | "compact";
  size?: number;
  reducedMotion?: boolean;
  theme?: "light" | "dark";
  class?: string;
  label?: string;
}

const labels: Record<BlobState, string> = { idle: "Voice idle", listening: "Listening", processing: "Processing voice", speaking: "Speaking", error: "Voice error", disconnected: "Voice disconnected" };
const palette = {
  dark: { core: [6, 12, 21], edge: [65, 108, 132], secondary: [136, 177, 196], warm: [174, 78, 63] },
  light: { core: [190, 208, 216], edge: [83, 123, 145], secondary: [87, 127, 148], warm: [161, 74, 60] },
};
const mix = (a: number[], b: number[], t: number) => a.map((v, i) => v + (b[i] - v) * t);
const rgb = (v: number[], alpha = 1) => `rgba(${v.map((n) => Math.round(n)).join(",")},${alpha})`;

export function VoiceBlob(props: VoiceBlobProps): JSX.Element {
  let canvas!: HTMLCanvasElement;
  let fallback!: HTMLCanvasElement;
  let host!: HTMLDivElement;
  const mode = () => props.mode ?? "expanded";
  const size = () => props.size ?? (mode() === "compact" ? 20 : 220);
  const isReduced = () => props.reducedMotion ?? window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const currentLabel = () => props.label ?? labels[props.state()];

  onMount(() => {
    const ctx = fallback.getContext("2d", { alpha: true });
    if (!ctx) return;
    const mic = new AdaptiveNormalizer();
    const out = new AdaptiveNormalizer();
    let renderer: BlobRenderer | null = null;
    let visible = false, inView = true, raf = 0, last = 0, lastPaint = 0, lastReducedLevel = 0;
    let time = 0, level = 0, side = size(), dpr = 1, changed = true, theme = props.theme ?? "dark";
    let activeState = props.state(), params: VisualParams = stateParams(activeState), from = { ...params }, target = { ...params }, transitionStart = 0;
    const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
    const setRendererVisibility = () => {
      const glVisible = mode() !== "compact" && !!renderer;
      canvas.style.display = glVisible ? "block" : "none";
      fallback.style.display = glVisible ? "none" : "block";
    };
    const resize = () => {
      side = Math.max(1, size()); dpr = Math.min(2, window.devicePixelRatio || 1);
      for (const targetCanvas of [canvas, fallback]) {
        targetCanvas.width = Math.round(side * dpr); targetCanvas.height = Math.round(side * dpr);
        targetCanvas.style.width = `${side}px`; targetCanvas.style.height = `${side}px`;
      }
      renderer?.resize(side, side, dpr); changed = true; schedule();
    };
    const drawFallback = (now: number, reduced: boolean) => {
      const compact = mode() === "compact", p = palette[theme], c = mix(p.core, p.warm, params.warmth), edge = mix(p.edge, p.warm, params.warmth);
      const radius = side * (compact ? 0.42 : 0.39), center = side / 2;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, side, side); ctx.globalAlpha = 1 - params.dim;
      const grad = ctx.createRadialGradient(center * 0.73, center * 0.66, radius * 0.05, center, center, radius * 1.15);
      grad.addColorStop(0, rgb(mix(c, [255, 255, 255], theme === "dark" ? 0.25 : 0.38), 0.98)); grad.addColorStop(0.58, rgb(c, 0.94)); grad.addColorStop(1, rgb(edge, 0.88));
      const curve = makeCurve({ time: reduced ? 0 : time * 1000, amplitude: params.intensity + level * 0.5, processing: activeState === "processing" ? 1 : 0, error: activeState === "error" ? 1 : 0, points: compact ? 8 : 10, radius, seed: 29 });
      ctx.beginPath(); ctx.moveTo(curve.start.x, curve.start.y); for (const s of curve.segments) ctx.bezierCurveTo(s.c1.x, s.c1.y, s.c2.x, s.c2.y, s.end.x, s.end.y); ctx.closePath(); ctx.fillStyle = grad; ctx.fill();
      if (!compact) { const sheen = ctx.createRadialGradient(center * 0.7, center * 0.58, 1, center * 0.8, center * 0.72, radius); sheen.addColorStop(0, rgb([255,255,255], theme === "dark" ? 0.2 : 0.3)); sheen.addColorStop(1, rgb(edge, 0)); ctx.fillStyle = sheen; ctx.fill(); }
      ctx.globalAlpha = 1; void now;
    };
    const paint = (now: number, dt: number) => {
      const state = props.state();
      if (state !== activeState) { activeState = state; from = { ...params }; target = stateParams(state); transitionStart = now; changed = true; }
      const nextTheme = props.theme ?? "dark"; if (nextTheme !== theme) { theme = nextTheme; changed = true; }
      const t = Math.min(1, Math.max(0, (now - transitionStart) / 250)); params = easeParams(from, target, now - transitionStart, 250);
      const raw = state === "listening" ? props.levels.mic : state === "speaking" ? props.levels.out : 0;
      const normalized = state === "listening" ? mic.update(props.levels.mic, dt) : state === "speaking" ? out.update(props.levels.out, dt) : 0;
      if (isReduced()) level = now - lastReducedLevel >= 166 ? (lastReducedLevel = now, normalized) : level;
      else level = normalized;
      if (state === "listening") params.intensity += level * (isReduced() ? 0.035 : 0.55);
      if (state === "speaking") params.intensity += level * (isReduced() ? 0.035 : 0.6);
      if (renderer && mode() !== "compact") renderer.render({ time, intensity: params.intensity, warmth: params.warmth, dim: params.dim, level: state === "speaking" ? level : 0, theme });
      else drawFallback(now, isReduced());
      time += dt * params.speed; changed = false;
      return { raw, transitioning: t < 1 };
    };
    const shouldAnimate = (now: number, raw: number, transitioning: boolean) => {
      if (changed || transitioning) return true;
      if (isReduced()) return (activeState === "listening" || activeState === "speaking") && raw > 0.00001;
      return activeState === "idle" || activeState === "disconnected" || activeState === "processing" || activeState === "listening" || activeState === "speaking";
    };
    const tick = (now: number) => {
      raf = 0; if (!visible) return;
      const state = props.state(), reduced = isReduced(); const interval = reduced ? 1000 / 6 : state === "idle" || state === "disconnected" ? 1000 / 30 : 1000 / 60;
      let result = { raw: 0, transitioning: false };
      if (now - lastPaint >= interval || changed) { const dt = last ? (now - last) / 1000 : interval / 1000; last = lastPaint = now; result = paint(now, dt); }
      if (shouldAnimate(now, result.raw, result.transitioning)) schedule();
    };
    function schedule() { if (visible && !raf) raf = requestAnimationFrame(tick); }
    const visibility = () => { visible = host.isConnected && document.visibilityState === "visible" && inView; if (visible) { last = 0; changed = true; schedule(); } else if (raf) { cancelAnimationFrame(raf); raf = 0; } };
    const observer = new IntersectionObserver((entries) => { inView = entries[0]?.isIntersecting ?? true; visibility(); }); observer.observe(host);
    if (mode() !== "compact") renderer = createBlobRenderer(canvas);
    setRendererVisibility(); resize();
    const onLost = (event: Event) => { event.preventDefault(); renderer?.dispose(); renderer = null; setRendererVisibility(); changed = true; schedule(); };
    const onRestored = () => { renderer = createBlobRenderer(canvas); setRendererVisibility(); resize(); changed = true; schedule(); };
    canvas.addEventListener("webglcontextlost", onLost); canvas.addEventListener("webglcontextrestored", onRestored);
    const onVisibility = () => visibility(); document.addEventListener("visibilitychange", onVisibility);
    const onMotionChange = () => { changed = true; schedule(); }; motionQuery.addEventListener("change", onMotionChange); window.addEventListener("resize", resize, { passive: true });
    createEffect(() => { props.state(); props.theme; props.mode; props.size; props.reducedMotion; changed = true; schedule(); });
    visibility();
    onCleanup(() => { if (raf) cancelAnimationFrame(raf); observer.disconnect(); document.removeEventListener("visibilitychange", onVisibility); motionQuery.removeEventListener("change", onMotionChange); window.removeEventListener("resize", resize); canvas.removeEventListener("webglcontextlost", onLost); canvas.removeEventListener("webglcontextrestored", onRestored); renderer?.dispose(); const lose = canvas.getContext("webgl2")?.getExtension("WEBGL_lose_context") ?? canvas.getContext("webgl")?.getExtension("WEBGL_lose_context"); lose?.loseContext(); });
  });

  return <div ref={host} class={`voice-blob voice-blob--${mode()} ${props.class ?? ""}`} style={{ width: `${size()}px`, height: `${size()}px` }} role="img" aria-label={currentLabel()} data-state={props.state()} data-theme={props.theme ?? "dark"}>
    <canvas ref={canvas} aria-hidden="true" />
    <canvas ref={fallback} aria-hidden="true" />
    <span class="voice-blob__forced" aria-hidden="true" />
  </div>;
}
