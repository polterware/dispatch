export type VoiceState = "off" | "connecting" | "idle" | "listening" | "transcribing" | "submitting" | "processing" | "speaking" | "error" | "disconnected"
export type BlobState = "idle" | "listening" | "processing" | "speaking" | "error" | "disconnected"
export type ControllerEvent =
  | { type: "enable" } | { type: "connected" } | { type: "disconnected" } | { type: "disable" }
  | { type: "listen.start" } | { type: "listen.stop" } | { type: "listen.cancel" }
  | { type: "transcript.partial"; text: string } | { type: "transcript.final"; text: string }
  | { type: "prompt.submitted" } | { type: "turn.started" } | { type: "turn.ended" }
  | { type: "speak.started" } | { type: "speak.ended"; reason: "done" | "cancelled" | "error" }
  | { type: "failure" } | { type: "mute"; value: boolean } | { type: "hands-free"; value: boolean }

export interface VoiceController {
  readonly levels: { mic: number; out: number }
  state: VoiceState
  micMuted: boolean
  handsFree: boolean
  partial: string
  final: string
  dispatch(event: ControllerEvent): void
  subscribe(listener: () => void): () => void
  blobState(): BlobState
}

export function createVoiceController(): VoiceController {
  const listeners = new Set<() => void>()
  const controller: VoiceController = {
    levels: { mic: 0, out: 0 },
    state: "off", micMuted: false, handsFree: false, partial: "", final: "",
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener) },
    blobState() {
      if (this.state === "error") return "error"
      if (this.state === "disconnected") return "disconnected"
      if (this.state === "off" || this.state === "connecting") return "idle"
      if (this.state === "listening") return "listening"
      if (this.state === "speaking") return "speaking"
      if (this.state === "transcribing" || this.state === "submitting" || this.state === "processing") return "processing"
      return "idle"
    },
    dispatch(event) {
      switch (event.type) {
        case "enable": this.state = "connecting"; break
        case "connected": this.state = "idle"; break
        case "disconnected": this.state = "disconnected"; break
        case "disable": this.state = "off"; this.partial = ""; this.levels.mic = 0; this.levels.out = 0; break
        case "listen.start": this.state = "listening"; this.partial = ""; break
        case "listen.stop": this.state = "transcribing"; break
        case "listen.cancel": this.state = "idle"; this.partial = ""; break
        case "transcript.partial": this.partial = event.text; break
        case "transcript.final": this.final = event.text; this.partial = ""; this.state = event.text.trim() ? "submitting" : "idle"; break
        case "prompt.submitted": this.state = "processing"; break
        case "turn.started": this.state = "processing"; break
        case "turn.ended": this.state = "idle"; break
        case "speak.started": this.state = "speaking"; break
        case "speak.ended": this.state = this.handsFree && event.reason === "done" ? "listening" : "idle"; break
        case "failure": this.state = "error"; break
        case "mute": this.micMuted = event.value; break
        case "hands-free": this.handsFree = event.value; break
      }
      for (const listener of listeners) listener()
    },
  }
  return controller
}
