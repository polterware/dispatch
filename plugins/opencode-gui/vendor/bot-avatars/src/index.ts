export { BotAvatar } from "./BotAvatar";
export { agentMarkFor, type AgentMark } from "./agent-marks";

export {
  botAvatarFaces,
  botAvatarPalette,
  botAvatarPresets,
  botAvatarStates,
  botAvatarTypes,
} from "./presets";
export { SHAPE_PARTS as botAvatarParts, SHAPE_PATHS as botAvatarShapes } from "./shapes";
export { autoInk, luminance, parseColor, shade } from "./color";
export { Sim as BotAvatarSim, restPose } from "./engine";
export { draw as drawBotAvatarFrame, OVERSCAN as BOT_AVATAR_OVERSCAN, RISE as BOT_AVATAR_RISE } from "./draw";
export { warmPlastic as warmBotAvatarPlastic } from "./plastic";
export {
  MATCAP_SIZE as BOT_AVATAR_MATCAP_SIZE,
  PAD as BOT_AVATAR_PAD,
  SPAN as BOT_AVATAR_SPAN,
  buildForm as bakeBotAvatarForm,
  buildMatcap as buildBotAvatarMatcap,
  capFrame as botAvatarCapFrame,
  shadeTexels as shadeBotAvatarTexels,
  tierFor as botAvatarTier,
} from "./plastic";
export type { Form as BotAvatarForm, Frame as BotAvatarFrame, Material as BotAvatarMaterial, Rig as BotAvatarRig } from "./plastic";
export { JUMP_DEFAULTS as botAvatarJumpDefaults } from "./engine";
export type { JumpConfig as BotAvatarJumpConfig } from "./engine";
export type { DrawConfig as BotAvatarDrawConfig } from "./draw";
export type { Pose as BotAvatarPose } from "./engine";

export type {
  BotAvatarFace,
  BotAvatarGlasses,
  BotAvatarHat,
  BotAvatarPreset,
  BotAvatarProps,
  BotAvatarShading,
  BotAvatarSquashEase,
  BotAvatarState,
  BotAvatarType,
} from "./types";
