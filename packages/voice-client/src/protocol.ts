// Wire protocol v1, mirrored from crates/voice-core/src/protocol.rs.
// Keep both files identical on the wire; see docs/protocol.md.

export const PROTOCOL_VERSION = 1

export type Language = "auto" | "pt-br" | "en"
export type ListenMode = "push-to-talk" | "hands-free"
export type TextFormat = "markdown" | "plain"

export type ClientMessage =
  | { type: "listen.start"; id: string; language?: Language; mode?: ListenMode }
  | { type: "listen.stop"; id: string }
  | { type: "listen.cancel"; id: string }
  | { type: "speak"; id: string; text: string; format?: TextFormat; language?: Language; voice?: string }
  | { type: "speak.cancel"; id?: string }
  | { type: "mic.mute"; muted: boolean }
  | { type: "models.warm"; stt?: boolean; tts?: boolean }
  | { type: "ping"; nonce?: number }

export type MicState = "idle" | "listening" | "muted"
export type SttState = "unloaded" | "loading" | "ready" | "transcribing"
export type TtsState = "unloaded" | "loading" | "ready" | "speaking"

export interface EngineState {
  mic: MicState
  stt: SttState
  tts: TtsState
  listen_id?: string
  speak_id?: string
  clients: number
}

export type LevelSource = "mic" | "output"
export type SpeakEndReason = "done" | "cancelled" | "error"

export interface TranscriptTimings {
  audio_ms: number
  stop_to_final_ms: number
  inference_ms: number
}

export type ErrorCode =
  | "busy"
  | "model_missing"
  | "audio_device"
  | "mic_permission"
  | "stt_failed"
  | "tts_failed"
  | "bad_request"
  | "internal"

export type EngineMessage =
  | { type: "hello"; protocol: number; engine_version: string; state: EngineState }
  | { type: "state"; state: EngineState }
  | { type: "level"; source: LevelSource; rms: number; peak: number }
  | { type: "vad"; id: string; speech: boolean }
  | { type: "transcript.partial"; id: string; text: string }
  | { type: "transcript.final"; id: string; text: string; language: Language; timings: TranscriptTimings }
  | { type: "speak.started"; id: string; first_audio_ms: number }
  | { type: "speak.ended"; id: string; reason: SpeakEndReason }
  | { type: "pong"; nonce: number }
  | { type: "error"; code: ErrorCode; message: string; id?: string }

export interface Health {
  ok: boolean
  protocol: number
  engine_version: string
  pid: number
}

/** The discovery file a running engine writes (0600). */
export interface EngineInfo {
  protocol: number
  engine_version: string
  pid: number
  port: number
  token: string
}
