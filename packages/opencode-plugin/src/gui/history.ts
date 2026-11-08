import { extractResponseText } from "./response-text.ts"

export interface Reply {
  id: string
  text: string
  created?: number
}

type Message = { type: string; id?: string; time?: { created?: number }; content?: Array<{ type?: string; text?: string }> }

/**
 * The session's replies, newest first. One reply is a whole turn: every assistant message
 * (each step between tool calls) after one user message, joined in order.
 */
export function assistantReplies(messages: ReadonlyArray<{ type: string }>, limit = 30): Reply[] {
  const replies: Reply[] = []
  let parts: string[] = []
  let id: string | undefined
  let created: number | undefined
  const flush = () => {
    if (parts.length && id) replies.push({ id, text: parts.join("\n\n"), created })
    parts = []
    id = undefined
    created = undefined
  }
  for (const raw of messages) {
    const message = raw as Message
    if (message.type === "user") {
      flush()
      continue
    }
    if (message.type !== "assistant") continue
    const text = extractResponseText({ role: "assistant", parts: message.content })
    if (!text) continue
    id ??= message.id
    parts.push(text)
    created = message.time?.created ?? created
  }
  flush()
  return replies.reverse().slice(0, limit)
}

export function timeLabel(created: number | undefined, now = Date.now()): string {
  if (!created) return ""
  const minutes = Math.round((now - created) / 60_000)
  if (minutes < 1) return "just now"
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  return new Date(created).toLocaleDateString()
}
