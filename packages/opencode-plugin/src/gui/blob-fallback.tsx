import { createEffect, createSignal, onCleanup, type Accessor, type JSX } from "solid-js"
export type BlobState = "idle" | "listening" | "processing" | "speaking" | "error" | "disconnected"
export type BlobProps = { state: Accessor<BlobState>; levels: { mic: number; out: number }; mode: "expanded" | "compact"; size: number; reducedMotion?: boolean; theme: "light" | "dark" }
export function VoiceBlob(props: BlobProps): JSX.Element {
  const [pulse, setPulse] = createSignal(1)
  createEffect(() => {
    if (props.reducedMotion || props.state() === "idle") return
    const timer = setInterval(() => setPulse(0.92 + Math.random() * 0.16), 180)
    onCleanup(() => clearInterval(timer))
  })
  return <div aria-label={`Voice ${props.state()}`} style={{ width: `${props.size}px`, height: `${props.size}px`, "border-radius": "50%", background: props.theme === "dark" ? "#9a72ff" : "#7253d6", opacity: props.state() === "error" ? 0.6 : 1, transform: `scale(${pulse()})`, transition: props.reducedMotion ? "none" : "transform 180ms ease-out" }} />
}
