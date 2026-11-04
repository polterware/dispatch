# dispatch-app

The Dispatch desktop app in GPUI (Rust), and the one binary `Dispatch.app` ships: given a command, or run as `dispatch` (`Contents/Resources/dispatch` links to it), `src/main.rs` hands the arguments to `dispatch_core::cli` (the CLI and the core, `dispatch serve`) instead of opening the window.

The window attaches to a running Dispatch core through `<data_dir>/run/core.json` (`$PWDISPATCH_DATA_DIR`, defaulting to `~/Library/Application Support/Polterware Dispatch`) and speaks Connect-JSON to it, `WatchRun` streaming included. It does not start the core: any `dispatch` command does. Shared controls, motion, the bots, and the window chrome come from [polterware-ui](https://github.com/polterware/ui), whose gallery (`cargo run -p polterware-gallery` there) is their catalog and visual test.

## Build & Test

```sh
cargo test -p dispatch-app
cargo build --release -p dispatch-app
```

## Run

```sh
cargo build -p dispatch-core --bin dispatch && ./target/debug/dispatch serve &
cargo run -p dispatch-app
```

A build run this way has no bundle, so it sets its Dock icon itself (`src/dock_icon.rs`, from `resources/dispatch-icon.png`) and keeps the Sparkle updater off.

## Screenshots

`DISPATCH_GPUI_SHOTS=/tmp/gpui-shots cargo run --release` visits every screen, saves a PNG of the app's own window for each (no Screen Recording permission needed), and quits.

## Differences from the Electron app


What GPUI cannot draw and what is not ported yet, so the pixels differ on purpose:

- **Motion.** GPUI cannot scale an element, so menus and popovers enter with the fade and the 4px slide but without their slight vertical scale. It has no CSS filters either, so dimmed and lifted tier cards use opacity instead of grayscale and brightness.
- **Masks.** The rail's content mask (the fade under its top and bottom edges) cannot be drawn, so the rail clips instead.
- **Inputs.** `TextInput::bare()` stands in for `.pw-filter-popover-search` (no fill, edge, or focus ring); the shared input has no other bare variant.
- **Type.** Code uses Menlo where Electron resolves SF Mono. GPUI's word wrap treats `-` as part of a word, so a long model id may break mid-word where Electron breaks after a hyphen, and the Intelligence footer wraps at a different point. It also counts the space after a line's last word toward the width, where Chromium lets the space hang, so a line that fills the measure to the last character, as the run transcript's 99-column lines can, keeps one word fewer; and it may break before a closing bracket, which Chromium keeps with the word before it. Electron truncates the agent cell at a sub-pixel width, GPUI at a whole pixel.
- **Color.** Screenshots of the Electron app come out in the display's P3 profile and are converted to sRGB before they are compared with GPUI's, which `save_window_png` draws straight into an sRGB bitmap; the conversion can leave saturated accents a few levels apart.
- **Mascot.** `EmptyBot` and the bots pick their pose from a seed, so two captures of the same page can show different poses.
- **Runs table.** The runs list's columns come out a few pixels wider than Electron's (the Run header starts at 289px, not 281px, and Status at 568px, not 547px), so the table overflows its page and shows a horizontal scrollbar Electron does not. `DataTable` is being rewritten, so its column sizing waits on that.
- **Hand-rolled.** The tier palette and the role composer are still hand-rolled; they move onto the shared popover once its rewrite lands. The DropdownMenu port waits on the same rewrite.

## Bundle

`bash tooling/release/bundle.sh <version>` (from the repository root) builds `Dispatch.app`: this binary as `Contents/MacOS/Dispatch`, the `Contents/Resources/dispatch` link to it, the icon compiled from `resources/dispatch.icon`, and Sparkle.

## ARCHITECTURE

`#![allow(dead_code)]` lives in `src/lib.rs` because the shared modules export more than the views use yet. Do not add a blanket allow for unused imports or variables.

| File | Owns |
| --- | --- |
| `src/main.rs` | The one binary's entry: the CLI and the core (`dispatch_core::cli::main`) for a command, else `dispatch_app::run`. |
| `src/lib.rs` | `run`: the window (frameless, 1200×800, min 800×560), menus, key bindings, the Dock icon, the updater, opening `AppView`. |
| `src/app.rs` | `AppView`: sidebar, the route, the Commander overlay, the toast stack. One entity per view. |
| `src/theme.rs` | Dark `Theme` global: colors, radii, spacing, type, row heights. |
| `src/client.rs` | Loopback `core.json` discovery and the blocking Connect-JSON client, including `WatchRun`. |
| `src/state.rs` | `AppState`: route, snapshots, 3s poll, toasts, preference files. |
| `src/format.rs` | Shared display strings (`prompt_first_line`, `non_empty_or`, `agent_display_name`, `is_active_status`). |
| `src/updater.rs` | Sparkle from the bundle's `Contents/Frameworks`, daily checks and Check for Updates…. |
| `src/dock_icon.rs` | The Dock icon of a build with no bundle. |
| `src/shots.rs` | The screenshot tour behind `DISPATCH_GPUI_SHOTS`. |
| `src/ui/` | Re-exports polterware-ui's controls (`avatar`, `motion`, `vibrancy`, `window_drag` among them); `parts/` holds the pieces only Dispatch's pages draw. |
| `resources/` | `dispatch.icon` (Icon Composer, the bundle's icon), `dispatch Exports` (its PNG exports), and `dispatch-icon.png` (the Default export on the macOS icon grid, the Dock icon of an unbundled build). |
| `src/views/<view>.rs` | One screen each: `sidebar`, `run_page`, `transcript`, `roles`, `intelligence`, `spend`, `stats`, `settings`, `commander`, `runs`. `TranscriptView` is not a route: the run page mounts it, and the sidebar's long-hover card mounts it compact. |
| `src/models/<view>.rs` | The pure logic of that view and its tests. |
| `src/ui/parts/<view>.rs` | UI helpers only that view uses. |
| `../crates/dispatch-core` | The core and CLI, a dependency of this binary; the window never calls it, only the core over Connect. |

`navigate` and `go_back` do not take `cx`. Callers redraw themselves:

```rust
state.update(cx, |s, cx| {
    s.navigate(route);
    s.request_refresh(cx);
    cx.notify();
});
```

### Parallel work

A change to one screen stays inside its three files (`src/views/<view>.rs`, `src/models/<view>.rs`, `src/ui/parts/<view>.rs`), so several screens can change at once without conflicts. Shared modules (`app`, `state`, `client`, `theme`, `ui/*`, `lib`, `main`) change on their own, and `cargo fmt` runs only there.

### Public APIs views use

**client.** `CoreClient` (`Clone + Send`, `connect`, `endpoint`, one method per RPC taking `&Request` and returning `Result<Response, CoreError>`, `watch_run` → `Result<WatchRunIter, CoreError>`, `run_reply`). `CoreError`, `CoreConfig`, `resolve_data_dir`, `core_json_path`, `read_core_config`, `validate_loopback_endpoint`, `strip_ansi`, `ProtoI64`, `encode_envelope`, `Envelope`, `EnvelopeDecoder`, `interpret_frame`. Every request and response struct uses camelCase, `#[serde(default)]`, and `ProtoI64` for int64. `WatchRunIter` items are `Result<WatchRunResponse, CoreError>`.

**state.** `AppState::open` / `new`, `reload_all`, `request_refresh`, `navigate`, `go_back`, `client`, `push_toast`, `dismiss_toast`, `toasts`, `route`, `connection`, `last_error`, `workspaces`, `workspaces_linked`, `runs`, `sections`, `workspace_groups`, `run`, `run_reply`, `reply_pending`, `commander_open`, `toggle_commander`, `set_commander_open`, `data_dir`, `read_preference`, `write_preference`. Types: `Route`, `SettingsPage`, `ConnectionState`, `Navigation`, `WorkspaceGroup`, `Toast`, `ToastTone`. `observe(state, cx)` subscribes a view.

**theme.** `Theme` global, `Theme::dark`, `Theme::global`. Named `color_*`, `radius_*` (`sm`/`md`/`lg` plus control, item, card, menu, surface, window, dialog), `space_1`…`space_8`, `font_size_*`, `font_sans` (Inter), `font_mono` (Menlo), `font_terminal` (JetBrains Mono), `row_height` 44, `page_header_height` 44, `sidebar_row_height` 44, `sidebar_width` 272. `StatusTone`, `status_tone`, `status_color`, `tone_color`.

**ui.** polterware-ui's controls, re-exported: buttons, toolbars, `data_table`, chips, `CollapsibleSection`, `rail_group`, `rail_card`, `TextInput`, `Popover`, `Menu`, `tooltip`, `HoverCard`, `Switch`, `radio_group`, `segment_display`, toasts, icons (`IconAssets`), `motion` (springs), and `avatar` (`agent_avatar(agent, status, size_px)`, the animated bots). See that repository for their APIs.
