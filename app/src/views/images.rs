//! Images: every artifact Dispatch indexed, newest first, as a grid of thumbnails. Each tile
//! says where it came from (the app, or the agent and session that delegated it), the run and
//! its title, the agent and model, the repository, and the date. The toolbar holds the search
//! over the runs' prompts and the shared `FilterPopover` (Origin, Agent, Repository), whose
//! pills sit in the `ActiveFilterBar` under it. A click opens the viewer with the image, the
//! prompt, and the way to its run; Delete asks first and removes the copy through
//! `DeleteArtifact`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, DispatchPhase, Entity, FocusHandle, FontWeight,
    IntoElement, KeyDownEvent, ObjectFit, Render, SharedString, Window, canvas, div, img,
    prelude::*, px,
};

use crate::client::{CoreClient, DeleteArtifactRequest, DispatchArtifact, ListArtifactsRequest};
use crate::models::images::{
    ImageTile, agent_text, filter_specs, group_by_day, local_datetime, origin_text, repo_name,
    visible_tiles,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, ToastTone};
use crate::theme::Theme;
use crate::ui::patterns::{ConfirmDialog, composer_card, composer_footer, modal_layer};
use crate::ui::{
    ButtonKind, ButtonSize, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood,
    SkeletonLayout, TextInput, button, card_grid, ellipsis, page_empty, page_skeleton, page_under,
    page_under_inset, scroll_area_under, toolbar,
};

/// The page's chrome state.
const PAGE: &str = "images-page";
/// The narrowest a tile gets before the grid drops a column.
const TILE_MIN: f32 = 220.0;
const TILE_GAP: f32 = 12.0;
/// The thumbnail's height, with the image filling its width.
const THUMB_HEIGHT: f32 = 150.0;
/// A reload on a state change comes no sooner than this, as the runs and Spend do.
const RELOAD_GAP: Duration = Duration::from_secs(2);
/// The viewer's width, wider than a Composer for its image.
const VIEWER_WIDTH: f32 = 820.0;
/// The viewer's image area.
const VIEWER_IMAGE_HEIGHT: f32 = 320.0;

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
    /// The tile the viewer shows, kept while it fades out after it closes.
    viewing: Option<String>,
    viewer_open: bool,
    viewer_opacity: SpringValue,
    viewer_y: SpringValue,
    /// The artifact whose delete is being confirmed.
    deleting: Option<String>,
    confirm_focus: FocusHandle,
    animator: Animator,
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
            viewing: None,
            viewer_open: false,
            viewer_opacity: SpringValue::new(0.0, SpringTier::Fast),
            viewer_y: SpringValue::new(8.0, SpringTier::Fast),
            deleting: None,
            confirm_focus: cx.focus_handle(),
            animator: Animator::new(),
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
        if !matches!(self.state.read(cx).route(), Route::Images) {
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

    fn open_viewer(&mut self, id: String, cx: &mut Context<Self>) {
        self.viewing = Some(id);
        self.viewer_open = true;
        cx.notify();
    }

    fn close_viewer(&mut self, cx: &mut Context<Self>) {
        if self.viewer_open {
            self.viewer_open = false;
            cx.notify();
        }
    }

    fn open_run(&mut self, run_id: String, cx: &mut Context<Self>) {
        self.viewer_open = false;
        self.state.update(cx, |app, cx| {
            app.navigate(Route::Run(run_id));
            cx.notify();
        });
        cx.notify();
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
                    if this.viewing.as_deref() == Some(id.as_str()) {
                        this.viewer_open = false;
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

    fn toast(&self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.state.update(cx, |app, cx| {
            app.push_toast(message, ToastTone::Danger);
            cx.notify();
        });
    }

    fn tick_motion(&mut self, window: &mut Window) {
        self.viewer_opacity
            .set_target(if self.viewer_open { 1.0 } else { 0.0 });
        self.viewer_y
            .set_target(if self.viewer_open { 0.0 } else { 8.0 });
        let moving = !self.viewer_opacity.settled() || !self.viewer_y.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.viewer_opacity.step(dt);
            self.viewer_y.step(dt);
        }
        if !self.viewer_open && self.viewer_opacity.settled() && self.viewer_opacity.value() < 0.02
        {
            self.viewing = None;
        }
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

    /// The viewer, while it is open or fading out.
    fn viewer_layer(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.viewing.as_ref()?;
        let tile = self.tiles.iter().find(|tile| &tile.id == id)?;
        let t = self.viewer_opacity.value();
        let card = image_viewer(tile, t, self.viewer_y.value(), cx.entity(), theme, cx);
        let dismiss = cx.entity();
        Some(modal_layer(
            t,
            self.viewer_open,
            Rc::new(move |_, cx| {
                dismiss.update(cx, |this, cx| this.close_viewer(cx));
            }),
            card,
            theme,
        ))
    }

    /// The delete's confirmation, over the viewer.
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
        let name = repo_name(&tile.rel_path);
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
        self.tick_motion(window);
        self.sync_fields(cx);
        let theme = *Theme::global(cx);
        let query = self.search.read(cx).text().to_string();
        let filters = self.filters.read(cx).filters().clone();
        let visible = visible_tiles(&self.tiles, &query, &filters);
        let has_tiles = !self.tiles.is_empty();
        let shows_grid = self.loaded && !visible.is_empty();
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        // The toolbar's inset, so the grid's edges line up with its buttons.
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, filter_bar.is_some(), padding, window, cx);
        let view = cx.entity();

        let content: AnyElement = if !self.loaded {
            // The gallery, in bones, until the first answer comes back.
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
            let mut body = div().w_full().flex().flex_col().gap(px(theme.space_4));
            for group in groups {
                let tiles = group
                    .tiles
                    .iter()
                    .map(|tile| image_tile(tile, &theme, view.clone()))
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
                            4,
                            tiles,
                            window,
                            cx,
                        )),
                );
            }
            scroll_area_under("images-scroll", top, body, window, cx)
        };

        // The `SearchBar` pill, then `FilterPopover`'s icon button, 8px apart.
        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone());
        let scrolls = shows_grid;
        let escape = (self.viewer_open || self.deleting.is_some()).then(|| {
            let view = cx.entity();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let view = view.clone();
                    window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            view.update(cx, |this, cx| {
                                if this.deleting.is_some() {
                                    this.cancel_delete(cx);
                                } else {
                                    this.close_viewer(cx);
                                }
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        let viewer = self.viewer_layer(&theme, cx);
        let confirm = self.confirm_layer(&theme, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            // The grid runs up under the toolbar, which fades it out on its trail.
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
            .children(viewer)
            .children(confirm)
    }
}

fn load_artifacts(client: &CoreClient) -> Option<Vec<DispatchArtifact>> {
    client
        .list_artifacts(&ListArtifactsRequest::default())
        .ok()
        .map(|response| response.artifacts)
}

/// One thumbnail on the grid: the image, its run's title, where it came from, its agent and
/// model, and its repository and date.
fn image_tile(tile: &ImageTile, theme: &Theme, view: Entity<ImagesView>) -> AnyElement {
    let selector = format!("image-tile-{}", tile.id);
    let artifact = tile.id.clone();
    let opener = view;
    let mut meta = div().flex().flex_col().gap(px(4.0));
    meta = meta
        .child(
            div()
                .text_size(px(theme.font_size_sm))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.color_text_primary)
                .child(ellipsis(tile.title.clone())),
        )
        .child(origin_line(tile, theme))
        .child(meta_line(agent_text(tile), theme));
    let mut where_line = tile.repo_name.clone();
    if !tile.date.is_empty() {
        if !where_line.is_empty() {
            where_line.push_str(" · ");
        }
        where_line.push_str(&tile.date);
    }
    if !where_line.is_empty() {
        meta = meta.child(meta_line(where_line, theme));
    }
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .flex()
        .flex_col()
        .rounded(px(theme.radius_card))
        .border_1()
        .border_color(theme.color_border_subtle)
        .bg(theme.color_surface_subtle)
        .cursor_pointer()
        .hover(|style| style.border_color(theme.color_border_strong))
        .on_click(move |_, _, cx| {
            let artifact = artifact.clone();
            opener.update(cx, |this, cx| this.open_viewer(artifact, cx));
        })
        .child(
            div()
                .w_full()
                .h(px(THUMB_HEIGHT))
                .flex_none()
                .bg(theme.color_surface)
                .rounded_tl(px(theme.radius_card))
                .rounded_tr(px(theme.radius_card))
                .overflow_hidden()
                .child(
                    img(PathBuf::from(&tile.path))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Cover),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .px(px(10.0))
                .py(px(9.0))
                .child(meta),
        )
        .into_any_element()
}

/// Where a tile came from: "App", or a delegated run's calling bot, agent, and session.
fn origin_line(tile: &ImageTile, theme: &Theme) -> AnyElement {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .text_size(px(theme.font_size_xs))
        .text_color(theme.color_text_muted);
    if tile.origin == crate::models::images::Origin::Delegated {
        row = row.child(crate::avatar::agent_avatar(&tile.caller_agent, "", 14.0));
    }
    row.child(SharedString::from(origin_text(tile)))
        .into_any_element()
}

fn meta_line(text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    div()
        .text_size(px(theme.font_size_xs))
        .text_color(theme.color_text_muted)
        .child(ellipsis(text.into()))
        .into_any_element()
}

/// The viewer: the image, the prompt that made it, where it came from, and the ways to its
/// run and to delete it. Kept in one function so a shared image viewer can replace it.
fn image_viewer(
    tile: &ImageTile,
    presence: f32,
    offset_y: f32,
    view: Entity<ImagesView>,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let closer = view.clone();
    let opener = view.clone();
    let run_id = tile.run_id.clone();
    let deleter = view;
    let artifact = tile.id.clone();
    let prompt = tile.prompt.trim().to_string();
    let file = format!(
        "{} \u{00b7} {} \u{00d7} {} \u{00b7} {}",
        tile.rel_path,
        tile.width,
        tile.height,
        tile.kind.to_uppercase()
    );
    let facts = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .text_size(px(theme.font_size_sm))
        .child(fact("From", origin_text(tile), theme))
        .child(fact("Run", tile.title.clone(), theme))
        .child(fact("Agent", agent_text(tile), theme))
        .child(fact(
            "Repository",
            if tile.repo.is_empty() {
                repo_name(&tile.path)
            } else {
                tile.repo.clone()
            },
            theme,
        ))
        .child(fact("Date", local_datetime(&tile.created_at), theme))
        .child(fact("File", file, theme));
    let body = div()
        .debug_selector(|| "image-viewer".to_string())
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .w_full()
                .h(px(VIEWER_IMAGE_HEIGHT))
                .rounded(px(theme.radius_item))
                .bg(theme.color_surface_subtle)
                .overflow_hidden()
                .child(
                    img(PathBuf::from(&tile.path))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Contain),
                ),
        )
        .child(
            div()
                .text_size(px(theme.font_size_md))
                .text_color(theme.color_text_secondary)
                .when(prompt.is_empty(), |text| {
                    text.child("No prompt was recorded.")
                })
                .when(!prompt.is_empty(), |text| text.child(prompt)),
        )
        .child(facts);
    composer_card(tile.title.clone(), presence, offset_y, theme)
        .heading()
        .width(VIEWER_WIDTH)
        .on_close(Rc::new(move |_, cx| {
            closer.update(cx, |this, cx| this.close_viewer(cx));
        }))
        .child(body)
        .footer(composer_footer(
            [button(
                "image-delete",
                "Delete",
                ButtonKind::Danger,
                Rc::new(move |_, cx| {
                    let artifact = artifact.clone();
                    deleter.update(cx, |this, cx| this.ask_delete(artifact, cx));
                }),
                cx,
            )
            .size(ButtonSize::Sm)
            .into_any_element()],
            [button(
                "image-open-run",
                "Open run",
                ButtonKind::Primary,
                Rc::new(move |_, cx| {
                    let run_id = run_id.clone();
                    opener.update(cx, |this, cx| this.open_run(run_id, cx));
                }),
                cx,
            )
            .size(ButtonSize::Sm)
            .into_any_element()],
            theme,
        ))
        .into_any_element()
}

fn fact(label: &'static str, value: String, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .w(px(78.0))
                .flex_none()
                .text_color(theme.color_text_muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(theme.color_text_secondary)
                .child(value),
        )
        .into_any_element()
}
