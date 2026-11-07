import { createEffect, onCleanup, onMount } from "solid-js";
import type { JSX } from "solid-js";
import { AdaptiveNormalizer, easeParams, makeCurve, materialColors, stateParams } from "./math";
import type { BlobState, Rgb, VisualParams } from "./math";
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

const labels: Record<BlobState, string> = {
  idle: "Voice idle",
  listening: "Listening",
  processing: "Processing voice",
  speaking: "Speaking",
  error: "Voice error",
  disconnected: "Voice disconnected",
};

const TRANSITION_MS = 250;
const css = (c: Rgb, alpha = 1) => `rgba(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)}, ${alpha})`;

/** When `localStorage["voice-blob:debug"] = "1"`, each blob overlays its renderer, state, level, and frame time. */
const debugEnabled = () => {
  try {
    return globalThis.localStorage?.getItem("voice-blob:debug") === "1";
  } catch {
    return false;
  }
};

export function VoiceBlob(props: VoiceBlobProps): JSX.Element {
  let host!: HTMLDivElement;
  let glCanvas!: HTMLCanvasElement;
  let flatCanvas!: HTMLCanvasElement;
  let debug!: HTMLSpanElement;
  const mode = () => props.mode ?? "expanded";
  const size = () => props.size ?? (mode() === "compact" ? 20 : 220);
  const theme = () => props.theme ?? "dark";
  const reduced = () => props.reducedMotion ?? window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  onMount(() => {
    const showDebug = debugEnabled();
    const mic = new AdaptiveNormalizer();
    const out = new AdaptiveNormalizer();
    let renderer: BlobRenderer | null = createBlobRenderer(glCanvas, mode() === "compact" ? 3 : 5);
    let flat: CanvasRenderingContext2D | null = null;
    let raf = 0;
    let visible = false;
    let inView = true;
    let lastFrame = 0;
    let lastPaint = 0;
    let time = Math.random() * 100;
    let side = size();
    let dpr = 1;
    let dirty = true;
    let state = props.state();
    let params: VisualParams = stateParams(state);
    let from = params;
    let target = params;
    let changedAt = -Infinity;
    let frameMs = 0;

    const useRenderer = () => {
      glCanvas.style.display = renderer ? "block" : "none";
      flatCanvas.style.display = renderer ? "none" : "block";
      if (!renderer && !flat) flat = flatCanvas.getContext("2d");
    };
    const resize = () => {
      side = Math.max(1, size());
      dpr = Math.min(2, window.devicePixelRatio || 1);
      if (renderer) renderer.resize(side, side, dpr);
      flatCanvas.width = Math.round(side * dpr);
      flatCanvas.height = Math.round(side * dpr);
      dirty = true;
      schedule();
    };
    const setGlow = (accent: Rgb) => {
      host.style.setProperty("--voice-blob-glow", css(accent, theme() === "dark" ? 0.32 : 0.22));
    };

    const drawFlat = (amplitude: number, colors: { body: Rgb; band: Rgb }, accent: Rgb) => {
      if (!flat) return;
      const radius = side * 0.42;
      const center = side / 2;
      flat.setTransform(dpr, 0, 0, dpr, 0, 0);
      flat.clearRect(0, 0, side, side);
      flat.globalAlpha = 1 - params.dim;
      const curve = makeCurve({ time: time * 1000, amplitude, processing: state === "processing" ? 1 : 0, error: 0, points: 12, radius });
      flat.save();
      flat.translate(center - radius, center - radius);
      flat.beginPath();
      flat.moveTo(curve.start.x, curve.start.y);
      for (const s of curve.segments) flat.bezierCurveTo(s.c1.x, s.c1.y, s.c2.x, s.c2.y, s.end.x, s.end.y);
      flat.closePath();
      const fill = flat.createRadialGradient(radius * 0.75, radius * 0.6, radius * 0.1, radius, radius, radius * 1.1);
      fill.addColorStop(0, css(colors.band, 0.9));
      fill.addColorStop(0.45, css(colors.body));
      fill.addColorStop(1, css(accent));
      flat.fillStyle = fill;
      flat.fill();
      flat.restore();
      flat.globalAlpha = 1;
    };

    const paint = (now: number, dt: number) => {
      const next = props.state();
      if (next !== state) {
        state = next;
        from = params;
        target = stateParams(next);
        changedAt = now;
        setGlow(target.accent);
      }
      params = easeParams(from, target, now - changedAt, TRANSITION_MS);
      const level =
        state === "listening" ? mic.update(props.levels.mic, dt) : state === "speaking" ? out.update(props.levels.out, dt) : (mic.update(0, dt), out.update(0, dt), 0);
      const amplitude = Math.min(1, params.amplitude + level * params.levelGain * (reduced() ? 0.1 : 1));
      if (!reduced()) time += dt * params.speed;
      const colors = materialColors(theme(), params.accent, mode() === "compact");
      if (renderer) {
        renderer.render({ time, amplitude, pulse: state === "speaking" ? level : 0, dim: params.dim, body: colors.body, band: colors.band, accent: params.accent });
      } else {
        drawFlat(amplitude, colors, params.accent);
      }
      if (showDebug) debug.textContent = `${renderer ? "webgl" : "2d"} ${state} lvl ${level.toFixed(2)} amp ${amplitude.toFixed(2)} ${frameMs.toFixed(1)}ms`;
      dirty = false;
      return now - changedAt < TRANSITION_MS;
    };

    const tick = (now: number) => {
      raf = 0;
      if (!visible) return;
      const isReduced = reduced();
      // Full rate while reacting to audio or changing state; ~30 FPS at rest; ~6 FPS with reduced motion.
      const interval = isReduced ? 1000 / 6 : state === "listening" || state === "speaking" ? 0 : 1000 / 30;
      let transitioning = false;
      if (dirty || now - lastPaint >= interval) {
        const started = performance.now();
        const dt = lastFrame ? Math.min(0.1, (now - lastFrame) / 1000) : 1 / 60;
        lastFrame = lastPaint = now;
        transitioning = paint(now, dt);
        frameMs = performance.now() - started;
      } else {
        transitioning = now - changedAt < TRANSITION_MS;
      }
      const moving = !isReduced && state !== "disconnected";
      const reactive = isReduced && (state === "listening" || state === "speaking");
      if (moving || reactive || transitioning || dirty) schedule();
    };
    function schedule() {
      if (visible && !raf) raf = requestAnimationFrame(tick);
    }
    const updateVisibility = () => {
      visible = host.isConnected && document.visibilityState === "visible" && inView;
      if (visible) {
        lastFrame = 0;
        dirty = true;
        schedule();
      } else if (raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
    };

    const observer = new IntersectionObserver((entries) => {
      inView = entries.some((entry) => entry.isIntersecting);
      updateVisibility();
    });
    observer.observe(host);
    const onLost = (event: Event) => {
      event.preventDefault();
      renderer = null;
      useRenderer();
      dirty = true;
      schedule();
    };
    const onRestored = () => {
      renderer = createBlobRenderer(glCanvas, mode() === "compact" ? 3 : 5);
      useRenderer();
      resize();
    };
    glCanvas.addEventListener("webglcontextlost", onLost);
    glCanvas.addEventListener("webglcontextrestored", onRestored);
    document.addEventListener("visibilitychange", updateVisibility);
    window.addEventListener("resize", resize, { passive: true });

    useRenderer();
    setGlow(params.accent);
    resize();
    createEffect(() => {
      props.state();
      theme();
      size();
      props.reducedMotion;
      setGlow(stateParams(props.state()).accent);
      resize();
    });
    updateVisibility();

    onCleanup(() => {
      if (raf) cancelAnimationFrame(raf);
      observer.disconnect();
      document.removeEventListener("visibilitychange", updateVisibility);
      window.removeEventListener("resize", resize);
      glCanvas.removeEventListener("webglcontextlost", onLost);
      glCanvas.removeEventListener("webglcontextrestored", onRestored);
      renderer?.dispose();
      const gl = (glCanvas.getContext("webgl2") ?? glCanvas.getContext("webgl")) as WebGLRenderingContext | null;
      gl?.getExtension("WEBGL_lose_context")?.loseContext();
    });
  });

  return (
    <div
      ref={host}
      class={`voice-blob voice-blob--${mode()} ${props.class ?? ""}`}
      style={{ width: `${size()}px`, height: `${size()}px` }}
      role="img"
      aria-label={props.label ?? labels[props.state()]}
      data-state={props.state()}
      data-theme={theme()}
    >
      <span class="voice-blob__glow" aria-hidden="true" />
      <canvas ref={glCanvas} aria-hidden="true" />
      <canvas ref={flatCanvas} aria-hidden="true" />
      <span class="voice-blob__forced" aria-hidden="true" />
      <span ref={debug} class="voice-blob__debug" aria-hidden="true" />
    </div>
  );
}
