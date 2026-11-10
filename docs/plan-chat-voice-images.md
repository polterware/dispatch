# Plan: voice and images in the chat

Two features that share one idea: the app is also a chat. Conversations in the app can be
spoken to and listened to, and any run, started in the app or delegated by an agent, can
produce images that land in one gallery that says where each came from.

Order: 0 → 1 → 2. Phase 0 is shared groundwork; 1 and 2 can then go in parallel only
where they touch different files (both touch the sidebar and Settings, so those edits
land one at a time).

## Phase 0: groundwork

- **Fix the repository picker's row menu** (New page, repository pill → a row's "…",
  built from `views::repository_actions::menu_items` as the option's row menu). Today the
  menu opens detached from its row: it hangs from the window's top edge, so its first rows
  (Reveal in Finder, Copy path, Open in Cursor) are cut off, it covers the picker's own
  list, and the row's path tooltip opens beside it at the same time. Expected: it hangs
  from the "…" it came from (polterware-ui `window_anchor` from that button's
  `AnchorSlot`), flips up only when there is no room below, stays inside the window, and
  the tooltip hides while the menu is open. Also check the picker's last row ("Add
  repository") is fully visible and not cut by the list's end fade. If the bug is in the
  shared menu or picker, fix it in polterware-ui (AGENTS.md), not in Dispatch. Add a
  `ui_flows` test that opens a row's menu and checks its bounds against the row and window.

- **Conversation origin** (the gallery's App / Delegated filter needs it). A conversation started from the app's New page gets the
  caller agent `dispatch` and a conversation id of its own (`dispatch:<uuid>`), sent in
  `StartRunRequest.caller`; follow-ups keep it (they already inherit the conversation).
  Verify what the app sends today and what older app-started runs hold (likely an empty
  conversation): a migration-free rule reads "started in the app" as caller agent
  `dispatch`, or no caller at all with `StartRunRequest` coming from the app (record it).
- **Run artifacts.** Each run gets a folder of its own under the data dir
  (`artifacts/<run id>/`), exported to the agent as `DISPATCH_ARTIFACTS_DIR` and named in
  the brief. When a run ends, the core indexes image files from that folder and from the
  run's changed files (worktree diff, by extension and magic bytes) into a new
  `dispatch_artifacts` table (run, conversation, caller agent, agent, model, path, kind,
  size, dimensions, created). Copies live in the data dir so a cleaned worktree keeps its
  images. RPCs `ListArtifacts` (filters: origin, repository, agent, conversation; paged)
  and `DeleteArtifact`. Deleting a run deletes its artifacts.

## Phase 1: voice in the chat

The engine comes from `~/codes/polterware-workspace/voice` (CPAL audio, whisper.cpp on
Metal, Piper/Kokoro/Chatterbox TTS on ONNX Runtime, Silero VAD, local HTTP + WebSocket API,
token in a 0600 discovery file, 127.0.0.1 only).

- **Crates.** Move `voice-core`, `voice-audio`, `voice-stt`, `voice-tts`, `voice-server`
  into `crates/`. The engine builds as its own binary `dispatch-voice` (models take
  hundreds of MB; the core and app stay lean), bundled in `Contents/Resources` by
  `release/bundle.sh`. Keep the clang_rt link and `MACOSX_DEPLOYMENT_TARGET=13.4` ORT needs.
  Models in `~/Library/Application Support/Polterware Dispatch/voice/models` (moved from
  `voice-engine/models` once).
- **Lifecycle.** The app starts it on demand like `start_core`; it unloads models after
  10 min and exits after 30 min idle. `Dispatch.app` declares `NSMicrophoneUsageDescription`
  (macOS attributes the engine's capture to the app that launched it).
- **App client** `app/src/voice.rs`: one WebSocket on a background thread; levels kept out
  of entity notifies and read by the blob each frame.
- **Talk.** A mic action in the composer (New and run pages), added to polterware-ui's
  `message_composer` as a general trailing action. Tap starts, tap ends; the transcript
  goes into the composer, or is sent at once if "Send when I stop" is on.
- **Hear.** When a run of a voiced turn ends COMPLETED, its stored reply goes to `speak`
  (markdown: code, diffs, tables, long paths dropped). Talking again cancels speech. A
  play action on each reply in the transcript reads it on demand.
- **Settings › Voice:** language (pt-BR default), voice per language with Test, speed,
  live transcript, read replies aloud, send when I stop, STT model, and the model list with
  Download (size, license, progress) and Delete (refused while in use).
- **Blob** next to the composer while listening, working, or speaking. Check whether gpui's
  `runtime_shaders` allows the shader blob; otherwise a path-drawn deforming disc.
- Defaults: Piper `pt_BR-jeff-medium`; Chatterbox listed as experimental (today on an M1:
  58 s load, RTF 3.7, 1.26 GB RSS); `faber` hidden until its ORT crash is fixed.

## Phase 2: images

- **Survey first (read-only).** For every installed agent CLI, check whether it can
  generate an image and save it to a given folder, with which models, and what it costs:
  Codex 0.164, OpenCode 2.2 (its model list has `type: image`), Gemini/Antigravity,
  Cursor, Claude Code. The result becomes a capability `images` per agent/model in the
  catalog (benchmarks do not rate images; Auto picks by price and quota among capable
  ones, or the user names the model).
- **From the chat.** The composer gets an Image mode (a pill beside the route) that routes
  only to image-capable agents and tells the agent to write into `DISPATCH_ARTIFACTS_DIR`;
  without it, a normal run that happens to produce images still shows them. The transcript
  renders a run's images inline under its reply (thumbnails, click to open, Reveal in
  Finder, Copy).
- **From delegation.** Nothing special is needed: an agent that delegates an image task
  with `dispatch run` gets the same folder; the skill and `dispatch run --images` (routes to
  capable agents) document it.
- **Images page** (sidebar, beside Spend and Stats): a gallery of every artifact, newest
  first, each with where it came from: conversation (app or the calling agent's bot and
  session), run, agent and model, repository, date. Filters: origin (App / Delegated),
  agent, repository; search over the prompt. Click opens a viewer with the prompt and a
  link to the run. Delete removes the file.
- **Spend** learns per-image prices where the CLI or catalog gives them.

## Tests

Core e2e (fake ACP agent): app-origin runs marked as such; an agent writing
a PNG into the artifacts folder and one committing a PNG are both indexed; delete cascades.
Voice: engine tests come along; app e2e with a fake engine (talk → composer, run end →
speak). `ui_flows`: repository row menu, Images page, Settings › Voice.
`bash tooling/verify.sh` after each phase.
