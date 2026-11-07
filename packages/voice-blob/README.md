# @local-voice/blob

A standalone SolidJS voice indicator. Its public API is `VoiceBlob` with `state`, `levels`, `mode`, `size`, `reducedMotion`, `theme`, `class`, and `label` props. The only runtime peer dependency is SolidJS.

## Rendering

Expanded mode uses a raw WebGL2 renderer with WebGL1 fallback. TypeScript builds a cached-per-instance, subdivision-4 icosphere (2,562 vertices); the vertex shader displaces each point along its normal with animated 3D classic Perlin noise (`cnoise`). The fragment shader mixes the base and highlight colors in displacement-driven waves, then adds a low-cost Fresnel rim and soft specular light. No three.js, React, matrix package, or other runtime dependency is used. The transparent, premultiplied canvas sits directly over the app surface.

The visual direction is inspired by [R3F Blob](https://ui-layouts.com/components/r3f-blob) (MIT). The classic noise implementation is Stefan Gustavson's public-domain `cnoise` algorithm, adapted to GLSL ES.

State uniforms blend with smooth easing over 250 ms:

| State | Intensity | Speed | Accent |
|---|---:|---:|---|
| idle | 0.18 | 0.25 | neutral, gentle motion |
| listening | 0.15 + normalized mic × 0.55 | 0.4 | cool |
| processing | 0.2 | 0.8 | violet |
| speaking | 0.2 + normalized output × 0.6 | 0.55 | warm, small level scale pulse |
| error | 0.1 | 0.16 | muted red |
| disconnected | 0.05 | 0 | dim, desaturated |

Dark theme uses a near-black body with cool light bands; light theme uses a pale body with darker colored bands. Mic and output use separate adaptive peak normalizers with faster attack than release. The level object is read directly in the animation loop; Solid does not update per frame.

Compact mode always uses the simpler Canvas 2D renderer, allowing header dots without another WebGL context. Canvas 2D also takes over when WebGL is unavailable or its context is lost; restoration returns to WebGL. The component owns one animation-frame loop, runs only while visible, caps DPR at 2, and releases its buffers and context at unmount. Idle and disconnected render at 30 FPS; active states run at display rate. Reduced motion renders on state/theme changes, with levels sampled at up to 6 FPS. Forced-colors mode uses a system-color outline.

## Demo and checks

```sh
bun run --cwd packages/voice-blob dev
bun run --cwd packages/voice-blob build:demo
bun test packages/voice-blob
```

The demo includes all states, a level slider, local microphone sampling, synthetic speech, both themes, an expanded WebGL blob and a compact dot, plus a frame meter. Microphone access is used only by the demo.

`bench/frame-cost.ts` measures per-frame CPU-side uniform-target blending and level smoothing. It does not measure WebGL driver calls, browser shader execution, or GPU rendering.
