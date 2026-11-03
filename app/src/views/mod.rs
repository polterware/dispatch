//! One file per screen. Later runs replace exactly one of these and leave the others alone.

mod commander;
mod intelligence;
mod roles;
mod run_page;
mod runs;
mod settings;
mod sidebar;
mod spend;
mod stats;
mod transcript;

pub use commander::CommanderView;
pub use intelligence::IntelligenceView;
pub use roles::RolesView;
pub use run_page::RunPageView;
pub use runs::RunsView;
pub use settings::SettingsView;
pub use sidebar::SidebarView;
pub use spend::SpendView;
pub use stats::StatsView;
// Not mounted. Run pages construct it later without editing this module's other lines.
#[allow(unused_imports)]
pub use transcript::TranscriptView;
