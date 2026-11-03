//! Screenshot tour for development: with `DISPATCH_GPUI_SHOTS=<dir>`, the app
//! visits every screen, saves a PNG of its own window for each one, and quits.
//!
//! A process may capture its own windows without the Screen Recording
//! permission, so this works where `screencapture` is refused.
//!
//! `DISPATCH_GPUI_SHOTS_ONLY=01-roles,14-commander` keeps only the stops whose names start
//! with one of those prefixes.
//!
//! After a page with filters, `<page>-filter-menu` opens its filter menu as the F key does and
//! `<page>-filter-bar` adds a pill for its first field's first value. `<page>-empty` filters the
//! first field by a value no row has, so the page shows its `PageEmpty` "No matches".

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, AsyncWindowContext, Entity, Window};

use crate::state::{AppState, Route, SettingsPage};
use crate::ui::FilterPopover;

/// A page's filters, for the stops that open its menu and add a pill.
pub type PageFilters = Vec<(&'static str, Entity<FilterPopover>)>;

/// The directory to write to, when the tour is on.
pub fn shots_dir() -> Option<PathBuf> {
    std::env::var_os("DISPATCH_GPUI_SHOTS")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// Visits each screen, waits for it to settle, and saves it.
pub fn start_tour(
    dir: PathBuf,
    state: Entity<AppState>,
    filters: PageFilters,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            let _ = std::fs::create_dir_all(&dir);
            settle(cx, 6000).await;
            let first_run = cx
                .update(|_, app| state.read(app).runs().first().map(|run| run.id.clone()))
                .ok()
                .flatten();
            let mut stops: Vec<(&str, Route)> = vec![
                ("01-roles", Route::Roles),
                ("02-role-board-general", Route::RoleBoard("general".into())),
                ("03-intelligence", Route::Intelligence),
                ("04-spend", Route::Spend),
                ("05-stats", Route::Stats),
                ("06-settings-agents", Route::Settings(SettingsPage::Agents)),
                (
                    "07-settings-try-route",
                    Route::Settings(SettingsPage::TryRoute),
                ),
                (
                    "08-settings-classifier",
                    Route::Settings(SettingsPage::Classifier),
                ),
                (
                    "09-settings-pick-role",
                    Route::Settings(SettingsPage::PickRole),
                ),
                (
                    "10-settings-classify",
                    Route::Settings(SettingsPage::ClassifyPrompt),
                ),
                (
                    "11-settings-routing",
                    Route::Settings(SettingsPage::RoutingPolicy),
                ),
                ("12-runs", Route::Runs),
            ];
            if let Some(id) = first_run {
                stops.push(("13-run", Route::Run(id)));
            }
            let only: Option<Vec<String>> = std::env::var("DISPATCH_GPUI_SHOTS_ONLY")
                .ok()
                .filter(|list| !list.is_empty())
                .map(|list| list.split(',').map(str::to_owned).collect());
            let wanted = |name: &str| {
                only.as_ref()
                    .is_none_or(|only| only.iter().any(|prefix| name.starts_with(prefix.as_str())))
            };
            stops.retain(|(name, _)| wanted(name));
            for (name, route) in stops {
                let _ = cx.update(|_, app| {
                    state.update(app, |state, cx| {
                        state.navigate(route);
                        state.request_refresh(cx);
                        cx.notify();
                    })
                });
                front(cx);
                settle(cx, 2500).await;
                front(cx);
                settle(cx, 300).await;
                capture(cx, &dir.join(format!("{name}.png")));
                if let Some((_, popover)) = filters.iter().find(|(page, _)| *page == name) {
                    filter_stops(cx, &dir, name, popover, &wanted).await;
                }
            }
            if wanted("14-commander") {
                let _ = cx.update(|_, app| {
                    state.update(app, |state, cx| {
                        state.navigate(Route::Roles);
                        state.set_commander_open(true);
                        cx.notify();
                    })
                });
                front(cx);
                settle(cx, 1500).await;
                capture(cx, &dir.join("14-commander.png"));
            }
            let _ = cx.update(|_, app| app.quit());
        })
        .detach();
}

/// The page's filter menu, open, then its bar with one pill, then its empty state; the page is
/// left closed and unfiltered.
async fn filter_stops(
    cx: &mut AsyncWindowContext,
    dir: &Path,
    page: &str,
    popover: &Entity<FilterPopover>,
    wanted: &impl Fn(&str) -> bool,
) {
    let menu = format!("{page}-filter-menu");
    if wanted(&menu) {
        let _ = cx.update(|_, app| {
            popover.update(app, |popover, cx| popover.open_from_shortcut(cx));
        });
        settle(cx, 600).await;
        capture(cx, &dir.join(format!("{menu}.png")));
        let _ = cx.update(|_, app| {
            popover.update(app, |popover, cx| popover.close(cx));
        });
        settle(cx, 400).await;
    }
    let bar = format!("{page}-filter-bar");
    if wanted(&bar) {
        let _ = cx.update(|_, app| {
            popover.update(app, |popover, cx| {
                let pick = popover.fields().first().and_then(|field| {
                    let option = field.spec.options.first()?;
                    Some((field.spec.id.clone(), option.id.clone()))
                });
                if let Some((field, value)) = pick {
                    popover.add_filter(&field, &[value], cx);
                }
            });
        });
        settle(cx, 600).await;
        capture(cx, &dir.join(format!("{bar}.png")));
    }
    let _ = cx.update(|_, app| {
        popover.update(app, |popover, cx| popover.clear(cx));
    });
    let empty = format!("{page}-empty");
    if wanted(&empty) {
        let _ = cx.update(|_, app| {
            popover.update(app, |popover, cx| {
                let field = popover.fields().first().map(|field| field.spec.id.clone());
                if let Some(field) = field {
                    popover.add_filter(&field, &["\u{1}".to_string()], cx);
                }
            });
        });
        settle(cx, 900).await;
        capture(cx, &dir.join(format!("{empty}.png")));
        let _ = cx.update(|_, app| {
            popover.update(app, |popover, cx| popover.clear(cx));
        });
    }
}

/// An occluded window stops drawing, so each shot brings the window forward.
fn front(cx: &mut AsyncWindowContext) {
    let _ = cx.update(|window, app| {
        app.activate(true);
        window.activate_window();
        window.refresh();
    });
}

async fn settle(cx: &mut AsyncWindowContext, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(cx: &mut AsyncWindowContext, path: &Path) {
    let _ = cx.update(|window, _| {
        if let Err(err) = save_window_png(window, path, None) {
            eprintln!("shot {}: {err}", path.display());
        }
    });
}

pub use polterware_ui::window::capture::{Crop, save_window_png};
