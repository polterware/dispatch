import {
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  onCleanup,
  splitProps,
  untrack,
  type JSX,
} from "solid-js";
import { autoInk, richer, shade } from "./color";
import { draw, LIGHT_DEFAULTS, OVERSCAN, RISE, type DrawConfig } from "./draw";
import { Sim, restPose, type JumpConfig } from "./engine";
import { bakesPending, hurry, onLanded, stockLights, takeMissed, warmPlastic, type FurStyle } from "./plastic";
import { botAvatarPresets, stateLabels } from "./presets";
import { SHAPE_PART_COLORS, SHAPE_PARTS, SHAPE_PATHS } from "./shapes";
import { pointer, subscribe } from "./ticker";
import type { BotAvatarProps, BotAvatarShading, BotAvatarState } from "./types";

/* A 0–1 seed from the instance id, so two avatars side by side never blink
   in step unless asked to. */
function hashSeed(id: string): number {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
  return ((h >>> 0) % 1000) / 1000;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : 1));

const pathCache = new Map<string, Path2D>();
function bodyPath(d: string): Path2D {
  let p = pathCache.get(d);
  if (!p) {
    p = new Path2D(d);
    pathCache.set(d, p);
  }
  return p;
}

/* the thin parts one piece per subpath, so each rounds on its own */
const partsCache = new Map<string, Path2D[]>();
function partPaths(d: string): Path2D[] {
  let p = partsCache.get(d);
  if (!p) {
    p = d.split(/(?=M)/).filter((s) => s.trim()).map((s) => new Path2D(s));
    partsCache.set(d, p);
  }
  return p;
}

/* the longest an avatar waits for its material before it is shown anyway */
const SHOW_WITHIN = 4000;
/* the avatars waiting to be shown, each with whether it is ready: they are
   shown together, once all are and no bake is under way */
const waiters = new Map<object, { ready: () => boolean; show: () => void }>();
function showReady() {
  if (bakesPending()) return;
  for (const w of waiters.values()) if (!w.ready()) return;
  const all = [...waiters.values()];
  waiters.clear();
  for (const w of all) w.show();
}

const reducedMotion = () =>
  typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

function shadingModeOf(shading: BotAvatarProps["shading"]): BotAvatarShading {
  if (shading === true) return "crisp";
  if (shading === false) return "flat";
  return shading ?? "fabric";
}

/** Animated bot avatar. The canvas engine is the MIT bot-avatars renderer; this wrapper is Solid. */
export function BotAvatar(props: BotAvatarProps) {
  const [local, rest] = splitProps(props, [
    "type",
    "path",
    "face",
    "state",
    "expression",
    "size",
    "color",
    "ink",
    "brightness",
    "saturation",
    "speed",
    "paused",
    "seed",
    "shading",
    "hat",
    "glasses",
    "headphones",
    "bowTie",
    "accessoryColor",
    "shadow",
    "highlight",
    "depth",
    "roundness",
    "furLength",
    "furDensity",
    "furFuzz",
    "furClumps",
    "furCurl",
    "furGravity",
    "light",
    "rim",
    "spread",
    "backLight",
    "lightFront",
    "shine",
    "sheen",
    "backSoftness",
    "pose",
    "interactive",
    "turn",
    "theme",
    "whirl",
    "whirlSize",
    "whirlWidth",
    "whirlLength",
    "whirlTilt",
    "jumpHeight",
    "jumpTime",
    "jumpStretch",
    "jumpSpin",
    "jumpLean",
    "jumpEvery",
    "jumpLand",
    "jumpSquash",
    "jumpSquashTime",
    "jumpSquashEase",
    "jumpGroundTime",
    "jumpGroundEase",
    "jumpRiseTime",
    "jumpRiseEase",
    "jumpClickSquashTime",
    "class",
    "className",
    "style",
    "aria-label",
    "ref",
    "onClick",
  ]);

  const instanceId = createUniqueId();
  const startsShown = !isMaterial(shadingModeOf(local.shading));
  const [shown, setShown] = createSignal(startsShown);
  let shownLive = startsShown;
  let canvasEl: HTMLCanvasElement | undefined;
  let sim: Sim | null = null;
  let cfg: DrawConfig | null = null;
  let cssSize = 0;
  let speedNow = 1;
  let interactiveLive = true;
  let heldNow: BotAvatarProps["pose"];
  let stateKey: BotAvatarState = "default";
  let sizeNow: number | string = 64;
  let themeMode: "auto" | "dark" | "light" = "auto";
  let frozenNow = false;
  let stillPaint: (() => void) | null = null;
  let waiting: (() => void) | null = null;
  let showPaint: (() => void) | null = null;
  let watch: (() => void) | null = null;
  const me = {};
  let ready = false;
  let lastLook: string | null = null;
  let holding: (() => void) | null = null;

  const markShown = (value: boolean) => {
    shownLive = value;
    setShown(value);
  };

  const model = createMemo(() => {
    const type = local.type ?? "clover";
    const preset = botAvatarPresets[type] ?? botAvatarPresets.clover;
    const faceKind = local.face ?? preset.face;
    const state = local.state ?? "default";
    const stateKeyNow: BotAvatarState = state in stateLabels ? state : "default";
    const expression = local.expression;
    const size = local.size ?? 64;
    const picked = local.color ?? preset.color;
    const brightness = local.brightness ?? preset.brightness ?? 1;
    const saturation = local.saturation ?? preset.saturation ?? 1.5;
    const vivid = clamp(saturation, 1.5, 2.5) - 1.5;
    const body = richer(
      brightness === 1 && saturation === 1
        ? picked
        : shade(picked, (Math.min(2, Math.max(0, brightness)) - 1) * 0.35, (Math.min(1.5, Math.max(0, saturation)) - 1) * 0.5),
      vivid,
    );
    const speed = local.speed ?? 1;
    const frozen = (local.paused ?? false) || !(speed > 0);
    const shadingMode = shadingModeOf(local.shading);
    const customPath = typeof local.path === "string" && local.path.trim() ? local.path.trim() : null;
    const outlineKey = customPath ? `path:${hashSeed(customPath)}:${customPath.length}` : type;
    const lit = LIGHT_DEFAULTS[shadingMode === "fabric" ? "fabric" : "other"];
    const q = (v: number, lo: number, hi: number) => Math.round(clamp(v, lo, hi) * 20) / 20;
    const fur: FurStyle = {
      length: q(local.furLength ?? 1, 0.3, 2.5),
      density: q(local.furDensity ?? 1.6, 0.3, 2),
      fuzz: q(local.furFuzz ?? 0.9, 0, 1),
      clumps: q(local.furClumps ?? 0.4, 0, 1),
      curl: q(local.furCurl ?? 0.7, 0, 1),
      gravity: q(local.furGravity ?? 0.9, 0, 1),
    };
    const furId = `${fur.length},${fur.density},${fur.fuzz},${fur.clumps},${fur.curl},${fur.gravity}`;
    const depth = local.depth ?? 0.65;
    const part = !customPath ? SHAPE_PARTS[type] : undefined;
    const drawCfg: DrawConfig = {
      path: typeof Path2D === "undefined" ? (null as unknown as Path2D) : bodyPath(customPath ?? SHAPE_PATHS[type] ?? SHAPE_PATHS.clover),
      face: faceKind,
      expression,
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
      still: frozen || reducedMotion(),
      furOnIdle: true,
      whirl: {
        strength: clamp(local.whirl ?? 0, 0, 2),
        size: clamp(local.whirlSize ?? 1, 0.6, 1.6),
        width: clamp(local.whirlWidth ?? 1, 0.4, 2),
        length: clamp(local.whirlLength ?? 1, 0.4, 1.6),
        tilt: clamp(local.whirlTilt ?? 1, 0.5, 1.8),
      },
      partsColors: !customPath ? SHAPE_PART_COLORS[type] : undefined,
      parts: typeof Path2D !== "undefined" && part ? partPaths(part) : undefined,
      wear: {
        hat: local.hat ?? "none",
        glasses: local.glasses ?? "none",
        headphones: local.headphones ?? false,
        bowTie: local.bowTie ?? false,
        color: local.accessoryColor ?? "#27272b",
        fabric: shadingMode === "fabric",
      },
    };
    const jump: Partial<JumpConfig> = {
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
      clickSquashTime: Math.max(0.05, local.jumpClickSquashTime ?? 0.24),
    };
    return {
      type,
      preset,
      faceKind,
      stateKey: stateKeyNow,
      expression,
      size,
      seedValue: Math.min(1, Math.max(0, local.seed ?? hashSeed(instanceId))),
      speed,
      frozen,
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
      jump,
    };
  });

  const warmKey = createMemo(() => {
    const m = model();
    return `${m.shadingMode}|${m.outlineKey}|${String(m.size)}|${m.depth}|${m.furId}|${m.light ?? ""}|${m.backLight ?? ""}|${m.lightFront ?? ""}`;
  });

  const resolveTheme = (el: HTMLElement | null): "dark" | "light" => {
    if (themeMode !== "auto") return themeMode;
    const host = el?.closest("[data-theme], .dark, .light") as HTMLElement | null;
    if (host) {
      const v = host.getAttribute("data-theme");
      if (v === "dark" || v === "light") return v;
      if (host.classList.contains("dark")) return "dark";
      if (host.classList.contains("light")) return "light";
    }
    return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  };

  /* A canvas that is not in the document draws nothing: a list rebuilt off
     screen would bake a full set of textures for each copy, and enough of
     them run the GPU out of surfaces, which blanks every avatar on the page.
     The tick paints it once it is laid out and on screen. */
  function paint() {
    const canvas = canvasEl;
    const c = cfg;
    if (!canvas || !canvas.isConnected || !c || !c.path) return;
    const px = canvas.clientWidth / OVERSCAN || cssSize || (typeof sizeNow === "number" ? sizeNow : 64);
    if (!px) return;
    const dpr = Math.min(2, (typeof devicePixelRatio === "number" && devicePixelRatio) || 1);
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
    const pose = held
      ? { ...rest0, yaw: held.yaw ?? 0, pitch: held.pitch ?? rest0.pitch, roll: held.roll ?? rest0.roll }
      : sim
        ? sim.pose
        : rest0;
    draw(ctx, px, pose, c);
  }

  function paintStill() {
    const canvas = canvasEl;
    if (!canvas || !canvas.isConnected || !cfg || !cfg.path) return;
    const px = canvas.clientWidth / OVERSCAN || cssSize || (typeof sizeNow === "number" ? sizeNow : 64);
    if (!px) return;
    cssSize = px;
    const dpr = Math.min(2, (typeof devicePixelRatio === "number" && devicePixelRatio) || 1);
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

  function settle(paintNow: () => void) {
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

  function showWhenBaked(paintNow: () => void) {
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
    const m = untrack(model);
    if (!m.material || !cfg?.path) return;
    const path = cfg.path;
    const dev = (typeof m.size === "number" ? m.size : 64) * Math.min(2, (typeof devicePixelRatio === "number" && devicePixelRatio) || 1);
    const lightAt = cfg.light ?? 295;
    const a = (lightAt * Math.PI) / 180;
    const b = ((m.backLight ?? 0) * Math.PI) / 180;
    const lights = stockLights(Math.sin(a), -Math.cos(a));
    if (m.backLight !== undefined) [lights.bx, lights.by] = [Math.sin(b), -Math.cos(b)];
    if (m.lightFront !== undefined) lights.front = (Math.min(85, Math.max(0, m.lightFront)) * Math.PI) / 180;
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

  /* a canvas made off the document lays out again once it is in it, for a
     still avatar that no tick will paint */
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

  createEffect(layoutNow);

  createEffect(() => {
    warmKey();
    const m = untrack(model);
    if (!m.material || !cfg?.path) return;
    let idle = false;
    const id = typeof requestIdleCallback === "function"
      ? ((idle = true), requestIdleCallback(() => warm()))
      : window.setTimeout(() => warm(), 1);
    onCleanup(() => {
      if (idle && typeof cancelIdleCallback === "function") cancelIdleCallback(id);
      else window.clearTimeout(id);
    });
  });

  const frozen = createMemo(() => model().frozen);
  createEffect(() => {
    if (frozen() || reducedMotion()) return;
    const canvas = canvasEl;
    if (!canvas) return;
    let unsub: (() => void) | null = null;
    const REACH = 3;
    const tick = (dt: number) => {
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
    let io: IntersectionObserver | null = null;
    if (typeof IntersectionObserver === "function") {
      io = new IntersectionObserver((entries) => {
        if (entries[0]?.isIntersecting ?? true) run();
        else stop();
      });
      io.observe(canvas);
    } else run();
    onCleanup(() => {
      stop();
      io?.disconnect();
    });
  });

  createEffect(() => {
    if (shown()) return;
    waiters.set(me, { ready: () => ready, show: () => markShown(true) });
    showReady();
    hurry(true);
    const t = window.setTimeout(() => markShown(true), SHOW_WITHIN);
    onCleanup(() => {
      hurry(false);
      window.clearTimeout(t);
      stopWatch();
      waiters.delete(me);
      showReady();
    });
  });

  onCleanup(() => {
    disposed = true;
    if (attachWait) cancelAnimationFrame(attachWait);
    stopWaiting();
    holding?.();
  });

  const avatarClass = () => {
    const extra = local.class ?? local.className;
    return extra ? `ba ${extra}` : "ba";
  };

  const canvasStyle = (): JSX.CSSProperties => {
    const size = model().size;
    const dim = typeof size === "number" ? `${size * OVERSCAN}px` : `calc(${size} * ${OVERSCAN})`;
    const pull = (k: number) => (typeof size === "number" ? `${-size * k}px` : `calc(${size} * ${-k})`);
    const side = (OVERSCAN - 1) / 2;
    const base: JSX.CSSProperties = {
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
      transition: "opacity 0.3s ease-out",
    };
    const extra = local.style;
    return extra && typeof extra === "object" ? { ...base, ...extra } : base;
  };

  const handleClick: JSX.EventHandler<HTMLCanvasElement, MouseEvent> = (event) => {
    const m = model();
    if (m.interactive && !m.frozen && !m.heldPose) sim?.poke();
    const handler = local.onClick;
    if (typeof handler === "function") handler(event);
    else if (handler) handler[0](handler[1], event);
  };

  const setRef = (el: HTMLCanvasElement) => {
    canvasEl = el;
    const ref = local.ref;
    if (typeof ref === "function") ref(el);
  };

  return (
    <canvas
      {...rest}
      ref={setRef}
      class={avatarClass()}
      data-bot-avatar={model().type}
      data-face={model().faceKind}
      data-state={model().stateKey}
      data-expression={model().expression ?? "none"}
      role="img"
      aria-label={local["aria-label"] ?? `${model().preset.label} bot, ${stateLabels[model().stateKey]}`}
      style={canvasStyle()}
      onClick={handleClick}
    />
  );
}

function isMaterial(mode: BotAvatarShading) {
  return mode === "plastic" || mode === "fabric";
}
