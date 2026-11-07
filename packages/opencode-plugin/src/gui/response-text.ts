export type MessagePart = { type?: string; text?: string }
export type AssistantMessage = { role?: string; info?: { role?: string }; parts?: MessagePart[] }

export function extractResponseText(message: AssistantMessage): string {
  if ((message.role ?? message.info?.role) !== "assistant") return ""
  return (message.parts ?? []).filter(part => part.type === "text" && typeof part.text === "string").map(part => part.text!.trim()).filter(Boolean).join("\n")
}
