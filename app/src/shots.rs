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
//!
//! `03-intelligence-detail` opens the first model's dialog, as a click on its row does.
//!
//! `DISPATCH_GPUI_SHOTS_RUN=<id>` picks the run `13-run` opens, whose `13-run-earlier` scrolls
//! to the top, where a long run reads the page before.
//!
//! `00-launch` captures the first page 0.2s to 3s after the window opens, while its
//! data is still on the way.
//!
//! `14-commander-again` presses ⌘K to close the palette and, once it has played out, again
//! to open it; `14-settings-shortcut` then presses ⌘, for Settings.
//!
//! `18-settings-panels` opens Settings' menus and pickers over the dialog: Routing policy's
//! Priority menu (`-priority`) and Allow models picker (`-tokens`), then the Classifier set to an agent CLI with its model picker (`-cli`); the
//! last writes the classifier, so it is for a scratch data folder.
//!
//! `15-display-menu` opens the sidebar's Display menu on its Group by submenu, and
//! `16-role-composer` opens the Composer on the explorer role, and `16-role-composer-tokens`
//! fills its Prefer and Avoid.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, AsyncWindowContext, Entity, Window};

use crate::state::{AppState, Route, SettingsPage};
use crate::ui::FilterPopover;
use crate::views::StatsPane as Pane;
use crate::views::{RolesView, SettingsView, SidebarView};

/// A page's filters, for the stops that open its menu and add a pill.
pub type PageFilters = Vec<(&'static str, Entity<FilterPopover>)>;

/// The views the tour opens menus and dialogs on.
pub struct TourViews {
    pub sidebar: Entity<SidebarView>,
    pub roles: Entity<RolesView>,
    pub run_page: Entity<crate::views::RunPageView>,
    pub stats: Entity<crate::views::StatsView>,
    pub intelligence: Entity<crate::views::IntelligenceView>,
    pub settings: Entity<SettingsView>,
}

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
    views: TourViews,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            let _ = std::fs::create_dir_all(&dir);
            // `00-launch-<ms>`: the first page while its data is still on the way.
            let launch = std::env::var("DISPATCH_GPUI_SHOTS_ONLY")
                .is_ok_and(|only| only.contains("00-launch"));
            let mut waited = 0;
            for ms in [200u64, 400, 1000, 3000] {
                if !launch {
                    break;
                }
                settle(cx, ms - waited).await;
                waited = ms;
                front(cx);
                capture(cx, &dir.join(format!("00-launch-{ms}.png")));
            }
            settle(cx, 6000).await;
            // `DISPATCH_GPUI_SHOTS_RUN=<id>` opens that run at `13-run` instead of the newest.
            let picked = std::env::var("DISPATCH_GPUI_SHOTS_RUN")
                .ok()
                .filter(|id| !id.is_empty());
            let first_run = picked.or_else(|| {
                cx.update(|_, app| state.read(app).runs().first().map(|run| run.id.clone()))
                    .ok()
                    .flatten()
            });
            let mut stops: Vec<(&str, Route)> = vec![
                ("01-roles", Route::Roles),
                ("02-role-board-general", Route::RoleBoard("general".into())),
                (
                    "02-role-board-explorer",
                    Route::RoleBoard("explorer".into()),
                ),
                ("03-intelligence", Route::Intelligence),
                ("04-spend", Route::Spend),
                ("05-stats", Route::Stats),
                ("06-settings-agents", Route::Settings(SettingsPage::Agents)),
                (
                    "08-settings-classifier",
                    Route::Settings(SettingsPage::Classifier),
                ),
                (
                    "09-settings-pick-role",
                    Route::Settings(SettingsPage::PickRole),
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
                        state.navigate(route.clone());
                        state.request_refresh(cx);
                        cx.notify();
                    })
                });
                front(cx);
                settle(cx, 2500).await;
                front(cx);
                settle(cx, 300).await;
                capture(cx, &dir.join(format!("{name}.png")));
                if name == "01-roles" && wanted("01-roles-return") {
                    // Away and back, with no refresh of the tour's own: the page draws at once.
                    for (route, wait) in [(Route::Stats, 800), (Route::Roles, 250)] {
                        let _ = cx.update(|_, app| {
                            state.update(app, |state, cx| {
                                state.navigate(route);
                                cx.notify();
                            })
                        });
                        settle(cx, wait).await;
                    }
                    capture(cx, &dir.join("01-roles-return.png"));
                }
                if name == "03-intelligence" && wanted("03-intelligence-detail") {
                    let _ = cx.update(|_, app| {
                        views
                            .intelligence
                            .update(app, |view, cx| view.open_first_detail(cx));
                    });
                    settle(cx, 900).await;
                    capture(cx, &dir.join("03-intelligence-detail.png"));
                    let _ = cx.update(|_, app| {
                        views
                            .intelligence
                            .update(app, |view, cx| view.close_detail(cx));
                    });
                    settle(cx, 600).await;
                }
                if let Some((_, popover)) = filters.iter().find(|(page, _)| *page == name) {
                    filter_stops(cx, &dir, name, popover, &wanted).await;
                }
                if name == "05-stats" {
                    // The chart by week, its window moved to weeks with it.
                    let chart = cx
                        .update(|_, app| views.stats.read(app).filters(Pane::Chart))
                        .expect("stats filters");
                    if wanted("05-stats-chart-week") {
                        let _ = cx.update(|_, app| {
                            chart.update(app, |popover, cx| {
                                popover.set_filter("by", &["week".to_string()], cx);
                            });
                        });
                        settle(cx, 1500).await;
                        capture(cx, &dir.join("05-stats-chart-week.png"));
                    }
                }
                if name == "13-run" {
                    // The rail's properties, with the Agent group above them folded away.
                    let _ = cx.update(|_, app| {
                        views
                            .run_page
                            .update(app, |page, cx| page.fold_agent_group(cx));
                    });
                    settle(cx, 800).await;
                    capture(cx, &dir.join("13-run-properties.png"));
                    let _ = cx.update(|_, app| {
                        views
                            .run_page
                            .update(app, |page, cx| page.scroll_transcript(400.0, cx));
                        views
                            .sidebar
                            .update(app, |sidebar, cx| sidebar.scroll_to(190.0, cx));
                    });
                    settle(cx, 800).await;
                    capture(cx, &dir.join("13-run-scrolled.png"));
                    // The jump-to-end button, pressed: the text glides to its end and the
                    // button leaves.
                    let _ = cx.update(|_, app| {
                        views
                            .run_page
                            .update(app, |page, cx| page.jump_transcript_to_end(cx));
                    });
                    settle(cx, 1200).await;
                    capture(cx, &dir.join("13-run-end.png"));
                    // Up to the top: a run longer than its first page reads the page before,
                    // and the row on screen stays where it was.
                    let _ = cx.update(|_, app| {
                        views
                            .run_page
                            .update(app, |page, cx| page.scroll_transcript(-1.0e9, cx));
                    });
                    settle(cx, 4000).await;
                    capture(cx, &dir.join("13-run-earlier.png"));
                    // The run's transcript card, from a pointer resting on its row.
                    if let Route::Run(id) = route.clone() {
                        let _ = cx.update(|_, app| {
                            views.sidebar.update(app, |sidebar, cx| {
                                sidebar.scroll_to(0.0, cx);
                                sidebar.hover_run(id, cx);
                            });
                        });
                        settle(cx, 2200).await;
                        capture(cx, &dir.join("13-run-preview.png"));
                    }
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
                if wanted("14-commander-again") {
                    // ⌘K closes it, and once it has played out, ⌘K opens it again.
                    for wait in [1500, 600] {
                        let _ = cx.update(|window, app| {
                            window.dispatch_keystroke(
                                gpui::Keystroke::parse("cmd-k").expect("cmd-k"),
                                app,
                            );
                        });
                        settle(cx, wait).await;
                        let open = cx
                            .update(|_, app| state.read(app).commander_open())
                            .unwrap_or(false);
                        eprintln!("shot 14-commander-again: open {open}");
                    }
                    capture(cx, &dir.join("14-commander-again.png"));
                }
                if wanted("14-settings-shortcut") {
                    // ⌘, opens Settings over the page, the palette closing first.
                    let _ = cx.update(|window, app| {
                        window.dispatch_keystroke(
                            gpui::Keystroke::parse("cmd-,").expect("cmd-,"),
                            app,
                        );
                    });
                    settle(cx, 1200).await;
                    let open = cx
                        .update(|_, app| matches!(state.read(app).route(), Route::Settings(_)))
                        .unwrap_or(false);
                    eprintln!("shot 14-settings-shortcut: settings {open}");
                    capture(cx, &dir.join("14-settings-shortcut.png"));
                    let _ = cx.update(|_, app| {
                        state.update(app, |state, cx| {
                            state.close_settings();
                            cx.notify();
                        })
                    });
                    settle(cx, 600).await;
                }
            }
            if wanted("15-display-menu") {
                let _ = cx.update(|_, app| {
                    state.update(app, |state, cx| {
                        state.set_commander_open(false);
                        state.navigate(Route::Roles);
                        cx.notify();
                    });
                    views.sidebar.update(app, |sidebar, cx| {
                        sidebar.toggle_display(cx);
                        sidebar.highlight_menu_row(0, cx);
                    });
                });
                front(cx);
                settle(cx, 800).await;
                capture(cx, &dir.join("15-display-menu.png"));
                let _ = cx.update(|_, app| {
                    views
                        .sidebar
                        .update(app, |sidebar, cx| sidebar.toggle_display(cx));
                });
                settle(cx, 400).await;
            }
            if wanted("17-model-palette") {
                let _ = cx.update(|_, app| {
                    state.update(app, |state, cx| {
                        state.set_commander_open(false);
                        state.navigate(Route::RoleBoard("explorer".into()));
                        state.request_refresh(cx);
                        cx.notify();
                    });
                });
                front(cx);
                settle(cx, 2500).await;
                let _ = cx.update(|_, app| {
                    views
                        .roles
                        .update(app, |roles, cx| roles.open_palette("easy", 0, cx));
                });
                settle(cx, 3000).await;
                capture(cx, &dir.join("17-model-palette.png"));
            }
            if wanted("16-role-composer") {
                let _ = cx.update(|_, app| {
                    state.update(app, |state, cx| {
                        state.set_commander_open(false);
                        state.navigate(Route::Roles);
                        cx.notify();
                    });
                    views
                        .roles
                        .update(app, |roles, cx| roles.open_composer(Some("explorer"), cx));
                });
                front(cx);
                settle(cx, 1200).await;
                capture(cx, &dir.join("16-role-composer.png"));
                if wanted("16-role-composer-tokens") {
                    let _ = cx.update(|_, app| {
                        views.roles.update(app, |roles, cx| {
                            roles.draft_tokens(&["claude", "codex", "copilot"], &["cursor"], cx)
                        });
                    });
                    settle(cx, 600).await;
                    capture(cx, &dir.join("16-role-composer-tokens.png"));
                    // Prefer's checkboxes, open over the Composer.
                    let _ = cx.update(|window, app| {
                        views
                            .roles
                            .update(app, |roles, cx| roles.open_pill(3, window, cx));
                    });
                    settle(cx, 800).await;
                    capture(cx, &dir.join("16-role-composer-prefer.png"));
                    let _ = cx.update(|window, app| {
                        views
                            .roles
                            .update(app, |roles, cx| roles.open_pill(3, window, cx));
                    });
                    settle(cx, 400).await;
                }
                let _ = cx.update(|window, app| {
                    views
                        .roles
                        .update(app, |roles, cx| roles.open_pill(0, window, cx));
                });
                settle(cx, 800).await;
                capture(cx, &dir.join("16-role-composer-picker.png"));
            }
            if wanted("18-settings-panels") {
                let settings = views.settings.clone();
                let go = |page: SettingsPage, cx: &mut AsyncWindowContext| {
                    let _ = cx.update(|_, app| {
                        state.update(app, |state, cx| {
                            state.set_commander_open(false);
                            state.navigate(Route::Settings(page));
                            state.request_refresh(cx);
                            cx.notify();
                        })
                    });
                };
                go(SettingsPage::RoutingPolicy, cx);
                front(cx);
                settle(cx, 2500).await;
                for (name, open) in [
                    ("18-settings-panels-priority", 0usize),
                    ("18-settings-panels-tokens", 1),
                ] {
                    let _ = cx.update(|_, app| {
                        settings.update(app, |settings, cx| settings.tour_open(open, cx))
                    });
                    settle(cx, 2500).await;
                    capture(cx, &dir.join(format!("{name}.png")));
                    let _ = cx.update(|_, app| {
                        settings.update(app, |settings, cx| settings.tour_close(cx))
                    });
                    settle(cx, 500).await;
                }
                go(SettingsPage::Classifier, cx);
                settle(cx, 1500).await;
                let _ = cx.update(|_, app| {
                    settings.update(app, |settings, cx| settings.tour_open(2, cx))
                });
                settle(cx, 2500).await;
                let _ = cx.update(|_, app| {
                    settings.update(app, |settings, cx| settings.tour_open(3, cx))
                });
                settle(cx, 2500).await;
                capture(cx, &dir.join("18-settings-panels-cli.png"));
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
