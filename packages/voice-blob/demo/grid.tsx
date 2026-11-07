import { For } from "solid-js"
import { render } from "solid-js/web"
import { VoiceBlob } from "../src"
import type { BlobState } from "../src"

const states: BlobState[] = ["idle", "listening", "processing", "speaking", "error", "disconnected"]
const levels = { mic: 0, out: 0 }
let t = 0
setInterval(() => {
  t += 0.033
  const speechLike = Math.max(0, Math.sin(t * 7) * 0.5 + Math.sin(t * 13.1) * 0.3) * 0.25
  levels.mic = speechLike
  levels.out = speechLike
}, 33)

function Row(props: { theme: "light" | "dark" }) {
  return (
    <div class={`row ${props.theme}`}>
      <For each={states}>
        {(state) => (
          <div class="cell">
            <VoiceBlob state={() => state} levels={levels} theme={props.theme} size={150} />
            <span class="pill"><VoiceBlob state={() => state} levels={levels} theme={props.theme} mode="compact" size={18} />{state}</span>
          </div>
        )}
      </For>
    </div>
  )
}

const only = new URLSearchParams(location.search).get("theme")
render(() => (<>{only !== "light" && <Row theme="dark" />}{only !== "dark" && <Row theme="light" />}</>), document.getElementById("root")!)
