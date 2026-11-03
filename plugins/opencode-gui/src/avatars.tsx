import { BotAvatar, agentMarkFor, botAvatarTypes, type BotAvatarType } from "@polterware/bot-avatars";

// The same bots the Dispatch app draws: the Polterware ghost for an empty
// page, and each agent's own bot, which moves while its run works.

function hash32(id: string) {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
  return h >>> 0;
}

const FABRIC = { shading: "fabric", furLength: 1, furDensity: 1.6, furFuzz: 0.9 } as const;

/** The Polterware ghost, asleep while nothing has run and lost while delegation is off. */
export function EmptyBot(props: { mood: "sleeping" | "lost"; size?: number }) {
  return (
    <BotAvatar
      {...FABRIC}
      type="ghost"
      state={props.mood === "sleeping" ? "sleeping" : "default"}
      size={props.size ?? 64}
      jumpEvery={0}
      turn={props.mood === "lost" ? 1 : 0.35}
      aria-hidden="true"
    />
  );
}

/** An agent's bot: its mark when it has one, else a body from its name. */
export function AgentBot(props: { agent?: string; seed: string; status: string; size?: number }) {
  const key = () => props.agent?.trim() || "agent";
  const mark = () => agentMarkFor(key());
  const type = (): BotAvatarType => mark()?.type ?? botAvatarTypes[hash32(key()) % botAvatarTypes.length] ?? "clover";
  const working = () => props.status === "PENDING" || props.status === "RUNNING";
  const failed = () => ["FAILED", "CANCELLED", "TIMED_OUT"].includes(props.status);
  return (
    <BotAvatar
      {...FABRIC}
      type={type()}
      color={mark()?.color}
      state={working() ? "working" : failed() ? "sleeping" : "default"}
      size={props.size ?? 28}
      seed={(hash32(props.seed) % 1000) / 1000}
      aria-hidden="true"
      {...(working() ? {} : { jumpEvery: 0, turn: 0.35 })}
    />
  );
}
