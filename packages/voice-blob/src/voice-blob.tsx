import { createEffect, onCleanup, onMount } from "solid-js";
import type { JSX } from "solid-js";
import { AdaptiveNormalizer, blendParams, makeCurve, stateParams } from "./math";
import type { BlobLevels, BlobState, VisualParams } from "./math";
import "./voice-blob.css";

export interface VoiceBlobProps {
  state: () => BlobState;
  levels: BlobLevels;
  mode?: "expanded" | "compact";
  size?: number;
  reducedMotion?: boolean;
  theme?: "light" | "dark";
  class?: string;
  label?: string;
}

const labels: Record<BlobState, string> = {
  idle: "Voice idle", listening: "Listening", processing: "Processing voice",
  speaking: "Speaking", error: "Voice error", disconnected: "Voice disconnected",
};

const palette = {
  dark: { core: [157, 187, 202], edge: [112, 155, 180], secondary: [148, 163, 184], warm: [196, 143, 119] },
  light: { core: [95, 133, 153], edge: [73, 119, 147], secondary: [130, 151, 171], warm: [174, 118, 95] },
};

function mix(a: number[], b: number[], t: number): number[] { return a.map((v, i) => v + (b[i] - v) * t); }
function rgb(v: number[], alpha = 1): string { return `rgba(${v.map((n) => Math.round(n)).join(",")},${alpha})`; }

export function VoiceBlob(props: VoiceBlobProps): JSX.Element {
  let canvas!: HTMLCanvasElement;
  let host!: HTMLDivElement;
  const mode = () => props.mode ?? "expanded";
  const size = () => props.size ?? (mode() === "compact" ? 20 : 220);
  const isReduced = () => props.reducedMotion ?? window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const currentLabel = () => props.label ?? labels[props.state()];

  onMount(() => {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) return;
    const mic = new AdaptiveNormalizer();
    const out = new AdaptiveNormalizer();
    let visible = false;
    let raf = 0;
    let last = 0;
    let lastPaint = 0;
    let lastReducedLevel = 0;
    let settledAt = performance.now();
    let errorUntil = 0;
    let time = 0;
    let activeState: BlobState = props.state();
    let params = stateParams(activeState);
    let from: VisualParams = { ...params };
    let target: VisualParams = { ...params };
    let transitionStart = 0;
    let changed = false;
    let theme = props.theme ?? "dark";
    let pixelRatio = 1;
    let side = size();
    const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");

    const resize = () => {
      side = Math.max(1, size());
      pixelRatio = Math.min(2, window.devicePixelRatio || 1);
      canvas.width = Math.round(side * pixelRatio);
      canvas.height = Math.round(side * pixelRatio);
      canvas.style.width = `${side}px`;
      canvas.style.height = `${side}px`;
      ctx.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
      changed = true;
      schedule();
    };

    const paint = (now: number, dt: number) => {
      const compact = mode() === "compact";
      const reduced = isReduced();
      const state = props.state();
      if (state !== activeState) {
        activeState = state;
        from = { ...params };
        target = stateParams(state);
        transitionStart = now;
        changed = true;
        if (state === "error") errorUntil = now + 1350;
        if (state === "idle") settledAt = now;
      }
      if ((props.theme ?? "dark") !== theme) { theme = props.theme ?? "dark"; changed = true; }
      const nextParams = blendParams(from, target, Math.min(1, (now - transitionStart) / 380));
      params = nextParams;
      const input = state === "speaking" ? props.levels.out : state === "listening" ? props.levels.mic : 0;
      const smoothLevel = state === "speaking" ? out.update(props.levels.out, dt) : mic.update(props.levels.mic, dt);
      const level = reduced ? (now - lastReducedLevel >= 166 ? (lastReducedLevel = now, smoothLevel) : 0) : smoothLevel;
      const amplitude = params.amplitude + level * (state === "speaking" || state === "listening" ? (reduced ? 0.035 : 0.48) : 0);
      if (input > 0.00001) settledAt = now;
      const p = palette[theme];
      const core = mix(p.core, p.warm, params.warmth);
      const edge = mix(p.edge, p.warm, params.warmth);
      const alt = mix(p.secondary, p.warm, params.warmth * 0.7);
      const alpha = 1 - params.dim;
      const radius = compact ? side * 0.42 : side * 0.39;
      ctx.clearRect(0, 0, side, side);
      ctx.save();
      ctx.globalAlpha = alpha;
      const center = side / 2;
      const gradient = ctx.createRadialGradient(center * 0.75, center * 0.68, radius * 0.08, center, center, radius * 1.16);
      gradient.addColorStop(0, rgb(mix(core, [255, 255, 255], theme === "dark" ? 0.27 : 0.42), 0.97));
      gradient.addColorStop(0.52, rgb(core, 0.92));
      gradient.addColorStop(1, rgb(edge, 0.88));
      const curve = makeCurve({ time: reduced ? 0 : time, amplitude, processing: params.processing,
        error: now < errorUntil ? 1 : 0, points: compact ? 8 : 10, radius, seed: 29 });
      ctx.beginPath();
      ctx.moveTo(curve.start.x, curve.start.y);
      for (const s of curve.segments) ctx.bezierCurveTo(s.c1.x, s.c1.y, s.c2.x, s.c2.y, s.end.x, s.end.y);
      ctx.closePath();
      ctx.fillStyle = gradient;
      ctx.fill();
      if (!compact) {
        const sheen = ctx.createRadialGradient(center * 0.7, center * 0.58, 1, center * 0.8, center * 0.72, radius * 0.95);
        sheen.addColorStop(0, rgb([255, 255, 255], theme === "dark" ? 0.21 : 0.3));
        sheen.addColorStop(1, rgb(alt, 0));
        ctx.fillStyle = sheen;
        ctx.fill();
        ctx.beginPath();
        ctx.moveTo(center - radius * 0.55, center - radius * 0.32);
        ctx.quadraticCurveTo(center, center - radius * 0.72, center + radius * 0.39, center - radius * 0.39);
        ctx.strokeStyle = rgb([255, 255, 255], theme === "dark" ? 0.16 : 0.24);
        ctx.lineWidth = Math.max(1, side * 0.009);
        ctx.lineCap = "round";
        ctx.stroke();
        const ring = ctx.createRadialGradient(center, center, radius * 0.91, center, center, radius * 1.14);
        ring.addColorStop(0, rgb(edge, 0));
        ring.addColorStop(0.72, rgb(edge, 0.055));
        ring.addColorStop(1, rgb(edge, 0));
        ctx.fillStyle = ring;
        ctx.fillRect(0, 0, side, side);
      }
      ctx.restore();
      time += dt * 1000;
      changed = false;
    };

    const shouldAnimate = (now: number) => {
      const s = props.state();
      if (changed || now - transitionStart < 380) return true;
      if (isReduced()) {
        const activeLevel = (s === "listening" ? props.levels.mic : s === "speaking" ? props.levels.out : 0) > 0.00001;
        return (s === "listening" || s === "speaking") && activeLevel;
      }
      if (s === "listening" || s === "speaking" || s === "processing") return true;
      if (s === "error") return now < errorUntil;
      if (s === "idle") return now - settledAt < 4200;
      return changed;
    };
    const tick = (now: number) => {
      raf = 0;
      if (!visible) return;
      const s = props.state();
      const interval = isReduced() ? 166 : s === "idle" ? 1000 / 24 : 1000 / 60;
      if (now - lastPaint >= interval || changed) {
        const dt = last ? (now - last) / 1000 : interval / 1000;
        last = now;
        lastPaint = now;
        paint(now, dt);
      }
      if (shouldAnimate(now)) schedule();
    };
    function schedule() { if (visible && !raf) raf = requestAnimationFrame(tick); }
    const visibility = () => { visible = host.isConnected && document.visibilityState === "visible" && inView; if (visible) { last = 0; changed = true; schedule(); } else if (raf) { cancelAnimationFrame(raf); raf = 0; } };
    let inView = true;
    const observer = new IntersectionObserver((entries) => { inView = entries[0]?.isIntersecting ?? true; visibility(); });
    observer.observe(host);
    resize();
    visible = document.visibilityState === "visible";
    const onVisibility = () => visibility();
    document.addEventListener("visibilitychange", onVisibility);
    const onMotionChange = () => { changed = true; schedule(); };
    motionQuery.addEventListener("change", onMotionChange);
    window.addEventListener("resize", resize, { passive: true });

    createEffect(() => {
      props.state(); props.theme; props.mode; props.size; props.reducedMotion;
      changed = true;
      schedule();
    });
    visibility();
    onCleanup(() => {
      if (raf) cancelAnimationFrame(raf);
      observer.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      motionQuery.removeEventListener("change", onMotionChange);
      window.removeEventListener("resize", resize);
    });
  });

  return <div ref={host} class={`voice-blob voice-blob--${mode()} ${props.class ?? ""}`} style={{ width: `${size()}px`, height: `${size()}px` }} role="img" aria-label={currentLabel()} data-state={props.state()} data-theme={props.theme ?? "dark"}>
    <canvas ref={canvas} aria-hidden="true" />
    <span class="voice-blob__forced" aria-hidden="true" />
  </div>;
}
