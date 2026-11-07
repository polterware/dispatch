import type { BotAvatarExpression } from "@polterware/bot-avatars";

// The face a run's bot holds in the Dispatch panel: its status, and whether it merged.
// A working run keeps its animated bot (no held face); every other run is paused with
// this face, as the app shows the same run's bot.

/** The held face for a run, or `null` while it is queued or working. */
export function expressionFor(status: string, mergedAt?: string): BotAvatarExpression | null {
  switch (String(status || "").trim().toUpperCase()) {
    case "COMPLETED":
    case "SUCCEEDED":
      return String(mergedAt || "").trim() ? "celebrate" : "happy";
    case "FAILED":
      return "sad";
    case "CANCELLED":
    case "CANCELED":
      return "cancelled";
    case "TIMED_OUT":
      return "tired";
    case "ORPHANED":
      return "confused";
    default:
      return null;
  }
}
