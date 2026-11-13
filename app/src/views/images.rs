//! Images: every artifact Dispatch indexed, newest first, as a grid of thumbnails. Each tile
//! says where it came from (the app, or the agent and session that delegated it), the run and
//! its title, the agent and model, the repository, and the date. The toolbar holds the search
//! over the runs' prompts and the shared `FilterPopover` (Origin, Agent, Repository), whose
//! pills sit in the `ActiveFilterBar` under it. Clicking a tile navigates to `Route::Image`
//! showing the image large with its properties and actions in a floating rail; Delete asks
//! first and removes the copy through `DeleteArtifact`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, DispatchPhase, Entity, FocusHandle,
    FontWeight, Image, ImageFormat, IntoElement, KeyDownEvent, ObjectFit, Render, SharedString,
    Window, canvas, div, img, prelude::*, px,
};

use crate::client::{CoreClient, DeleteArtifactRequest, DispatchArtifact, ListArtifactsRequest};
use crate::format::agent_display_name;
use crate::models::images::{
    ImageTile, filter_specs, format_bytes, group_by_day, local_datetime, visible_tiles,
};
use crate::models::runs::{EM_DASH, format_ago};
use crate::state::{AppState, ConnectionState, Route, ToastTone};
use crate::theme::Theme;
use crate::ui::patterns::{ConfirmDialog, modal_layer};
use crate::ui::{
    Button, ButtonKind, ButtonSize, FilterField, FilterPopover, FiltersChanged, Icon, ImageCard,
    ImageData, MascotMood, PageWidth, RailPresence, SkeletonLayout, TextInput, ZoomableImage,
    card_grid, ellipsis, grid_columns, page_empty, page_skeleton, page_under, page_under_inset,
    rail_page_body_flush, rail_page_header, rail_properties, rail_property, rail_section,
    rail_toggle_label, scroll_area_under, toolbar, trailing_toolbar, wrap_text,
};

/// The page's chrome state.
const PAGE: &str = "images-page";
/// The narrowest a tile gets before the grid drops a column.
const TILE_MIN: f32 = 220.0;
const TILE_GAP: f32 = 12.0;
/// A reload on a state change comes no sooner than this, as the runs and Spend do.
const RELOAD_GAP: Duration = Duration::from_secs(2);
/// Where the rail's open state is remembered.
const RAIL_PREFERENCE: &str = "polterware.image.rail";

pub struct ImagesView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    tiles: Vec<ImageTile>,
    loaded: bool,
    on_page: bool,
    in_flight: bool,
    again: bool,
    last_kick: Instant,
    ticket: u64,
    /// The artifact whose delete is being confirmed.
    deleting: Option<String>,
    confirm_focus: FocusHandle,
    rail_open: bool,
    drawer_open: bool,
    rail: RailPresence,
    page_width: PageWidth,
    zoomable: Option<(String, Entity<ZoomableImage>)>,
}

impl ImagesView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder("Search image prompts\u{2026}")
                .pill()
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let filters = cx.new(|cx| FilterPopover::new("image-filters", "Image filters", true, cx));
        cx.subscribe(&filters, |_, _, _: &FiltersChanged, cx| cx.notify())
            .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |this, _, cx| this.kick(cx)).detach();

        let rail_open = state
            .read(cx)
            .read_preference::<String>(RAIL_PREFERENCE)
            .ok()
            .flatten()
            .is_none_or(|value| value != "hidden");

        let view = Self {
            state,
            search,
            filters,
            fields_key: String::new(),
            tiles: Vec::new(),
            loaded: false,
            on_page: false,
            in_flight: false,
            again: false,
            last_kick: Instant::now(),
            ticket: 0,
            deleting: None,
            confirm_focus: cx.focus_handle(),
            rail_open,
            drawer_open: false,
            rail: RailPresence::new(rail_open),
            page_width: PageWidth::default(),
            zoomable: None,
        };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |view, cx| view.kick(cx));
        })
        .detach();
        view
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// How many tiles the page holds, whatever the filters keep (the tests read it).
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Loads the gallery on entry and as the app's state moves, at most every two seconds for
    /// a routine reload. A load in flight takes one more after it.
    fn kick(&mut self, cx: &mut Context<Self>) {
        let on_images = matches!(self.state.read(cx).route(), Route::Images | Route::Image(_));
        if !on_images {
            self.on_page = false;
            return;
        }
        let entered = !self.on_page;
        self.on_page = true;
        if self.in_flight {
            if entered {
                self.again = true;
            }
            return;
        }
        if !entered && self.last_kick.elapsed() < RELOAD_GAP {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            let offline = matches!(
                self.state.read(cx).connection(),
                ConnectionState::Offline { .. }
            );
            if offline && !self.loaded {
                self.loaded = true;
                cx.notify();
            }
            return;
        };
        self.in_flight = true;
        self.again = false;
        self.last_kick = Instant::now();
        self.ticket = self.ticket.wrapping_add(1);
        let ticket = self.ticket;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { load_artifacts(&client) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.ticket != ticket {
                    return;
                }
                this.in_flight = false;
                if let Some(artifacts) = result {
                    this.tiles = artifacts.iter().map(ImageTile::from).collect();
                }
                this.loaded = true;
                if this.again {
                    this.again = false;
                    this.kick(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_image(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.navigate(Route::Image(id));
            cx.notify();
        });
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.navigate(Route::Images);
            cx.notify();
        });
    }

    fn open_run(&mut self, run_id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.navigate(Route::Run(run_id));
            cx.notify();
        });
    }

    fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        if self.page_width.is_narrow() {
            self.drawer_open = !self.drawer_open;
        } else {
            self.rail_open = !self.rail_open;
            let value = if self.rail_open { "open" } else { "hidden" };
            let _ = self
                .state
                .read(cx)
                .write_preference(RAIL_PREFERENCE, &value);
        }
        cx.notify();
    }

    fn rail_visible(&self) -> bool {
        if self.page_width.is_narrow() {
            self.drawer_open
        } else {
            self.rail_open
        }
    }

    fn ask_delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.deleting = Some(id);
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        if self.deleting.take().is_some() {
            cx.notify();
        }
    }

    fn delete_image(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", cx);
            return;
        };
        self.deleting = None;
        cx.spawn(async move |this, cx| {
            let target = id.clone();
            let result = cx
                .background_spawn(async move {
                    client.delete_artifact(&DeleteArtifactRequest { id: target })
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.tiles.retain(|tile| tile.id != id);
                    if matches!(this.state.read(cx).route(), Route::Image(cur_id) if cur_id == &id)
                    {
                        this.state.update(cx, |app, cx| {
                            app.navigate(Route::Images);
                            cx.notify();
                        });
                    }
                    this.state.update(cx, |app, cx| {
                        app.push_toast("Image deleted.", ToastTone::Success);
                        cx.notify();
                    });
                    cx.notify();
                }
                Err(error) => this.toast(crate::views::core_message(error), cx),
            });
        })
        .detach();
    }

    fn reveal_image(&self, path: &str, cx: &mut Context<Self>) {
        let opened = Command::new("/usr/bin/open")
            .args(["-R", path])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !opened {
            self.toast("Could not reveal the image", cx);
        }
    }

    fn copy_image(&self, path: &str, cx: &mut Context<Self>) {
        let format = match path.rsplit('.').next() {
            Some(extension) if extension.eq_ignore_ascii_case("png") => Some(ImageFormat::Png),
            Some(extension) if extension.eq_ignore_ascii_case("jpg") => Some(ImageFormat::Jpeg),
            Some(extension) if extension.eq_ignore_ascii_case("jpeg") => Some(ImageFormat::Jpeg),
            Some(extension) if extension.eq_ignore_ascii_case("webp") => Some(ImageFormat::Webp),
            Some(extension) if extension.eq_ignore_ascii_case("gif") => Some(ImageFormat::Gif),
            _ => None,
        };
        let copied = format
            .and_then(|format| {
                std::fs::read(Path::new(path))
                    .ok()
                    .map(|bytes| Image::from_bytes(format, bytes))
            })
            .map(|picture| {
                cx.write_to_clipboard(ClipboardItem::new_image(&picture));
            });
        if copied.is_none() {
            cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
            self.state.update(cx, |app, cx| {
                app.push_toast("Copied the image's path", ToastTone::Success);
                cx.notify();
            });
        } else {
            self.state.update(cx, |app, cx| {
                app.push_toast("Image copied to clipboard", ToastTone::Success);
                cx.notify();
            });
        }
    }

    fn toast(&self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.state.update(cx, |app, cx| {
            app.push_toast(message, ToastTone::Danger);
            cx.notify();
        });
    }

    /// The fields follow the tiles' origins, agents, and repositories, rebuilt only when
    /// those change.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let specs = filter_specs(&self.tiles);
        let key = specs
            .iter()
            .flat_map(|spec| spec.options.iter().map(|option| option.id.as_str()))
            .collect::<Vec<_>>()
            .join("\u{1f}");
        if key == self.fields_key && !self.filters.read(cx).fields().is_empty() {
            return;
        }
        self.fields_key = key;
        let icons = [Icon::Globe2, Icon::Bot, Icon::Folder];
        self.filters.update(cx, |popover, _| {
            popover.set_fields(
                specs
                    .into_iter()
                    .zip(icons)
                    .map(|(spec, icon)| FilterField::new(spec, Some(icon)))
                    .collect(),
            );
        });
    }

    /// The delete's confirmation dialog.
    fn confirm_layer(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.deleting.as_ref()?;
        let tile = self.tiles.iter().find(|tile| &tile.id == id)?;
        if !self.confirm_focus.is_focused(window) {
            self.confirm_focus.focus(window);
        }
        let name = crate::models::images::repo_name(&tile.rel_path);
        let cancel: crate::ui::Handler = {
            let view = cx.entity();
            Rc::new(move |_, cx: &mut App| {
                view.update(cx, |this, cx| this.cancel_delete(cx));
            })
        };
        let confirm: crate::ui::Handler = {
            let view = cx.entity();
            let id = id.clone();
            Rc::new(move |_, cx: &mut App| {
                let id = id.clone();
                view.update(cx, |this, cx| this.delete_image(id, cx));
            })
        };
        let dialog = ConfirmDialog::new(
            "delete-image",
            format!("Delete \u{201c}{name}\u{201d}?"),
            "Dispatch removes the image's copy from this computer.",
        )
        .recovery("The run that made it stays, and its spend and stats with it.")
        .confirm_label("Delete")
        .danger()
        .focus(&self.confirm_focus)
        .on_cancel(cancel.clone())
        .on_confirm(confirm);
        Some(modal_layer(1.0, true, cancel, dialog, theme))
    }
}

impl Render for ImagesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = self.state.read(cx).route().clone();
        if let Route::Image(artifact_id) = route {
            return self.render_image_page(&artifact_id, window, cx);
        }
        self.render_gallery(window, cx)
    }
}

impl ImagesView {
    fn render_gallery(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.sync_fields(cx);
        let theme = *Theme::global(cx);
        let query = self.search.read(cx).text().to_string();
        let filters = self.filters.read(cx).filters().clone();
        let visible = visible_tiles(&self.tiles, &query, &filters);
        let has_tiles = !self.tiles.is_empty();
        let shows_grid = self.loaded && !visible.is_empty();
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, filter_bar.is_some(), padding, window, cx);
        let view = cx.entity();

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);

        let content: AnyElement = if !self.loaded {
            page_skeleton(SkeletonLayout::Page)
                .without_toolbar()
                .into_any_element()
        } else if visible.is_empty() {
            let (title, mood) = if has_tiles {
                ("No matches", MascotMood::Lost)
            } else {
                ("No images yet", MascotMood::Sleeping)
            };
            page_empty(title, mood, cx).into_any_element()
        } else {
            let groups = group_by_day(&visible);
            let viewport = f32::from(window.viewport_size().width);
            let available_width = (viewport - theme.sidebar_width - 26.0).max(TILE_MIN);
            let cols = grid_columns(available_width, TILE_MIN, TILE_GAP);
            let col_width =
                (available_width - (f32::from(cols) - 1.0) * TILE_GAP) / f32::from(cols);
            let thumb_height = ((col_width - 2.0) * 10.0 / 16.0).max(100.0);

            let mut body = div().w_full().flex().flex_col().gap(px(theme.space_4));
            for group in groups {
                let tiles = group
                    .tiles
                    .iter()
                    .map(|tile| image_tile(tile, thumb_height, &theme, view.clone(), now_ms))
                    .collect();
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme.space_2))
                        .child(
                            div()
                                .text_size(px(theme.font_size_sm))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.color_text_muted)
                                .child(SharedString::from(group.label.clone())),
                        )
                        .child(card_grid(
                            SharedString::from(format!("image-grid-{}", group.label)),
                            TILE_MIN,
                            TILE_GAP,
                            cols,
                            tiles,
                            window,
                            cx,
                        )),
                );
            }
            scroll_area_under("images-scroll", top, body, window, cx)
        };

        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone());
        let scrolls = shows_grid;
        let escape = self.deleting.is_some().then(|| {
            let view = cx.entity();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let view = view.clone();
                    window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            view.update(cx, |this, cx| {
                                this.cancel_delete(cx);
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        let confirm = self.confirm_layer(&theme, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                filter_bar,
                padding,
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .when(!scrolls, |content| content.pt(px(top)))
                    .child(content),
                window,
                cx,
            ))
            .children(escape)
            .children(confirm)
            .into_any_element()
    }

    fn render_image_page(
        &mut self,
        artifact_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *Theme::global(cx);
        let tile = self.tiles.iter().find(|t| t.id == artifact_id).cloned();
        let view = cx.entity();
        let back_view = view.clone();

        let back = Button::new("image-page-back", ButtonKind::Secondary)
            .size(ButtonSize::Icon)
            .icon(Icon::ChevronLeft)
            .tooltip("Back")
            .on_click(move |_, cx| back_view.update(cx, |this, cx| this.go_back(cx)));

        let visible = self.rail_visible();
        self.rail.set_open(visible);

        let toggle_view = view.clone();
        let toggle = Button::new("image-rail-toggle", ButtonKind::Secondary)
            .size(ButtonSize::Icon)
            .icon(Icon::SidebarRightIcon)
            .tooltip(rail_toggle_label(visible))
            .selected(visible)
            .on_click(move |_, cx| toggle_view.update(cx, |this, cx| this.toggle_rail(cx)));

        let Some(tile) = tile else {
            if !self.loaded {
                return page_skeleton(SkeletonLayout::Page)
                    .without_toolbar()
                    .into_any_element();
            }
            let header = rail_page_header(back, toggle, cx);
            return div()
                .size_full()
                .relative()
                .child(header)
                .child(page_empty("Image not found", MascotMood::Lost, cx))
                .into_any_element();
        };

        let reveal_path = tile.path.clone();
        let copy_path = tile.path.clone();
        let delete_id = tile.id.clone();
        let run_id = tile.run_id.clone();
        let reveal_view = view.clone();
        let copy_view = view.clone();
        let delete_view = view.clone();
        let run_view = view.clone();

        let title = div()
            .max_w(px(360.0))
            .min_w(px(0.0))
            .text_size(px(theme.font_size_xs))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.color_text_primary)
            .child(ellipsis(tile.title.clone()));

        let start = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(back)
            .child(title);

        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                Button::new("image-reveal", ButtonKind::Secondary)
                    .size(ButtonSize::Sm)
                    .icon(Icon::Folder)
                    .label("Reveal in Finder")
                    .on_click(move |_, cx| {
                        reveal_view.update(cx, |this, cx| this.reveal_image(&reveal_path, cx));
                    }),
            )
            .child(
                Button::new("image-copy", ButtonKind::Secondary)
                    .size(ButtonSize::Sm)
                    .icon(Icon::Copy)
                    .label("Copy")
                    .on_click(move |_, cx| {
                        copy_view.update(cx, |this, cx| this.copy_image(&copy_path, cx));
                    }),
            )
            .child(
                Button::new("image-delete", ButtonKind::Ghost)
                    .size(ButtonSize::Icon)
                    .icon(Icon::Delete)
                    .tooltip("Delete image")
                    .on_click(move |_, cx| {
                        let id = delete_id.clone();
                        delete_view.update(cx, |this, cx| this.ask_delete(id, cx));
                    }),
            );

        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(actions)
            .child(toggle);

        let header = rail_page_header(start, end, cx);
        let header_h = theme.page_header_height;

        let zoomable = match &self.zoomable {
            Some((id, entity)) if id == &tile.id => entity.clone(),
            _ => {
                let nat_w = if tile.width > 0 {
                    tile.width as f32
                } else {
                    1024.0
                };
                let nat_h = if tile.height > 0 {
                    tile.height as f32
                } else {
                    1024.0
                };
                let entity = cx.new(|_| {
                    ZoomableImage::new(
                        "image-viewer",
                        ImageData::from_path(PathBuf::from(&tile.path)),
                    )
                    .natural(nat_w, nat_h)
                    .debug_selector("image-viewer")
                });
                self.zoomable = Some((tile.id.clone(), entity.clone()));
                entity
            }
        };

        // Center column: zoomable image fitted inside the surface preserving aspect ratio
        let column = div()
            .id("image-viewer-column")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .pt(px(header_h))
            .child(zoomable);

        let prompt_text = if tile.prompt.trim().is_empty() {
            "No prompt was recorded.".to_string()
        } else {
            tile.prompt.clone()
        };

        let image_section = rail_section("Image", Icon::Image, cx).child(
            rail_properties()
                .child(rail_property(
                    "Kind",
                    wrapped(&tile.kind.to_uppercase()),
                    false,
                    cx,
                ))
                .child(rail_property(
                    "Dimensions",
                    wrapped(&format!("{} \u{00d7} {}", tile.width, tile.height)),
                    false,
                    cx,
                ))
                .child(rail_property(
                    "Bytes",
                    wrapped(&format_bytes(tile.bytes)),
                    false,
                    cx,
                ))
                .child(rail_property(
                    "Created",
                    wrapped(&local_datetime(&tile.created_at)),
                    false,
                    cx,
                )),
        );

        let run_section = rail_section("Run", Icon::InformationCircle, cx).child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .w_full()
                        .text_size(px(theme.font_size_sm))
                        .line_height(px(theme.font_size_sm * 1.4))
                        .text_color(theme.color_text_secondary)
                        .max_h(px(84.0))
                        .overflow_hidden()
                        .child(wrapped(&prompt_text)),
                )
                .child(
                    div().flex().child(
                        Button::new("image-open-run", ButtonKind::Secondary)
                            .size(ButtonSize::Sm)
                            .icon(Icon::ArrowUpRight01Icon)
                            .label("Open run")
                            .on_click(move |_, cx| {
                                let id = run_id.clone();
                                run_view.update(cx, |this, cx| this.open_run(id, cx));
                            }),
                    ),
                ),
        );

        let agent_avatar_el = crate::avatar::agent_avatar(&tile.agent, "", 18.0);
        let agent_section = rail_section("Agent", Icon::Bot, cx).child(
            rail_properties()
                .child(rail_property(
                    "Agent",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(agent_avatar_el)
                        .child(wrapped(&agent_display_name(&tile.agent))),
                    false,
                    cx,
                ))
                .child(rail_property(
                    "Model",
                    wrapped(if tile.model.trim().is_empty() {
                        EM_DASH
                    } else {
                        &tile.model
                    }),
                    false,
                    cx,
                )),
        );

        let origin_section = rail_section("Origin", Icon::Globe2, cx).child(
            rail_properties()
                .child(rail_property(
                    "Origin",
                    wrapped(tile.origin.label()),
                    false,
                    cx,
                ))
                .when(
                    tile.origin == crate::models::images::Origin::Delegated,
                    |props| {
                        let caller = if tile.session.is_empty() {
                            agent_display_name(&tile.caller_agent)
                        } else {
                            format!(
                                "{} \u{00b7} {}",
                                agent_display_name(&tile.caller_agent),
                                tile.session
                            )
                        };
                        props.child(rail_property("Caller", wrapped(&caller), false, cx))
                    },
                ),
        );

        let repo_display = if tile.repo.is_empty() {
            tile.repo_name.clone()
        } else {
            tile.repo.clone()
        };
        let repo_section =
            rail_section("Repository", Icon::Folder, cx).child(rail_properties().child(
                rail_property("Repository", wrapped(&repo_display), true, cx),
            ));

        let rail = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(image_section)
            .child(run_section)
            .child(agent_section)
            .child(origin_section)
            .child(repo_section)
            .into_any_element();

        let (row, drawer) = rail_page_body_flush(
            column,
            rail,
            &self.rail,
            &self.page_width,
            header_h,
            window,
            cx,
        );

        let confirm = self.confirm_layer(&theme, window, cx);
        let view_escape = view;

        let escape_canvas = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let view = view_escape.clone();
                window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                        view.update(cx, |this, cx| {
                            if this.deleting.is_some() {
                                this.cancel_delete(cx);
                            } else {
                                this.go_back(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0();

        div()
            .size_full()
            .relative()
            .child(self.page_width.probe())
            .child(row)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .child(trailing_toolbar(
                        header,
                        theme.color_surface,
                        header_h,
                        theme.radius_surface,
                    )),
            )
            .children(drawer)
            .children(confirm)
            .child(escape_canvas)
            .into_any_element()
    }
}

fn load_artifacts(client: &CoreClient) -> Option<Vec<DispatchArtifact>> {
    client
        .list_artifacts(&ListArtifactsRequest::default())
        .ok()
        .map(|response| response.artifacts)
}

fn wrapped(text: &str) -> AnyElement {
    wrap_text(text.to_string()).into_any_element()
}

/// One thumbnail on the grid: the image, its run's title, its small bot + model and relative date.
fn image_tile(
    tile: &ImageTile,
    thumb_height: f32,
    theme: &Theme,
    view: Entity<ImagesView>,
    now_ms: i64,
) -> AnyElement {
    let selector = format!("image-tile-{}", tile.id);
    let artifact = tile.id.clone();
    let opener = view;

    let ago = format_ago(&tile.created_at, now_ms);
    let meta_text = if tile.model.trim().is_empty() {
        if ago.is_empty() {
            agent_display_name(&tile.agent)
        } else {
            format!("{} \u{00b7} {ago}", agent_display_name(&tile.agent))
        }
    } else if ago.is_empty() {
        tile.model.clone()
    } else {
        format!("{} \u{00b7} {ago}", tile.model)
    };

    let meta = div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_size(px(theme.font_size_xs))
        .text_color(theme.color_text_muted)
        .child(crate::avatar::agent_avatar(&tile.agent, "", 14.0))
        .child(div().flex_1().min_w(px(0.0)).child(ellipsis(meta_text)));

    let card = ImageCard::new(
        SharedString::from(format!("card-{}", tile.id)),
        ImageData::from_path(PathBuf::from(&tile.path)),
        tile.title.clone(),
    )
    .thumb_height(thumb_height)
    .meta(meta)
    .on_click(move |_, cx| {
        let artifact = artifact.clone();
        opener.update(cx, |this, cx| this.open_image(artifact, cx));
    });

    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .child(card)
        .into_any_element()
}
