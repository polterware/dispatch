import { atom, read, update } from 'claude-code'
import type { Atom, EngineInterface, Register } from 'claude-code'

import type {
  AgentEfforts,
  DelegateCatalog,
  DelegateCheck,
  DelegateEffort,
  DelegateMode,
  DelegatePriority,
  DelegateSettings,
  DelegateTier,
  DelegateWorktree,
} from '../types'

const PANE = 'dispatch-delegate'
const STORE_KEY = 'settings'
const DEFAULTS: DelegateSettings = {
  mode: 'default',
  tier: 'auto',
  agent: '',
  model: '',
  provider: '',
  role: '',
  effort: '',
  priority: '',
  worktree: '',
  relax: false,
  readonly: false,
}

const settings: Atom<DelegateSettings> = atom(
  { plugin: 'dispatch-delegate', key: 'prefs' } as const,
  DEFAULTS,
  { shape: 'prefs-2' },
)
const catalog: Atom<DelegateCatalog> = atom(
  { plugin: 'dispatch-delegate', key: 'catalog' } as const,
  { agents: [], roles: [] } as DelegateCatalog,
  { shape: 'catalog-3' },
)
const check: Atom<DelegateCheck> = atom(
  { plugin: 'dispatch-delegate', key: 'check' } as const,
  { status: 'idle', flags: '' } as DelegateCheck,
  { shape: 'check-1' },
)

const MODES: DelegateMode[] = ['off', 'default', 'mandatory']
const MODE_LABEL: Record<DelegateMode, string> = { off: 'Off', default: 'Default', mandatory: 'Mandatory' }
const MODE_HINT: Record<DelegateMode, string> = {
  off: 'Claude works as usual; nothing is routed to Dispatch.',
  default: 'Claude delegates non-trivial work with dispatch run and keeps small edits.',
  mandatory: 'Project edits are blocked here; Claude reads, delegates, verifies and merges.',
}
const TIERS: DelegateTier[] = ['auto', 'easy', 'normal', 'hard']
const EFFORTS: DelegateEffort[] = ['', 'low', 'medium', 'high', 'xhigh', 'max']
const PRIORITIES: DelegatePriority[] = ['', 'speed', 'balanced', 'quality']
const WORKTREES: DelegateWorktree[] = ['', 'worktree', 'in-place']
const ANY = '__any__'
const NO_ROLE = 'none'

// Files the main loop may still write in mandatory mode: memory, plans,
// scratchpad and temp files, never project code.
const WRITABLE = /(\/\.claude\/|^\/private\/tmp\/|^\/tmp\/)/
const WRITERS = new Set(['Edit', 'Write', 'NotebookEdit'])

const cap = (t: string) => (t ? t[0]!.toUpperCase() + t.slice(1) : t)
const quote = (v: string) => (/^[\w./:@+-]+$/.test(v) ? v : `'${v.replace(/'/g, `'\\''`)}'`)

/** The `dispatch run` flags the settings stand for, in a stable order. */
function flagList(s: DelegateSettings): [name: string, value?: string][] {
  const out: [string, string?][] = []
  if (s.tier !== 'auto') out.push(['tier', s.tier])
  if (s.agent) out.push(['agent', s.agent])
  if (s.model) out.push(['model', s.model])
  if (s.provider) out.push(['provider', s.provider])
  if (s.role) out.push(['role', s.role])
  if (s.effort) out.push(['effort', s.effort])
  if (s.priority) out.push(['priority', s.priority])
  if (s.worktree) out.push(['worktree-mode', s.worktree])
  if (s.relax) out.push(['relax'])
  if (s.readonly) out.push(['readonly'])
  return out
}

const flagText = (f: [string, string?][]) =>
  f.map(([n, v]) => (v === undefined ? `--${n}` : `--${n} ${quote(v)}`)).join(' ')

function routeLabel(s: DelegateSettings): string {
  const f = flagList(s)
  return f.length ? flagText(f) : 'Auto'
}

function promptText(s: DelegateSettings): string {
  const f = flagList(s)
  const lines = [
    '# Delegation to Dispatch (dispatch-delegate mod)',
    'Delegate implementation, tests, reviews and code reading to Dispatch with `dispatch run`, following the `dispatch` skill: write a brief, `dispatch wait`, read the summary, check the diff stat and tests, then `dispatch merge`.',
    f.length
      ? `The person chose this routing: \`dispatch run ${flagText(f)}\`. The mod adds each of these flags to every \`dispatch run\` that does not pass that flag itself, so you need not repeat them; pass a flag yourself only to override it for one run.`
      : 'Leave routing on Auto unless a task clearly needs a tier.',
  ]
  lines.push(
    s.mode === 'mandatory'
      ? 'Delegation is MANDATORY: Edit, Write and NotebookEdit on project files are blocked in this session. Do not write project files through Bash either (sed -i, heredocs, scripts). Your job is to read, delegate, verify and merge. Memory, plan and scratchpad files under ~/.claude or /tmp stay writable.'
      : 'Do trivial changes (a line or two, a handful of calls) yourself; delegate the rest by default.',
  )
  return lines.join('\n')
}

/** Reads what any earlier version stored, the old `{ mode, route }` included. */
function migrate(raw: unknown): DelegateSettings {
  if (!raw || typeof raw !== 'object') return DEFAULTS
  const o = raw as Record<string, unknown>
  const s: DelegateSettings = { ...DEFAULTS }
  if (MODES.includes(o.mode as DelegateMode)) s.mode = o.mode as DelegateMode
  const route = o.route as { kind?: string; tier?: string; model?: string; agent?: string } | undefined
  if (route?.kind === 'tier' && TIERS.includes(route.tier as DelegateTier)) s.tier = route.tier as DelegateTier
  if (route?.kind === 'model') {
    s.model = route.model ?? ''
    s.agent = route.agent ?? ''
  }
  for (const k of ['agent', 'model', 'provider', 'role'] as const)
    if (typeof o[k] === 'string') s[k] = o[k] as string
  if (TIERS.includes(o.tier as DelegateTier)) s.tier = o.tier as DelegateTier
  if (EFFORTS.includes(o.effort as DelegateEffort)) s.effort = o.effort as DelegateEffort
  if (PRIORITIES.includes(o.priority as DelegatePriority)) s.priority = o.priority as DelegatePriority
  if (WORKTREES.includes(o.worktree as DelegateWorktree)) s.worktree = o.worktree as DelegateWorktree
  s.relax = o.relax === true
  s.readonly = o.readonly === true
  return s
}

async function save($: EngineInterface, s: DelegateSettings) {
  await update($, settings, () => s)
  await $.store.set(STORE_KEY, s)
  $.ui.status(s.mode === 'off' ? undefined : `Dispatch: ${MODE_LABEL[s.mode]} · ${routeLabel(s)}`)
  void runCheck($, s)
}

/** Efforts an agent takes, from `dispatch doctor`; 'any' when unknown. */
function effortsOf(cat: DelegateCatalog, agent: string): AgentEfforts {
  return cat.agents.find(a => a.agent === agent)?.efforts ?? 'any'
}

// Eliminations that only say a filter did its job, not why the route failed.
const EXPECTED = /(filter requires|is disabled)/

/** Asks `dispatch route` whether the current flags leave any candidate. */
async function runCheck($: EngineInterface, s: DelegateSettings): Promise<DelegateCheck> {
  const flags = flagList(s)
  const key = flagText(flags)
  if (flags.length === 0) {
    const done: DelegateCheck = { status: 'ok', flags: key, winner: 'Auto' }
    await update($, check, () => done)
    return done
  }
  await update($, check, () => ({ status: 'checking', flags: key }))
  // Auto would ask the classifier on every click; the normal table answers
  // whether the filters leave a candidate at all.
  const argv = ['dispatch', 'route', '--json']
  for (const [n, v] of flags) argv.push(`--${n}`, ...(v === undefined ? [] : [v]))
  if (s.tier === 'auto') argv.push('--tier', 'normal')
  if (!s.role) argv.push('--role', 'none')
  argv.push('check the route')
  // worktree-mode and readonly are run flags, not route flags.
  const routeArgv = argv.filter((a, i) => {
    if (a === '--readonly') return false
    if (a === '--worktree-mode') return false
    if (argv[i - 1] === '--worktree-mode') return false
    return true
  })
  let result: DelegateCheck
  try {
    const { stdout, stderr } = await $.process.run(routeArgv, { timeoutMs: 20000 })
    const d = JSON.parse(stdout) as {
      candidates?: { Agent?: string; Model?: string; Effort?: string }[]
      eliminated?: string[]
      reason?: string
    }
    const top = d.candidates?.[0]
    if (top) {
      result = {
        status: 'ok',
        flags: key,
        winner: `${top.Model ?? '?'} · ${top.Agent ?? '?'}${top.Effort ? ` · ${top.Effort}` : ''}`,
      }
    } else {
      const why = [...new Set((d.eliminated ?? []).filter(x => !EXPECTED.test(x)))]
      result = {
        status: 'fail',
        flags: key,
        reason: (why.length ? why.slice(0, 3).join('; ') : (d.reason ?? stderr ?? 'no eligible candidate')).slice(0, 300),
      }
    }
  } catch (err) {
    result = { status: 'fail', flags: key, reason: `dispatch route failed: ${String(err).slice(0, 200)}` }
  }
  // A later change may have started its own check; keep only the newest.
  const now = await read($, settings)
  if (flagText(flagList(now)) === key) await update($, check, () => result)
  return result
}

async function runJson($: EngineInterface, argv: string[]): Promise<unknown> {
  const { exitCode, stdout, stderr } = await $.process.run(argv, { timeoutMs: 20000 })
  if (exitCode !== 0) throw new Error((stderr || stdout).trim().slice(0, 200))
  return JSON.parse(stdout)
}

async function refresh($: EngineInterface): Promise<DelegateCatalog> {
  let next: DelegateCatalog
  try {
    const [models, agents, roles] = await Promise.all([
      runJson($, ['dispatch', 'models', '--json']),
      runJson($, ['dispatch', 'agents', '--json']).catch(() => ({})),
      runJson($, ['dispatch', 'roles']).catch(() => ({})),
    ])
    const doctor = (await runJson($, ['dispatch', 'doctor', '--json']).catch(() => [])) as {
      name?: string
      capabilities?: { effortMode?: string; efforts?: string[] }
    }[]
    const effortOf = new Map<string, AgentEfforts>()
    for (const d of Array.isArray(doctor) ? doctor : []) {
      if (!d.name) continue
      const c = d.capabilities ?? {}
      effortOf.set(d.name, c.efforts?.length ? c.efforts : c.effortMode ? 'any' : 'none')
    }
    const providerOf = new Map<string, string>()
    for (const a of (agents as { agents?: { Name: string; Provider?: string }[] }).agents ?? [])
      if (a.Provider) providerOf.set(a.Name, a.Provider)
    const raw = models as {
      agents?: { agent: string; enabled?: boolean; models?: { id: string; enabled?: boolean }[] }[]
    }
    next = {
      agents: (raw.agents ?? [])
        .filter(a => a.enabled !== false)
        .map(a => ({
          agent: a.agent,
          provider: providerOf.get(a.agent) ?? '',
          models: (a.models ?? []).filter(m => m.enabled !== false).map(m => m.id),
          efforts: effortOf.get(a.agent) ?? 'any',
        }))
        .filter(a => a.models.length > 0),
      roles: Object.keys((roles as { roles?: Record<string, unknown> }).roles ?? {}).sort(),
    }
  } catch (err) {
    next = { agents: [], roles: [], error: String(err).slice(0, 200) }
  }
  await update($, catalog, () => next)
  return next
}

function parseMode(arg: string): DelegateMode | undefined {
  const a = arg.trim().toLowerCase()
  if (a === 'off') return 'off'
  if (['default', 'standard', 'on'].includes(a)) return 'default'
  if (['mandatory', 'required'].includes(a)) return 'mandatory'
  return undefined
}

const CLEAR = new Set(['', 'any', 'auto', 'default', 'clear', 'off'])

/** `/delegate-set <key> <value>`: one option, or `reset`. */
function applySet(s: DelegateSettings, key: string, value: string): DelegateSettings | string {
  const v = value.trim()
  const clear = CLEAR.has(v.toLowerCase())
  switch (key) {
    case 'tier':
      if (clear) return { ...s, tier: 'auto' }
      return TIERS.includes(v as DelegateTier) ? { ...s, tier: v as DelegateTier } : 'tier: auto | easy | normal | hard'
    case 'agent':
    case 'model':
    case 'provider':
      return { ...s, [key]: clear ? '' : v }
    case 'role':
      return { ...s, role: v.toLowerCase() === 'auto' || v === '' ? '' : v }
    case 'effort':
      if (clear) return { ...s, effort: '' }
      return EFFORTS.includes(v as DelegateEffort) ? { ...s, effort: v as DelegateEffort } : 'effort: low | medium | high | xhigh | max | any'
    case 'priority':
      if (clear) return { ...s, priority: '' }
      return PRIORITIES.includes(v as DelegatePriority) ? { ...s, priority: v as DelegatePriority } : 'priority: speed | balanced | quality | any'
    case 'worktree':
    case 'worktree-mode':
      if (clear) return { ...s, worktree: '' }
      return WORKTREES.includes(v as DelegateWorktree) ? { ...s, worktree: v as DelegateWorktree } : 'worktree: worktree | in-place | default'
    case 'relax':
    case 'readonly':
      return { ...s, [key]: ['on', 'true', 'yes', '1'].includes(v.toLowerCase()) }
    default:
      return 'keys: tier, agent, model, provider, role, effort, priority, worktree, relax, readonly (or: reset)'
  }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await save($, migrate(await $.store.get(STORE_KEY)))
    void refresh($)
    await $.command.register({
      name: 'delegate',
      description: 'Dispatch delegation: off | default | mandatory (no argument opens the pane)',
    })
    await $.command.register({
      name: 'delegate-set',
      description: 'Set one routing option: tier|agent|model|provider|role|effort|priority|worktree|relax|readonly <value>, or reset',
    })
    await $.command.register({
      name: 'delegate-model',
      description: 'Pin a model: <model> [agent], or auto (no argument opens the pane)',
    })
    return started
  })

  on('command.run', { command: 'delegate' }, async ($, e) => {
    const s = await read($, settings)
    if (!e.args.trim()) {
      await $.ui.open({ id: PANE, title: 'Dispatch' })
      void refresh($)
      return { text: `Delegation: ${MODE_LABEL[s.mode]} · ${routeLabel(s)}` }
    }
    const mode = parseMode(e.args)
    if (!mode) return { text: 'Usage: /delegate off | default | mandatory' }
    await save($, { ...s, mode })
    return { text: `Delegation is now ${MODE_LABEL[mode]}` }
  })

  on('command.run', { command: 'delegate-set' }, async ($, e) => {
    const s = await read($, settings)
    const [key = '', ...rest] = e.args.trim().split(/\s+/)
    if (key === 'reset') {
      await save($, { ...DEFAULTS, mode: s.mode })
      return { text: 'Routing reset to Auto.' }
    }
    const next = applySet(s, key.toLowerCase(), rest.join(' '))
    if (typeof next === 'string') return { text: `Usage: /delegate-set ${next}` }
    await save($, next)
    return { text: `Route is now ${routeLabel(next)}` }
  })

  on('command.run', { command: 'delegate-model' }, async ($, e) => {
    const s = await read($, settings)
    const [first = '', agentArg] = e.args.trim().split(/\s+/)
    if (!first) {
      await $.ui.open({ id: PANE, title: 'Dispatch' })
      void refresh($)
      return { text: `Route: ${routeLabel(s)}` }
    }
    if (first === 'auto' || first === 'any') {
      await save($, { ...s, model: '', agent: agentArg ? s.agent : '' })
      return { text: 'Model unpinned.' }
    }
    const cat = await refresh($)
    const owners = cat.agents.filter(a => a.models.includes(first)).map(a => a.agent)
    if (owners.length === 0) return { text: `${first} is not enabled in Dispatch. Run /delegate-model to see the list.` }
    if (agentArg && !owners.includes(agentArg)) return { text: `${first} is enabled on: ${owners.join(', ')}` }
    const next = { ...s, model: first, agent: agentArg ?? '' }
    await save($, next)
    return { text: `Route is now ${routeLabel(next)}` }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button, Select } = $.ui.resolve(e)
    const s = await read($, settings)
    const cat = await read($, catalog)
    const set = (patch: Partial<DelegateSettings>) => void save($, { ...s, ...patch })

    const agents = cat.agents.filter(a => !s.provider || a.provider === s.provider)
    const providers = [...new Set(cat.agents.map(a => a.provider).filter(Boolean))].sort()
    const modelPool = s.agent ? agents.filter(a => a.agent === s.agent) : agents
    const modelOwners = new Map<string, string[]>()
    for (const a of modelPool) for (const m of a.models) modelOwners.set(m, [...(modelOwners.get(m) ?? []), a.agent])
    const models = [...modelOwners.keys()].sort()

    const chips = <T extends string>(
      id: string,
      values: readonly T[],
      current: T,
      label: (v: T) => string,
      onPick: (v: T) => void,
    ) => (
      <Box flexDirection="row" gap="xs" flexWrap="wrap">
        {values.map(v => (
          <Button
            key={`${id}-${v || 'any'}`}
            variant={current === v ? 'primary' : 'secondary'}
            onPress={() => onPick(v)}
          >
            {label(v)}
          </Button>
        ))}
      </Box>
    )
    const section = (title: string, hint: string | undefined, body: unknown) => (
      <Box flexDirection="column" gap="xs">
        <Text bold>{title}</Text>
        {body as never}
        {hint ? <Text dimColor>{hint}</Text> : null}
      </Box>
    )
    const toggle = (id: 'relax' | 'readonly', label: string) => (
      <Button key={`toggle-${id}`} variant={s[id] ? 'primary' : 'secondary'} onPress={() => set({ [id]: !s[id] })}>
        {s[id] ? `✓ ${label}` : label}
      </Button>
    )
    const flags = flagList(s)
    const verdict = await read($, check)
    const agentEfforts = s.agent ? effortsOf(cat, s.agent) : 'any'
    const effortChoices =
      agentEfforts === 'any' || agentEfforts === 'none'
        ? EFFORTS
        : EFFORTS.filter(v => v === '' || agentEfforts.includes(v))
    const isFresh = verdict.flags === flagText(flags)

    return (
      <Box flexDirection="column" gap="md" paddingY="sm">
        <Box flexDirection="column" borderStyle="round" paddingX="md" paddingY="sm" gap="xs">
          <Box flexDirection="row" justifyContent="space-between">
            <Text dimColor>Delegation</Text>
            <Text bold color={s.mode === 'off' ? undefined : 'success'}>
              {MODE_LABEL[s.mode]}
            </Text>
          </Box>
          <Text dimColor>Every run goes out as</Text>
          <Text bold>{flags.length ? `dispatch run ${flagText(flags)}` : 'dispatch run (Auto)'}</Text>
          {!isFresh || verdict.status === 'checking' || verdict.status === 'idle' ? (
            <Text dimColor>Checking the route…</Text>
          ) : verdict.status === 'ok' ? (
            <Text color="success">
              {verdict.winner === 'Auto' ? '✓ Dispatch picks per task' : `✓ Would run on ${verdict.winner}`}
            </Text>
          ) : (
            <Text color="error">✗ No model fits: {verdict.reason}</Text>
          )}
          {isFresh && verdict.status === 'ok' && s.tier === 'auto' && verdict.winner !== 'Auto' ? (
            <Text dimColor>Checked on the normal table; Auto may pick another fit per task.</Text>
          ) : null}
        </Box>

        {section('Mode', MODE_HINT[s.mode], chips('mode', MODES, s.mode, m => MODE_LABEL[m], m => set({ mode: m })))}

        {section(
          'Tier',
          s.tier === 'auto' ? 'Auto lets Dispatch pick the cheapest model likely to succeed.' : 'Routes that tier’s table: its first model, then the fallbacks.',
          chips('tier', TIERS, s.tier, cap, t => set({ tier: t })),
        )}

        {section(
          'Provider',
          'Narrows every candidate to one provider.',
          <Select
            key="provider"
            label="Provider"
            value={s.provider || ANY}
            options={[{ value: ANY, label: 'Any provider' }, ...providers.map(p => ({ value: p, label: p }))]}
            onSelect={v => {
              const provider = v === ANY ? '' : v
              const keepAgent = !s.agent || cat.agents.some(a => a.agent === s.agent && (!provider || a.provider === provider))
              set({ provider, agent: keepAgent ? s.agent : '', model: keepAgent ? s.model : '' })
            }}
          />,
        )}

        {section(
          'Agent',
          s.agent ? 'Only this CLI runs it; a rate limit ends the run instead of falling back.' : 'Any installed agent can take the run.',
          <Select
            key="agent"
            label="Agent"
            value={s.agent || ANY}
            options={[
              { value: ANY, label: 'Any agent' },
              ...agents.map(a => ({ value: a.agent, label: a.provider ? `${a.agent} · ${a.provider}` : a.agent })),
            ]}
            onSelect={v => {
              const agent = v === ANY ? '' : v
              const keepModel = !s.model || !agent || cat.agents.some(a => a.agent === agent && a.models.includes(s.model))
              const ef = agent ? effortsOf(cat, agent) : 'any'
              const keepEffort = !s.effort || ef === 'any' || (ef !== 'none' && ef.includes(s.effort))
              set({ agent, model: keepModel ? s.model : '', effort: keepEffort ? s.effort : '' })
            }}
          />,
        )}

        {section(
          'Model',
          s.model && !s.agent ? 'Runs on every agent that serves it, the one with most quota first.' : undefined,
          <Select
            key="model"
            label="Model"
            value={s.model || ANY}
            options={[
              { value: ANY, label: 'Any model' },
              ...models.map(m => ({ value: m, label: s.agent ? m : `${m} · ${modelOwners.get(m)!.join(', ')}` })),
            ]}
            onSelect={v => set({ model: v === ANY ? '' : v })}
          />,
        )}

        {section(
          'Role',
          s.role === '' ? 'Dispatch picks the role from the task.' : s.role === NO_ROLE ? 'No role and no pick.' : undefined,
          <Select
            key="role"
            label="Role"
            value={s.role || ANY}
            options={[
              { value: ANY, label: 'Auto pick' },
              { value: NO_ROLE, label: 'No role' },
              ...cat.roles.map(r => ({ value: r, label: cap(r) })),
            ]}
            onSelect={v => set({ role: v === ANY ? '' : v })}
          />,
        )}

        {agentEfforts === 'none'
          ? section('Effort', `${s.agent} has no effort control; Dispatch runs it as is.`, null)
          : section(
              'Effort',
              s.agent ? `What ${s.agent} accepts.` : 'An agent that lacks this effort is passed over.',
              chips('effort', effortChoices, s.effort, v => (v ? v : 'Any'), v => set({ effort: v })),
            )}

        {section(
          'Priority',
          'How sure Auto must be: speed 0.70, balanced 0.80, quality 0.90.',
          chips('priority', PRIORITIES, s.priority, v => (v ? cap(v) : 'Default'), v => set({ priority: v })),
        )}

        {section(
          'Workspace',
          'Worktree runs in parallel on its own branch; in-place edits the folder itself.',
          chips('worktree', WORKTREES, s.worktree, v => (v ? cap(v) : 'Default'), v => set({ worktree: v })),
        )}

        {section(
          'Options',
          'Relax falls back to tier routing when the filters match nothing.',
          <Box flexDirection="row" gap="xs" flexWrap="wrap">
            {toggle('relax', 'Relax filters')}
            {toggle('readonly', 'Read-only')}
          </Box>,
        )}

        <Box flexDirection="row" gap="xs" justifyContent="space-between" alignItems="center">
          <Text dimColor>
            {cat.error
              ? `Could not read Dispatch: ${cat.error}`
              : cat.agents.length === 0
                ? 'Loading Dispatch…'
                : `${cat.agents.reduce((n, a) => n + a.models.length, 0)} models on ${cat.agents.length} agents`}
          </Text>
          <Box flexDirection="row" gap="xs">
            <Button key="refresh" plain dimColor onPress={() => void refresh($)}>
              Refresh
            </Button>
            <Button key="reset" plain dimColor onPress={() => void save($, { ...DEFAULTS, mode: s.mode })}>
              Reset
            </Button>
          </Box>
        </Box>
      </Box>
    )
  })

  on('prompt.compose', async ($, e, next) => {
    const composed = await next(e)
    const s = await read($, settings)
    if (s.mode === 'off') return composed
    return {
      sections: [...composed.sections, { id: 'dispatch-delegate:policy', text: promptText(s), scope: 'session' as const }],
    }
  })

  on('tool.call', async ($, e, next) => {
    if (e.agentId !== undefined) return next(e)
    const s = await read($, settings)
    if (s.mode === 'off') return next(e)

    if (e.tool === 'Bash') {
      const cmd = e.command
      // Only where a command starts (line start, or after ; & | ( ): text that
      // merely mentions it, such as a heredoc body or a commit message, stays.
      const RUN = /(^|[\n;&|(])\s*dispatch\s+run\b/
      if (!RUN.test(cmd)) return next(e)
      // A flag the command passes itself wins over the pane's.
      const all = flagList(s)
      const missing = all.filter(([n]) => !new RegExp(`--${n}(\\b|=)`).test(cmd))
      // The command overrides nothing: the pane's flags stand alone, so a
      // failed check means this run fails too. Say why before it starts.
      if (missing.length === all.length && all.length > 0) {
        const verdict = await read($, check)
        if (verdict.status === 'fail' && verdict.flags === flagText(all)) {
          return {
            deny: `dispatch-delegate: the routing the person chose (${flagText(all)}) leaves no model: ${verdict.reason}. Tell the person, or override a flag on this run (passing --agent, --model, --effort, etc. yourself replaces the pane's).`,
          }
        }
      }
      if (missing.length === 0) return next(e)
      return next({ ...e, command: cmd.replace(RUN, m => `${m} ${flagText(missing)}`) })
    }

    if (s.mode === 'mandatory' && WRITERS.has(e.tool)) {
      const path = String(
        (e as { file_path?: string }).file_path ?? (e as { notebook_path?: string }).notebook_path ?? '',
      )
      if (!WRITABLE.test(path)) {
        return {
          deny: 'dispatch-delegate: mandatory delegation is on. Delegate this change with `dispatch run` (see the dispatch skill). The person can run /delegate default to allow direct edits.',
        }
      }
    }
    return next(e)
  }).catch(($, e, next) => next(e))
}
