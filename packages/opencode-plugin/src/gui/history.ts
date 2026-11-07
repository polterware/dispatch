import { extractResponseText } from "./response-text.ts"

export interface Reply {
  id: string
  text: string
  created?: number
}

type Message = { type: string; id?: string; time?: { created?: number }; content?: Array<{ type?: string; text?: string }> }

/** The session's assistant replies that have text, newest first. */
export function assistantReplies(messages: ReadonlyArray<{ type: string }>, limit = 30): Reply[] {
  const replies: Reply[] = []
  for (let index = messages.length - 1; index >= 0 && replies.length < limit; index--) {
    const message = messages[index] as Message
    if (message.type !== "assistant") continue
    const text = extractResponseText({ role: "assistant", parts: message.content })
    if (!text) continue
    replies.push({ id: message.id ?? String(index), text, created: message.time?.created })
  }
  return replies
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
