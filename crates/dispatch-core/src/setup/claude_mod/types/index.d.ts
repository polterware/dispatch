export type DelegateMode = 'off' | 'default' | 'mandatory'
export type DelegateTier = 'auto' | 'easy' | 'normal' | 'hard'
export type DelegateEffort = '' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
export type DelegatePriority = '' | 'speed' | 'balanced' | 'quality'
export type DelegateWorktree = '' | 'worktree' | 'in-place'

/** Every way `dispatch run` can be routed; '' means "leave it to Dispatch". */
export type DelegateSettings = {
  mode: DelegateMode
  tier: DelegateTier
  agent: string
  model: string
  provider: string
  role: string
  effort: DelegateEffort
  priority: DelegatePriority
  worktree: DelegateWorktree
  relax: boolean
  readonly: boolean
}
/** Efforts an agent takes: a list, 'any' (set through the model name) or 'none'. */
export type AgentEfforts = string[] | 'any' | 'none'
export type AgentModels = { agent: string; provider: string; models: string[]; efforts: AgentEfforts }
/** What `dispatch route` said of the current flags. */
export type DelegateCheck = {
  status: 'idle' | 'checking' | 'ok' | 'fail'
  flags: string
  winner?: string
  reason?: string
}
export type DelegateCatalog = { agents: AgentModels[]; roles: string[]; error?: string }

declare module 'claude-code' {
  interface PluginState {
    'dispatch-delegate': {
      prefs: Shaped<DelegateSettings>
      catalog: Shaped<DelegateCatalog>
      check: Shaped<DelegateCheck>
    }
  }
}
