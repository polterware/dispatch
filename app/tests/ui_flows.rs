//! The app's window, driven as a person drives it: the real views in gpui's test window,
//! against a real core of the test's own with the fake ACP agent, clicked and typed into.
//! Buttons and switches are found by their ids, which gpui's test support maps to bounds.

mod common;

use common::*;
use dispatch_app::app::AppView;
use dispatch_app::state::{AppState, ConnectionState, Route, SettingsPage};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use std::path::Path;
use std::sync::Mutex;

/// The app finds its core through the environment, which the tests share: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Window<'a> {
    cx: &'a mut VisualTestContext,
    state: Entity<AppState>,
}

/// Opens the app's window on `core`.
#[allow(unsafe_code)]
fn open<'a>(core: &Core, cx: &'a mut TestAppContext) -> Window<'a> {
    // SAFETY: the tests that read the environment hold ONE_AT_A_TIME.
    unsafe { std::env::set_var("PWDISPATCH_DATA_DIR", &core.data) };
    cx.update(|cx| {
        cx.set_global(dispatch_app::theme::Theme::dark());
        polterware_ui::init(cx);
        dispatch_app::app::register_key_bindings(cx);
    });
    let (view, cx) = cx.add_window_view(AppView::new);
    let state = view.read_with(cx, |view, _| view.state());
    // Tall enough that every row of a page is on screen, where a click lands.
    cx.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(4000.0)));
    let mut window = Window { cx, state };
    window.settle();
    assert_eq!(
        window
            .state
            .read_with(window.cx, |s, _| s.connection().clone()),
        ConnectionState::Online,
        "the app did not reach the core"
    );
    window
}

impl Window<'_> {
    /// Lets the app load and draw, and its animations and timers run: the test clock moves
    /// only when told to.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.cx.run_until_parked();
            self.cx
                .executor()
                .advance_clock(std::time::Duration::from_millis(400));
        }
        self.cx.run_until_parked();
    }

    fn go(&mut self, route: Route) {
        self.state.update(self.cx, |state, cx| {
            state.navigate(route);
            state.request_refresh(cx);
            cx.notify();
        });
        self.settle();
    }

    fn route(&mut self) -> Route {
        self.state.read_with(self.cx, |s, _| s.route().clone())
    }

    fn toasts(&mut self) -> Vec<String> {
        self.state.read_with(self.cx, |s, _| {
            s.toasts().iter().map(|t| t.message.to_string()).collect()
        })
    }

    /// Clicks the element with this id, failing when it is not on screen.
    fn click(&mut self, id: &str) {
        let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
        let bounds = self
            .cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("no {id} on screen at {:?}", self.route_name()));
        self.cx.simulate_click(bounds.center(), Modifiers::none());
        self.settle();
    }

    /// Lets the springs play out: they run on the real clock and step only when the window
    /// draws, which a test window does when told to.
    fn pump(&mut self) {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            self.cx.update(|window, _| window.refresh());
            self.settle();
        }
    }

    /// Picks a row of the open menu and lets the menu leave, since while it leaves it still
    /// covers the rows under it.
    fn pick(&mut self, id: &str) {
        self.click(id);
        self.pump();
    }

    /// Scrolls the Settings section with the wheel until `id` is in its view, as a person
    /// scrolls to a row below the fold.
    fn scroll_settings_to(&mut self, id: &str) {
        let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
        for _ in 0..20 {
            let view = self
                .cx
                .debug_bounds("settings-content")
                .expect("the Settings section");
            let row = self
                .cx
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("no {id}"));
            if row.bottom() <= view.bottom() && row.top() >= view.top() {
                return;
            }
            self.cx.simulate_event(gpui::ScrollWheelEvent {
                position: view.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-200.0))),
                modifiers: Modifiers::none(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            self.settle();
        }
        panic!("{id} never scrolled into view");
    }

    fn shows(&mut self, id: &str) -> bool {
        let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
        self.cx.debug_bounds(id).is_some()
    }

    fn route_name(&mut self) -> String {
        format!(
            "{:?}",
            self.state.read_with(self.cx, |s, _| s.route().clone())
        )
    }

    /// Lets the app work until `done`, failing with its toasts after 30 seconds.
    fn until(&mut self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.settle();
            if done() {
                return;
            }
            if std::time::Instant::now() > deadline {
                let toasts = self.toasts();
                panic!("timed out waiting until {what}; the app said {toasts:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// Waits for the app to see the run's status, refreshing as its poll would.
    fn wait_for_status(&mut self, core: &Core, id: &str, want: &str) {
        self.until(&format!("the core reports {want}"), || {
            core.run(id).status == want
        });
        self.state
            .update(self.cx, |state, cx| state.request_refresh(cx));
        self.settle();
    }
}

#[gpui::test]
fn every_page_opens_with_data(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let write = core.start("WRITE a file", false);
    core.wait_for(&write, "COMPLETED");
    let read = core.start("FAIL please", true);
    core.wait_for(&read, "FAILED");
    let mut w = open(&core, cx);

    let runs = w.state.read_with(w.cx, |s, _| {
        s.runs().iter().map(|r| r.id.clone()).collect::<Vec<_>>()
    });
    assert!(runs.contains(&write) && runs.contains(&read), "{runs:?}");

    for route in [
        Route::Roles,
        Route::RoleBoard("explorer".into()),
        Route::Intelligence,
        Route::Spend,
        Route::Stats,
        Route::Runs,
        Route::Run(write.clone()),
        Route::Run(read.clone()),
        Route::Settings(SettingsPage::Agents),
        Route::Settings(SettingsPage::Classifier),
        Route::Settings(SettingsPage::PickRole),
        Route::Settings(SettingsPage::RoutingPolicy),
    ] {
        w.go(route.clone());
        assert_eq!(w.route(), route);
    }
    assert!(w.toasts().is_empty(), "a page complained: {:?}", w.toasts());
}

#[gpui::test]
fn a_core_with_no_runs_refreshes_without_an_error(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    core.cli(&["workspace", "add", &core.repo.to_string_lossy()]);
    let mut w = open(&core, cx);
    // The app opens on a new conversation.
    assert_eq!(w.route(), Route::NewRun);
    w.go(Route::Stats);
    let (runs, error) = w.state.read_with(w.cx, |s, _| {
        (s.runs().len(), s.last_error().map(str::to_owned))
    });
    assert_eq!(runs, 0);
    // The sidebar would say it could not refresh beside its empty state.
    assert_eq!(error, None);
}

#[gpui::test]
fn the_keyboard_opens_the_palette_settings_and_goes_back(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let mut w = open(&core, cx);
    w.go(Route::Stats);

    w.cx.simulate_keystrokes("cmd-k");
    w.settle();
    assert!(w.state.read_with(w.cx, |s, _| s.commander_open()));
    w.cx.simulate_keystrokes("cmd-k");
    w.settle();
    assert!(!w.state.read_with(w.cx, |s, _| s.commander_open()));

    // ⌘, opens Settings and, pressed again, closes it back onto the page.
    w.cx.simulate_keystrokes("cmd-,");
    w.settle();
    assert!(matches!(w.route(), Route::Settings(_)), "{:?}", w.route());
    w.cx.simulate_keystrokes("cmd-,");
    w.settle();
    assert_eq!(w.route(), Route::Stats);
    w.cx.simulate_keystrokes("cmd-,");
    w.settle();
    assert!(matches!(w.route(), Route::Settings(_)), "{:?}", w.route());
    w.cx.simulate_keystrokes("cmd-[");
    w.settle();
    assert_eq!(w.route(), Route::Stats);
}

#[gpui::test]
fn the_run_page_merges_then_deletes_the_worktree(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let id = core.start("WRITE a file", false);
    let run = core.wait_for(&id, "COMPLETED");
    let mut w = open(&core, cx);
    w.go(Route::Run(id.clone()));

    assert!(
        w.shows("run-merge"),
        "a finished run with changes offers no Merge"
    );
    w.click("run-merge");
    w.until("the merge lands", || !core.run(&id).merged_at.is_empty());
    w.settle();
    assert!(core.repo.join("e2e.txt").exists());
    assert!(
        w.toasts().iter().any(|t| t == "Run merged."),
        "{:?}",
        w.toasts()
    );

    w.state.update(w.cx, |state, cx| state.request_refresh(cx));
    w.settle();
    assert!(
        w.shows("run-delete-worktree"),
        "a merged run offers no Delete"
    );
    w.click("run-delete-worktree");
    w.until("the worktree goes", || !Path::new(&run.worktree).exists());
    w.settle();
    assert!(
        w.toasts().iter().any(|t| t == "Worktree deleted."),
        "{:?}",
        w.toasts()
    );
}

#[gpui::test]
fn the_run_page_sends_a_message_to_an_ended_run(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let id = core.start("say hi", true);
    core.wait_for(&id, "COMPLETED");
    let mut w = open(&core, cx);
    w.go(Route::Run(id.clone()));

    assert!(w.shows("run-message"), "an ended run offers no composer");
    // A press on the card writes in its field; Enter sends what it holds.
    w.click("run-message");
    w.cx.simulate_input("now say bye");
    w.settle();
    w.click("run-message-send");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let next = loop {
        w.settle();
        if let Route::Run(open) = w.route()
            && open != id
        {
            break open;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the message never opened its run; the app said {:?}",
            w.toasts()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let sent = core.wait_for(&next, "COMPLETED");
    assert_eq!(sent.resumed_of, id);
    assert_eq!(sent.prompt, "now say bye");
    // The chat goes on in the same transcript: the first message stays over the new one.
    w.settle();
    assert!(
        w.shows("transcript-prompt:say hi"),
        "the chat lost its first message"
    );
    assert!(
        w.shows("transcript-prompt:now say bye"),
        "the chat never showed the message sent"
    );
}

/// From the Runs page: a running run's own page follows its live output, which the test's
/// one-thread executor would wait on until the run ends.
#[gpui::test]
fn the_runs_page_cancels_a_running_run(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let id = core.start("SLOW answer", true);
    core.wait_for(&id, "RUNNING");
    let mut w = open(&core, cx);
    w.go(Route::Runs);

    let cancel = format!("run-cancel-{id}");
    assert!(w.shows(&cancel), "a running run offers no Cancel");
    w.click(&cancel);
    w.wait_for_status(&core, &id, "CANCELLED");
    let state = w.state.clone();
    let seen = |w: &mut Window| {
        state.read_with(w.cx, |s, _| {
            s.runs()
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.status.clone())
        })
    };
    let mut tries = 0;
    while seen(&mut w).as_deref() != Some("CANCELLED") && tries < 20 {
        w.state.update(w.cx, |state, cx| state.request_refresh(cx));
        w.settle();
        tries += 1;
    }
    assert_eq!(
        seen(&mut w).as_deref(),
        Some("CANCELLED"),
        "the app never saw the cancel"
    );
}

#[gpui::test]
fn settings_switch_an_agents_hook(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let hook_on = |agent: &str| {
        core.setup()["hooks"]
            .as_array()
            .and_then(|h| h.iter().find(|h| h["agent"] == agent))
            .map(|h| h["on"] == true)
    };
    assert_eq!(hook_on("claude"), Some(false));
    let mut w = open(&core, cx);
    w.go(Route::Settings(SettingsPage::Agents));

    w.scroll_settings_to("settings-hook-claude");
    w.click("settings-hook-claude");
    w.until("the hook is on", || hook_on("claude") == Some(true));
    assert!(
        std::fs::read_to_string(core.home.join(".claude/settings.json"))
            .unwrap()
            .contains("SessionStart")
    );
    w.click("settings-hook-claude");
    w.until("the hook is off", || hook_on("claude") == Some(false));
}

#[gpui::test]
fn settings_switch_an_agent_on_and_off(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let enabled = |agent: &str| {
        core.client
            .list_agents(&dispatch_app::client::ListAgentsRequest::default())
            .unwrap()
            .agents
            .iter()
            .any(|a| a.name == agent && a.enabled)
    };
    assert!(enabled("fake"));
    let mut w = open(&core, cx);
    w.go(Route::Settings(SettingsPage::Agents));

    w.scroll_settings_to("settings-agent-fake");
    w.click("settings-agent-fake");
    w.until("the agent is disabled", || !enabled("fake"));
    w.click("settings-agent-fake");
    w.until("the agent is enabled", || enabled("fake"));
    assert!(w.toasts().is_empty(), "{:?}", w.toasts());
}

#[gpui::test]
fn settings_install_and_remove_a_skill(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let shared = core.home.join(".agents/skills");
    let skill = shared.join("dispatch/SKILL.md");
    let mut w = open(&core, cx);
    w.go(Route::Settings(SettingsPage::Agents));

    let install = format!("settings-skill-{}", shared.display());
    w.scroll_settings_to(&install);
    w.click(&install);
    w.until("the skill is installed", || skill.exists());
    let remove = format!("settings-skill-remove-{}", shared.display());
    w.scroll_settings_to(&remove);
    w.click(&remove);
    w.until("the skill is removed", || !skill.exists());
    assert!(
        w.toasts().iter().any(|t| t == "Skill removed."),
        "{:?}",
        w.toasts()
    );
}

#[gpui::test]
fn settings_switch_the_routing_policy(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    assert_eq!(core.config("get", "discover", &[]), true);
    assert_eq!(core.config("get", "roles", &[]), true);
    let mut w = open(&core, cx);

    w.go(Route::Settings(SettingsPage::RoutingPolicy));
    w.scroll_settings_to("settings-discover");
    w.click("settings-discover");
    w.until("discovery is off", || {
        core.config("get", "discover", &[]) == false
    });
    w.click("settings-discover");
    w.until("discovery is on", || {
        core.config("get", "discover", &[]) == true
    });

    w.go(Route::Settings(SettingsPage::PickRole));
    w.scroll_settings_to("settings-pick-role");
    w.click("settings-pick-role");
    w.until("the role pick is off", || {
        core.config("get", "roles", &[]) == false
    });
}

#[gpui::test]
fn settings_menus_pick_the_priority_and_classifier_model(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    core.cli(&["workspace", "add", &core.repo.to_string_lossy()]);
    core.config("set", "classifier", &["cli"]);
    let global = core.data.join("dispatch").join("config.json");
    let mut w = open(&core, cx);

    // The policy is global: with a repository added, the priority still goes to the one file.
    w.go(Route::Settings(SettingsPage::RoutingPolicy));
    w.click("settings-priority");
    w.pick("menu-item-2");
    w.until("the policy's priority is quality", || {
        std::fs::read_to_string(&global).is_ok_and(|body| body.contains("quality"))
    });
    assert!(!core.repo.join(".dispatch").join("config.json").exists());
    w.click("settings-priority");
    w.pick("menu-item-1");
    w.until("the policy's priority is speed", || {
        std::fs::read_to_string(&global).is_ok_and(|body| body.contains("speed"))
    });

    // The CLI classifier asks the model typed into its picker.
    w.go(Route::Settings(SettingsPage::Classifier));
    // The picked option's editor opens on a spring.
    w.pump();
    w.click("settings-cli-model");
    w.pump();
    w.click("settings-cli-filter");
    w.cx.simulate_input("fake/fake-mini");
    w.cx.simulate_keystrokes("enter");
    w.until("the classifier asks fake-mini", || {
        core.config("get", "classifier_agent", &[]) == "fake/fake-mini"
    });
    assert_eq!(
        core.config("get", "classifier_cli", &[]),
        serde_json::json!({"agent": "fake", "model": "fake-mini"})
    );
    assert!(w.toasts().is_empty(), "{:?}", w.toasts());
}

#[gpui::test]
fn settings_models_switch_models_and_providers(cx: &mut TestAppContext) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let core = Core::new();
    let mut w = open(&core, cx);
    w.go(Route::Settings(SettingsPage::Models));
    let enabled = |id: &str| {
        core.client
            .list_models(&dispatch_app::client::ListModelsRequest::default())
            .unwrap()
            .models
            .iter()
            .any(|m| m.agent == "fake" && m.model == id)
    };
    w.scroll_settings_to("settings-model-fake-fake-fast");
    w.click("settings-model-fake-fake-fast");
    w.until("the model is disabled", || !enabled("fake-fast"));
    assert!(enabled("fake-deep"));
    w.click("settings-model-fake-fake-fast");
    w.until("the model is enabled", || enabled("fake-fast"));
    w.scroll_settings_to("settings-provider-fake");
    w.click("settings-provider-fake");
    w.until("the provider is disabled", || {
        !enabled("fake-fast") && !enabled("fake-deep")
    });
    w.click("settings-provider-fake");
    w.until("the provider is enabled", || {
        enabled("fake-fast") && enabled("fake-deep")
    });
    assert!(w.toasts().is_empty(), "{:?}", w.toasts());
}
