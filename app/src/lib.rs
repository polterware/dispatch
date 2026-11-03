//! Polterware Dispatch in GPUI: the app (`run`), its pages, and its client of the core.
#![allow(dead_code)]

pub mod app;
pub use polterware_ui::bots as avatar;
pub mod client;
mod dock_icon;
pub mod format;
pub mod models;
pub use polterware_ui::motion;
pub mod shots;
pub mod state;
pub mod theme;
pub mod ui;
pub mod updater;
pub use polterware_ui::window::vibrancy;
pub mod views;
pub use polterware_ui::window::window_drag;

use app::{CheckForUpdates, Quit, register_key_bindings};
use gpui::{App, AppContext as _, Application, Menu, MenuItem, px, size};
use theme::Theme;
use ui::IconAssets;

/// Opens the Dispatch window and runs the app until it quits.
pub fn run() {
    Application::new()
        .with_assets(IconAssets)
        .run(|cx: &mut App| {
            dock_icon::install();
            cx.activate(true);
            theme::set_root_rem(theme::REM);
            theme::set_status_tones(theme::dispatch_status_tone);
            cx.set_global(Theme::dark());
            polterware_ui::init(cx);
            register_key_bindings(cx);
            cx.bind_keys(views::CommanderView::key_bindings());
            let updates = updater::start().is_ok();
            let mut items = Vec::new();
            if updates {
                items.push(MenuItem::action("Check for Updates…", CheckForUpdates));
                items.push(MenuItem::separator());
            }
            items.push(MenuItem::action("Quit", Quit));
            cx.set_menus(vec![Menu {
                name: "Dispatch".into(),
                items,
            }]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let options = polterware_ui::window::window_options(
                "Dispatch",
                size(px(1200.0), px(800.0)),
                size(px(800.0), px(560.0)),
                cx,
            );

            cx.open_window(options, |window, cx| {
                vibrancy::install(window);
                cx.new(|cx| app::AppView::new(window, cx))
            })
            .expect("failed to open GPUI window");
        });
}
