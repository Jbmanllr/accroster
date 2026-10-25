//! AccRoster's core: all of the app's behaviour, as a plain Rust library.
//!
//! The app is one process with two parts. This crate is the core; the
//! `accroster` binary is a thin AppKit layer that only renders what the core
//! says and forwards clicks. See [`interface`] for the contract between them
//! and [`env`] for the boundary between the core and the rest of the Mac.

mod backups;
mod checks;
mod claude_json;
mod cli_switcher;
mod core;
mod desktop;
mod discovered;
pub mod driver;
pub mod env;
pub mod interface;
mod limit_offers;
mod logging;
mod monitor;
mod notifications;
mod opening;
mod profiles;
mod runner;
mod safety;
mod sessions;
mod settings;
mod sidebar_sync;
mod storage;
mod usage;
pub mod watcher;

pub use crate::core::{Core, LaunchError};
pub use env::{
    Clock, Environment, OsFacts, ProcessFacts, ProcessSpawner, RunningProcess, SpawnOutput,
    SpawnRequest, SystemClock, SystemOsFacts, SystemSpawner,
};
pub use env::{LoginItem, LoginItemStatus, SystemLoginItem};
pub use interface::{
    AboutView, Badge, Command, Effect, Event, LimitBar, MOST_HEADROOM, MenuBarView, MenuItem,
    MenuView, ProfileRow, SettingsView, View,
};
pub use interface::{
    AllSessionsView, OpenChoice, RunningDot, SessionDetail, SessionFilter, SessionRow,
    SidebarGroup, SidebarItem, SidebarPresence,
};
pub use interface::{CheckRow, CheckStatus, DiagnosticsView, PausedFeature, SafetyFeature};
pub use interface::{
    ClaudeVersions, CliSwitchingView, DiscoveredRow, FirstRunView, HiddenDiscovered,
    LaunchAtLoginView, NicknameTarget, NotificationsView, ProfileKey, ProfileSettings, Toggle,
};
pub use interface::{NotificationAction, SidebarSyncMode, SidebarSyncView, UninstallView};
pub use logging::install_panic_hook;
pub use storage::{AppPaths, BUNDLE_ID};
