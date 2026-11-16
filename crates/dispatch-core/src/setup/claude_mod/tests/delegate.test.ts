import { expect, test } from 'claude-code/testing'

// What version 0.1 stored: the old `{ mode, route }` shape.
const OLD = { mode: 'mandatory', route: { kind: 'model', model: 'gpt-6-luna', agent: 'codex' } }
const FULL = {
  mode: 'default', tier: 'hard', agent: 'codex', model: 'gpt-6-luna', provider: 'openai',
  role: 'reviewer', effort: 'high', priority: 'quality', worktree: 'in-place', relax: true, readonly: true,
}
const MODELS = JSON.stringify({
  agents: [
    { agent: 'codex', models: [{ id: 'gpt-6-luna', enabled: true }] },
    { agent: 'opencode', models: [{ id: 'opencode-go/deepseek-v4.1-flash', enabled: true }] },
  ],
})
const DOCTOR = JSON.stringify([
  { name: 'codex', capabilities: { effortMode: 'config', efforts: ['low', 'medium', 'high', 'xhigh'] } },
  { name: 'opencode', capabilities: { effortMode: '' } },
])
// A stand-in for `dispatch route`: opencode takes no effort.
function route(argv: readonly string[]) {
  const at = (f: string) => { const i = argv.indexOf(f); return i < 0 ? undefined : argv[i + 1] }
  if (at('--agent') === 'opencode' && at('--effort'))
    return { candidates: [], eliminated: ['codex/gpt-6-luna: agent filter requires opencode', 'opencode/opencode-go/deepseek-v4.1-flash: effort unsupported by agent (no effort control)'] }
  const agent = at('--agent') ?? 'codex'
  return { candidates: [{ Agent: agent, Model: agent === 'opencode' ? 'opencode-go/deepseek-v4.1-flash' : 'gpt-6-luna', Effort: at('--effort') ?? '' }] }
}

function engine(on: any, stored: unknown, seen: string[]) {
  on('command.register', () => ({ value: { command: 'x' } }))
  on('store.get', () => ({ value: stored }))
  on('store.set', () => ({ value: undefined }))
  on('process.run', (_: unknown, e: any) => {
    const sub = e.argv[1]
    const out = sub === 'models' ? MODELS : sub === 'doctor' ? DOCTOR : sub === 'route' ? JSON.stringify(route(e.argv)) : '{}'
    return { value: { exitCode: 0, stdout: out, stderr: '' } }
  })
  on('session.start', (_: unknown, e: any) => ({ cwd: e.cwd }))
  on('tool.call', (_: unknown, e: any) => {
    if (e.tool === 'Bash') seen.push(e.command)
    return { result: {} }
  })
}

test('old settings migrate; mandatory mode rewrites runs and blocks edits', async ($, on) => {
  const seen: string[] = []
  engine(on, OLD, seen)
  await $.session.start({ cwd: '/', surface: 'desktop', isInteractive: true })

  await $.tool.call({ tool: 'Bash', command: "dispatch run --card a - <<'B'\nx\nB" })
  expect(seen[0]).toContain('dispatch run --agent codex --model gpt-6-luna --card a')

  // A flag the command passes itself wins; the rest are still added.
  await $.tool.call({ tool: 'Bash', command: 'dispatch run --model other -' })
  expect(seen[1]).toBe('dispatch run --agent codex --model other -')

  const blocked = await $.tool.call({ tool: 'Edit', file_path: '/repo/src/a.ts', old_string: 'a', new_string: 'b' })
  expect(blocked.deny).toContain('mandatory delegation')

  const memory = await $.tool.call({ tool: 'Write', file_path: '/Users/me/.claude/projects/p/memory/x.md', content: 'x' })
  expect(memory.deny).toBeUndefined()
})

test('every routing option becomes a flag', async ($, on) => {
  const seen: string[] = []
  engine(on, FULL, seen)
  await $.session.start({ cwd: '/', surface: 'desktop', isInteractive: true })
  await $.tool.call({ tool: 'Bash', command: 'dispatch run -' })
  expect(seen[0]).toBe(
    'dispatch run --tier hard --agent codex --model gpt-6-luna --provider openai --role reviewer --effort high --priority quality --worktree-mode in-place --relax --readonly -',
  )
  // A mention inside text is not a run; one after && is.
  const note = "cat >> README.md <<'EOF'\nA `dispatch run` uses the pane's flags.\nEOF"
  await $.tool.call({ tool: 'Bash', command: note })
  expect(seen[1]).toBe(note)
  await $.tool.call({ tool: 'Bash', command: 'cd repo && dispatch run --tier easy -' })
  expect(seen[2]).toContain('cd repo && dispatch run --agent codex')
  seen.splice(1, 2)
  await $.tool.call({ tool: 'Bash', command: 'dispatch wait abc' })
  expect(seen[1]).toBe('dispatch wait abc')
})

test('the pane draws on every surface', async ($, on) => {
  engine(on, FULL, [])
  await $.session.start({ cwd: '/', surface: 'desktop', isInteractive: true })
  for (const surface of ['terminal', 'desktop', 'mobile'] as const) {
    const ui = await $.ui.mount({
      plugin: 'dispatch-delegate', component: 'Pane', requestId: 'dispatch-delegate', surface,
      props: { title: 'Dispatch', isFocused: false, bodyColumns: 40, placement: 'dock', scroll: { offset: 0, bodyRows: 60 }, view: {} },
    })
    expect((await ui.find({ type: 'Text', text: /^dispatch run --tier hard/ }))?.text).toContain('--readonly')
    await ui.unmount()
  }
})

const OPENCODE_HIGH = { ...FULL, mode: 'default', tier: 'auto', agent: 'opencode', model: '', provider: '', role: '', effort: 'high', priority: '', worktree: '', relax: false, readonly: false }
const PANE_PROPS = { title: 'Dispatch', isFocused: false, bodyColumns: 40, placement: 'dock', scroll: { offset: 0, bodyRows: 60 }, view: {} } as const

test('an impossible combination is shown and its run is stopped with the reason', async ($, on) => {
  const seen: string[] = []
  engine(on, OPENCODE_HIGH, seen)
  await $.session.start({ cwd: '/', surface: 'desktop', isInteractive: true })

  const ui = await $.ui.mount({ plugin: 'dispatch-delegate', component: 'Pane', requestId: 'dispatch-delegate', surface: 'desktop', props: PANE_PROPS })
  const bad = await ui.find({ type: 'Text', text: /No model fits/ })
  expect(bad?.text).toContain('effort unsupported')
  expect(bad?.text).not.toContain('agent filter requires')
  await ui.unmount()

  const ran = await $.tool.call({ tool: 'Bash', command: 'dispatch run -' })
  expect(ran.deny).toContain('leaves no model')
  expect(seen.length).toBe(0)

  // Overriding a flag on the run lets it through.
  await $.tool.call({ tool: 'Bash', command: 'dispatch run --agent codex -' })
  expect(seen[0]).toBe('dispatch run --effort high --agent codex -')
})

test('a fitting combination names the model it would use', async ($, on) => {
  engine(on, { ...OPENCODE_HIGH, effort: '' }, [])
  await $.session.start({ cwd: '/', surface: 'desktop', isInteractive: true })
  const ui = await $.ui.mount({ plugin: 'dispatch-delegate', component: 'Pane', requestId: 'dispatch-delegate', surface: 'desktop', props: PANE_PROPS })
  expect((await ui.find({ type: 'Text', text: /Would run on/ }))?.text).toBe('✓ Would run on opencode-go/deepseek-v4.1-flash · opencode')
  expect((await ui.find({ type: 'Text', text: /no effort control/ }))?.text).toContain('opencode has no effort control')
  await ui.unmount()
})
