# @local-voice/blob

A standalone SolidJS voice indicator with an accessible Canvas 2D renderer. The package has no runtime dependency beyond its `solid-js` peer.

## Use

```tsx
import { VoiceBlob } from "@local-voice/blob";
import type { BlobLevels, BlobState } from "@local-voice/blob";

const levels: BlobLevels = { mic: 0, out: 0 };
const state = () => "idle" satisfies BlobState;
<VoiceBlob state={state} levels={levels} />
```

The level object is read directly each animation frame. Update its `mic` and `out` values in place with raw linear RMS values. Defaults are 220 CSS px for expanded and 20 CSS px for compact. Theme defaults to dark.

## Rendering

Canvas 2D provides direct control over a small radial silhouette, gradients, a restrained inner sheen, and the smooth cubic path. At these desktop-panel sizes the shape has only eight to ten segments; WebGL would add shader, context, and GPU lifecycle complexity without improving the visible result. A capped-DPR backing store keeps the drawing crisp. Glow is a low-alpha radial ring, not a per-frame blur or large shadow.

The renderer owns one requestAnimationFrame chain per mounted component. It pauses while outside the viewport or while the document is hidden; idle breathing is capped at 24 FPS and settles after a short period. State parameters blend over 380 ms. Listening and speaking use independent adaptive peak normalizers and amplitude smoothing. Reduced motion keeps the geometry static and samples the level at up to 6 FPS. Forced-colors mode replaces the canvas with a system-color outline.

## Demo and checks

```sh
bun install
bun run --cwd packages/voice-blob dev
bun run --cwd packages/voice-blob build:demo
bun test packages/voice-blob
bunx tsc --noEmit --project packages/voice-blob/tsconfig.json
```

The demo offers state buttons, level control, a local microphone analyser, synthetic speech motion, both themes, compact and expanded examples, and a per-second frame interval meter. The microphone permission is only used by the demo; the component itself does not access audio devices.

`bench/frame-cost.ts` measures pure curve generation because Bun in this workspace does not provide a headless Canvas implementation. This isolates the procedural path math; it is not a browser paint/GPU benchmark. Browser frame cost depends on device, DPR, compositing, and concurrent application work.
