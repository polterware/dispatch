# dispatch-gpui

Experimental GPUI (Rust) port of the Polterware Dispatch desktop app. The Electron app in the Polterware monorepo stays the official one; this crate exists to test GPUI.

It attaches to a running Dispatch core through `<data_dir>/run/core.json` (`$PWDISPATCH_DATA_DIR`, defaulting to `~/Library/Application Support/Polterware Dispatch`) and speaks Connect-JSON to it, `WatchRun` streaming included. It does not start the core. The bots come from the sibling crate `../bot-avatar-rs`.

## Build & Test

```sh
cargo test
cargo build --release
```

## Run

```sh
cargo run --release
```

## Screenshots

`DISPATCH_GPUI_SHOTS=/tmp/gpui-shots cargo run --release` visits every screen, saves a PNG of the app's own window for each (no Screen Recording permission needed), and quits.

## Gallery

`cargo run --release --example gallery` opens every shared control in its variants and states, with the content and order of the Solid gallery (`apps/gallery` in the monorepo), each example in a box like its `.docs-preview` (920px wide, 24px padding, 16px gaps).

```sh
GALLERY_SHOTS=/tmp/compare/gallery/gpui GALLERY_SOLID=/tmp/compare/gallery/solid cargo run --release --example gallery
```

draws one example at a time, saves `<slug>/<nn>.png` at the display's scale, and quits (`GALLERY_ONLY=button,switch` limits it to those pages). Each capture takes the number the Solid gallery's `index.json` gives its title, so a page that leaves examples out still pairs each capture with its own, and one line per example prints the share of pixels whose largest channel differs by more than 24 (a pixel only one image covers counts as differing) and the mean of each pixel's largest channel difference. The Solid captures are 2x screenshots of each `.docs-preview` box under `TextureFaceProvider`; run on a Retina display with the session unlocked, since a locked session captures blank windows.

Measured at 2x on 2026-11-03 against the Solid gallery's captures. The preview box alone (its rounded corners and border) differs by about 0.4%, so that is the floor:

| Page | Examples | Differing pixels (mean) | Mean channel difference | Furthest example |
| --- | --- | --- | --- | --- |
| `button` | 8 | 3.30% | 4.47 | Texture variant (8.86%) |
| `radiogroup` | 1 | 1.06% | 1.22 | Pick one (1.06%) |
| `select` | 4 | 0.63% | 0.77 | Options with separators and disabled items (0.68%) |
| `switch` | 4 | 1.04% | 1.18 | Disabled (1.54%) |
| `progress-bar` | 5 | 0.46% | 0.37 | Controlled (0.70%) |
| `segment-display` | 2 | 0.59% | 0.42 | Clock (0.67%) |
| `status` | 2 | 1.17% | 1.04 | Status chips (1.65%) |
| `card` | 1 | 0.67% | 0.48 | Concentric rings (default) (0.67%) |
| `collapsible-section` | 4 | 1.52% | 1.49 | Default (1.85%) |
| `icon-tile` | 2 | 0.59% | 0.48 | Semantic tones (0.66%) |
| `keyboard-key` | 2 | 0.49% | 0.34 | Shortcut (0.50%) |

### Gaps

What GPUI cannot draw, what is not ported yet, and where the gallery itself differs from Dispatch, so the pixels differ on purpose:

- **The gallery's page.** Its root is 16px where Dispatch's is 13px, so rem paddings grow there: a texture button's `!px-4` is 3px wider on each side, which is most of what the button page differs by. Its `.docs-page h3` also draws section titles at 14px in the primary color, while Dispatch (and GPUI) keeps them 12px and muted. GPUI follows Dispatch in both.
- **Motion.** GPUI cannot scale an element, so menus and popovers enter with the fade and the 4px slide but without their slight vertical scale. It has no CSS filters either, so dimmed and lifted tier cards use opacity instead of grayscale and brightness.
- **Masks.** The rail's content mask (the fade under its top and bottom edges) cannot be drawn, so the rail clips instead.
- **Buttons.** `LinkButton` keeps the white keycap, since `TextureFaceProvider` does not reach anchors; GPUI draws the texture face only, and Dispatch has no link button, so the example draws a secondary one.
- **Inputs.** `TextInput::bare()` stands in for `.pw-filter-popover-search` (no fill, edge, or focus ring); the shared input has no other bare variant.
- **Type.** Code uses Menlo where Electron resolves SF Mono. GPUI's word wrap treats `-` as part of a word, so a long model id may break mid-word where Electron breaks after a hyphen, and the Intelligence footer wraps at a different point. It also counts the space after a line's last word toward the width, where Chromium lets the space hang, so a line that fills the measure to the last character, as the run transcript's 99-column lines can, keeps one word fewer; and it may break before a closing bracket, which Chromium keeps with the word before it. Electron truncates the agent cell at a sub-pixel width, GPUI at a whole pixel.
- **Color.** Screenshots of the Electron app come out in the display's P3 profile and are converted to sRGB before they are compared with GPUI's, which `save_window_png` draws straight into an sRGB bitmap; the conversion can leave saturated accents a few levels apart.
- **Mascot.** `EmptyBot` and the bots pick their pose from a seed, so two captures of the same page can show different poses.
- **Runs table.** The runs list's columns come out a few pixels wider than Electron's (the Run header starts at 289px, not 281px, and Status at 568px, not 547px), so the table overflows its page and shows a horizontal scrollbar Electron does not. `DataTable` is being rewritten, so its column sizing waits on that.
- **Hand-rolled.** The tier palette and the role composer are still hand-rolled; they move onto the shared popover once its rewrite lands. The DropdownMenu port waits on the same rewrite.
- **Not ported.** `StatusDot` with a label ("Compact dots"); Switch's sm size, disabled state, and trailing slot; Card's framed texture and the Surface variants (host, nested, floating); IconTile's brand, dimmed, chrome mark, chrome reflection, seeded, and aurora variants; RadioGroup's trailing slot; Select's disabled state.
- **No page yet.** Input, search bar, popover, DataTable, FilterPopover, and the active filter bar wait on the rewrite of those controls; toolbar icon button, tooltip, PageToolbar, badge, and RailPage have no page yet. The toast and page-states examples would differ anyway, since Dispatch's `PageEmpty` draws the mascot where the Solid gallery shows `EmptyIllustration`.

## Bundle

A plain `.app` for size and memory comparisons:

```sh
cargo build --release
mkdir -p target/Dispatch-GPUI.app/Contents/MacOS
cp target/release/dispatch-gpui target/Dispatch-GPUI.app/Contents/MacOS/
strip -x target/Dispatch-GPUI.app/Contents/MacOS/dispatch-gpui
```

with an `Info.plist` naming `dispatch-gpui` as `CFBundleExecutable`.

## ARCHITECTURE

`#![allow(dead_code)]` lives in `src/main.rs` because the shared modules export more than the views use yet. Do not add a blanket allow for unused imports or variables.

| File | Owns |
| --- | --- |
| `src/main.rs` | Window only: frameless transparent titlebar, traffic lights, min size 1000×640, font install, key-binding registration, opening `AppView`. |
| `src/app.rs` | `AppView`: sidebar, the route, the Commander overlay, the toast stack. One entity per view. |
| `src/theme.rs` | Dark `Theme` global: colors, radii, spacing, type, row heights. |
| `src/motion.rs` | `Spring`, `SpringTier`, `SpringValue`, `Animator`. |
| `src/client.rs` | Loopback `core.json` discovery and the blocking Connect-JSON client, including `WatchRun`. |
| `src/state.rs` | `AppState`: route, snapshots, 3s poll, toasts, preference files. |
| `src/format.rs` | Shared display strings (`prompt_first_line`, `non_empty_or`, `agent_display_name`, `is_active_status`). |
| `src/avatar.rs` | The only avatar seam views call: `agent_avatar` / `agent_avatar_keyed` draw the animated `bot-avatar` bot, one shared render per agent, status, size, and scale, ticking at 30 fps only while painted and the window is active. |
| `src/shots.rs` | The screenshot tour behind `DISPATCH_GPUI_SHOTS`. |
| `assets/fonts` | Inter and JetBrains Mono (SIL OFL, licenses beside them), embedded so text renders without them installed. |
| `src/window_drag.rs` | `WindowDragRegion::window_drag_region()`: drags the frameless window from any surface (AppKit `performWindowDragWithEvent:` after a 4px slop; double click follows the system titlebar setting). |
| `src/ui/` | Shared controls. One file per component. |
| `src/views/<view>.rs` | One screen each: `sidebar`, `run_page`, `transcript`, `roles`, `intelligence`, `spend`, `stats`, `settings`, `commander`, `runs`. `TranscriptView` is not a route: the run page mounts it, and the sidebar's long-hover card mounts it compact. |
| `src/models/<view>.rs` | The pure logic of that view, with the tests ported from the TypeScript original. |
| `src/ui/parts/<view>.rs` | UI helpers only that view uses. |
| `crates/dispatch-core` | A separate crate (the backend port), with its own lockfile; not a dependency of the app. |

`navigate` and `go_back` do not take `cx`. Callers redraw themselves:

```rust
state.update(cx, |s, cx| {
    s.navigate(route);
    s.request_refresh(cx);
    cx.notify();
});
```

### Parallel work

A change to one screen stays inside its three files (`src/views/<view>.rs`, `src/models/<view>.rs`, `src/ui/parts/<view>.rs`), so several screens can change at once without conflicts. Shared modules (`app`, `state`, `client`, `theme`, `motion`, `ui/*`, `avatar`, `main`) change on their own, and `cargo fmt` runs only there.

### Public APIs views use

**client.** `CoreClient` (`Clone + Send`, `connect`, `endpoint`, one method per RPC taking `&Request` and returning `Result<Response, CoreError>`, `watch_run` → `Result<WatchRunIter, CoreError>`, `run_reply`). `CoreError`, `CoreConfig`, `resolve_data_dir`, `core_json_path`, `read_core_config`, `validate_loopback_endpoint`, `strip_ansi`, `ProtoI64`, `encode_envelope`, `Envelope`, `EnvelopeDecoder`, `interpret_frame`. Every request and response struct uses camelCase, `#[serde(default)]`, and `ProtoI64` for int64. `WatchRunIter` items are `Result<WatchRunResponse, CoreError>`.

**state.** `AppState::open` / `new`, `reload_all`, `request_refresh`, `navigate`, `go_back`, `client`, `push_toast`, `dismiss_toast`, `toasts`, `route`, `connection`, `last_error`, `workspaces`, `workspaces_linked`, `runs`, `sections`, `workspace_groups`, `run`, `run_reply`, `reply_pending`, `commander_open`, `toggle_commander`, `set_commander_open`, `data_dir`, `read_preference`, `write_preference`. Types: `Route`, `SettingsPage`, `ConnectionState`, `Navigation`, `WorkspaceGroup`, `Toast`, `ToastTone`. `observe(state, cx)` subscribes a view.

**theme.** `Theme` global, `Theme::dark`, `Theme::global`. Named `color_*`, `radius_*` (`sm`/`md`/`lg` plus control, item, card, menu, surface, window, dialog), `space_1`…`space_8`, `font_size_*`, `font_sans` (Inter), `font_mono` (Menlo), `font_terminal` (JetBrains Mono), `row_height` 44, `page_header_height` 44, `sidebar_row_height` 44, `sidebar_width` 272. `StatusTone`, `status_tone`, `status_color`, `tone_color`.

**motion.** `Spring` (selection 420/38, collapse 380/39.5). `SpringTier::{Fast, Moderate, Slow}` and `params`. `SpringValue::{new, set_target, snap, step, value, target, settled, tier, set_tier}`. `Animator::dt(window, active) -> f32` requests another frame while any spring moves.

**ui.** `button` (`ButtonKind::{Primary, Secondary, Danger, Ghost}`), `icon_button` (icon only, tooltip is the label), `toolbar`, `titled_page`, `data_table` (`TableColumn`, `uniform_list`, 44px rows), `chip`, `chip_colors`, `status_dot`, `CollapsibleSection` (`SectionBody`), `rail_group`, `rail_card`, `TextInput` (single line, cursor, selection, copy/paste, placeholder, password, Enter submits, Escape cancels), `Popover` (`PopoverPanel`), `Menu` (`MenuEntry`), `tooltip`, `HoverCard` (`HoverBuilder`), `Switch`, `radio_group` (`RadioOption`), `segment_display` (`segment_glyph`, `GlyphKind`), `toast_stack` (`Toast`, `ToastTone`), `Icon`, `icon`, `IconAssets`, `register_key_bindings`.

**avatar.** `agent_avatar(agent, status, size_px)`. Raster seam, not for views: `agent_rgb`, `rasterize_placeholder_circle`, `premultiplied_rgba_to_bgra`, `render_image_from_rgba_premultiplied`, `paint_rgba_premultiplied`.
