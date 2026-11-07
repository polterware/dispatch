import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js"
import type { Catalog, CatalogEntry, EngineSettings, Language, SettingsPatch, VoiceEntry } from "@local-voice/client"
import { formatBytes, type EngineLink } from "./engine-api.ts"

/** The slice of `ctx.ui` this page renders with. */
export interface SettingsUi {
  SettingsLayout(props: { title: string; description?: string; children: any }): any
  SettingsSection(props: { title?: string; children: any; footer?: any }): any
  SettingsRow(props: { title: any; description?: any; id?: string; children?: any }): any
  Select(props: { options: readonly { value: string; label: string }[]; value: string | undefined; onChange: (value: string) => void; label: string; placeholder?: string; disabled?: boolean }): any
  Switch(props: { checked: boolean; onChange: (checked: boolean) => void; label: string; hideLabel?: boolean }): any
}

export interface SettingsPageProps {
  ui: SettingsUi
  engine: EngineLink
  theme: () => "light" | "dark"
  autoRead: () => boolean
  setAutoRead: (value: boolean) => void
  confirm: (input: { title: string; description?: string; confirm: string; danger?: boolean }) => Promise<boolean>
  notify: (title: string, error?: boolean) => void
  /** Speaks a short sample with the given voice. */
  test: (voice: string, language: Language) => void
}

const LANGUAGES: { value: Language; label: string }[] = [
  { value: "pt-br", label: "Português (Brasil)" },
  { value: "en", label: "English" },
  { value: "auto", label: "Detect automatically" },
]
const SPEEDS = ["0.8", "0.9", "1", "1.1", "1.25", "1.5"]
const KIND_TITLE = { stt: "Speech recognition", tts: "Voices", vad: "Voice activity detection" } as const

export function VoiceSettings(props: SettingsPageProps) {
  const ui = props.ui
  const [catalog, setCatalog] = createSignal<Catalog>()
  const [settings, setSettings] = createSignal<EngineSettings>()
  const [error, setError] = createSignal<string>()
  const [progress, setProgress] = createSignal<Record<string, number>>({})
  const [busy, setBusy] = createSignal<string>()

  const load = async () => {
    try {
      const [nextCatalog, nextSettings] = await Promise.all([props.engine.catalog(), props.engine.settings()])
      setCatalog(nextCatalog)
      setSettings(nextSettings)
      setError(undefined)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  onMount(() => {
    let release: (() => void) | undefined
    let disposed = false
    void props.engine
      .acquire()
      .then(done => {
        if (disposed) done()
        else release = done
        return load()
      })
      .catch(cause => setError(`Voice engine unavailable: ${cause instanceof Error ? cause.message : String(cause)}`))
    const unsubscribe = props.engine.subscribe(message => {
      switch (message.type) {
        case "model.progress":
          setProgress(current => ({ ...current, [message.id]: message.total_bytes ? message.downloaded_bytes / message.total_bytes : 0 }))
          break
        case "model.installed":
        case "model.removed":
          setProgress(current => {
            const next = { ...current }
            delete next[message.id]
            return next
          })
          void load()
          break
        case "model.failed":
          setProgress(current => {
            const next = { ...current }
            delete next[message.id]
            return next
          })
          props.notify(`Download failed: ${message.message}`, true)
          break
        case "settings.changed":
          setSettings(message.settings)
          void load()
          break
      }
    })
    onCleanup(() => {
      disposed = true
      unsubscribe()
      release?.()
    })
  })

  const patch = async (change: SettingsPatch) => {
    try {
      setSettings(await props.engine.patchSettings(change))
      void load()
    } catch (cause) {
      props.notify(cause instanceof Error ? cause.message : String(cause), true)
    }
  }

  const voicesFor = (language: Language) =>
    (catalog()?.voices ?? []).filter(voice => voice.language === language)
  const voiceLabel = (voice: VoiceEntry) => (voice.installed ? voice.name : `${voice.name} — not downloaded`)
  const sttModels = createMemo(() => (catalog()?.models ?? []).filter(model => model.kind === "stt"))
  const groups = createMemo(() => {
    const models = catalog()?.models ?? []
    return (["stt", "tts", "vad"] as const).map(kind => ({ kind, models: models.filter(model => model.kind === kind) })).filter(group => group.models.length)
  })

  const download = async (model: CatalogEntry) => {
    const ok = await props.confirm({
      title: `Download ${model.name}?`,
      description: `${formatBytes(model.size_bytes)} from the model's public host. License: ${model.license}. Runs fully offline afterwards.`,
      confirm: "Download",
    })
    if (!ok) return
    setBusy(model.id)
    try {
      await props.engine.download(model.id)
      setProgress(current => ({ ...current, [model.id]: 0 }))
    } catch (cause) {
      props.notify(cause instanceof Error ? cause.message : String(cause), true)
    } finally {
      setBusy(undefined)
    }
  }

  const remove = async (model: CatalogEntry) => {
    const ok = await props.confirm({ title: `Delete ${model.name}?`, description: `Frees ${formatBytes(model.size_bytes)}. You can download it again later.`, confirm: "Delete", danger: true })
    if (!ok) return
    setBusy(model.id)
    try {
      await props.engine.remove(model.id)
      await load()
    } catch (cause) {
      props.notify(cause instanceof Error ? cause.message : String(cause), true)
    } finally {
      setBusy(undefined)
    }
  }

  const voiceRow = (title: string, language: Language, key: "voice_pt" | "voice_en") => (
    <ui.SettingsRow id={key} title={title} description="Missing voices download from the Models section below.">
      <div class="lv-inline">
        <ui.Select
          label={title}
          options={voicesFor(language).map(voice => ({ value: voice.id, label: voiceLabel(voice) }))}
          value={settings()?.[key]}
          onChange={value => void patch({ [key]: value })}
        />
        <button class="lv-btn" onClick={() => { const voice = settings()?.[key]; if (voice) props.test(voice, language) }} disabled={!settings()}>
          Test
        </button>
      </div>
    </ui.SettingsRow>
  )

  return (
    <div class="lv" data-theme={props.theme()}>
      <ui.SettingsLayout title="Local Voice" description="Speak to your coding agent and hear its replies. Everything runs on this computer.">
        <Show when={error()}>
          <ui.SettingsSection>
            <ui.SettingsRow title="Voice engine unavailable" description={error()}>
              <button class="lv-btn" onClick={() => void load()}>Retry</button>
            </ui.SettingsRow>
          </ui.SettingsSection>
        </Show>
        <ui.SettingsSection title="General">
          <ui.SettingsRow id="language" title="Language" description="The language you speak and want to hear.">
            <ui.Select label="Language" options={LANGUAGES} value={settings()?.language} onChange={value => void patch({ language: value as Language })} disabled={!settings()} />
          </ui.SettingsRow>
          <ui.SettingsRow id="auto-read" title="Read replies aloud" description="While voice mode is on, read every finished reply. Turn off to only read messages you pick.">
            <ui.Switch label="Read replies aloud" hideLabel checked={props.autoRead()} onChange={props.setAutoRead} />
          </ui.SettingsRow>
          <ui.SettingsRow id="speed" title="Speaking speed">
            <ui.Select
              label="Speaking speed"
              options={SPEEDS.map(value => ({ value, label: `${value}×` }))}
              value={settings() ? String(Number(settings()!.speed.toFixed(2))) : undefined}
              onChange={value => void patch({ speed: Number(value) })}
              disabled={!settings()}
            />
          </ui.SettingsRow>
          <ui.SettingsRow id="partials" title="Live transcript" description="Show words while you speak (uses a little more processing).">
            <ui.Switch label="Live transcript" hideLabel checked={settings()?.partials ?? true} onChange={value => void patch({ partials: value })} />
          </ui.SettingsRow>
        </ui.SettingsSection>
        <ui.SettingsSection title="Voices">
          {voiceRow("Portuguese voice", "pt-br", "voice_pt")}
          {voiceRow("English voice", "en", "voice_en")}
        </ui.SettingsSection>
        <ui.SettingsSection title="Speech recognition">
          <ui.SettingsRow id="stt" title="Model" description="Larger models are more accurate and slower.">
            <ui.Select
              label="Speech recognition model"
              options={sttModels().map(model => ({ value: model.id, label: model.installed ? model.name : `${model.name} — not downloaded` }))}
              value={settings()?.stt_model}
              onChange={value => void patch({ stt_model: value })}
              disabled={!settings()}
            />
          </ui.SettingsRow>
        </ui.SettingsSection>
        <For each={groups()}>
          {group => (
            <ui.SettingsSection title={`Models · ${KIND_TITLE[group.kind]}`}>
              <For each={group.models}>
                {model => (
                  <ui.SettingsRow
                    title={
                      <span class="lv-model-title">
                        {model.name}
                        <Show when={model.active}>
                          <span class="lv-tag">In use</span>
                        </Show>
                      </span>
                    }
                    description={`${formatBytes(model.size_bytes)} · ${model.license} · ${model.description}`}
                  >
                    <Show
                      when={progress()[model.id] === undefined}
                      fallback={
                        <span class="lv-progress" role="progressbar" aria-valuenow={Math.round((progress()[model.id] ?? 0) * 100)}>
                          <span style={{ width: `${Math.round((progress()[model.id] ?? 0) * 100)}%` }} />
                        </span>
                      }
                    >
                      <Show
                        when={model.installed}
                        fallback={
                          <button class="lv-btn" onClick={() => void download(model)} disabled={busy() === model.id}>
                            Download
                          </button>
                        }
                      >
                        <button class="lv-btn lv-btn--danger" onClick={() => void remove(model)} disabled={model.active || busy() === model.id} title={model.active ? "In use by the current settings" : undefined}>
                          Delete
                        </button>
                      </Show>
                    </Show>
                  </ui.SettingsRow>
                )}
              </For>
            </ui.SettingsSection>
          )}
        </For>
      </ui.SettingsLayout>
    </div>
  )
}
