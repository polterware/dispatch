//! One file per screen. Later runs replace exactly one of these and leave the others alone.

mod commander;
mod conversation_usage;
mod image_viewer;
mod intelligence;
pub mod repository_actions;
mod roles;
mod run_page;
mod runs;
mod settings;
mod sidebar;
mod spend;
mod stats;
mod transcript;

pub use commander::CommanderView;
pub use conversation_usage::ConversationUsageView;
pub use image_viewer::ImageViewerView;

/// What the core said when a call failed, in words for a toast.
pub(crate) fn core_message(error: crate::client::CoreError) -> String {
    match error {
        crate::client::CoreError::NotRunning => "Dispatch core is not running".to_string(),
        crate::client::CoreError::Rpc(message) => message,
    }
}
pub use intelligence::IntelligenceView;
pub use roles::RolesView;
pub use run_page::RunPageView;
pub use runs::RunsView;
pub use settings::SettingsView;
pub use sidebar::SidebarView;
pub use spend::SpendView;
pub use stats::{Pane as StatsPane, StatsView};
// Not mounted. Run pages construct it later without editing this module's other lines.
#[allow(unused_imports)]
pub use transcript::TranscriptView;
