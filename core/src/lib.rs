//! AccRoster's core: all of the app's behaviour, as a plain Rust library.
//!
//! The app is one process with two parts. This crate is the core; the
//! `accroster` binary is a thin AppKit layer that only renders what the core
//! says and forwards clicks. See [`interface`] for the contract between them
//! and [`env`] for the boundary between the core and the rest of the Mac.

mod backups;
mod cli_switcher;
mod core;
mod discovered;
pub mod driver;
pub mod env;
pub mod interface;
mod limit_offers;
mod logging;
mod monitor;
mod profiles;
mod runner;
mod settings;
mod storage;
mod usage;

pub use crate::core::{Core, LaunchError};
pub use env::{
    Clock, Environment, OsFacts, ProcessFacts, ProcessSpawner, RunningProcess, SpawnOutput,
    SpawnRequest, SystemClock, SystemOsFacts, SystemSpawner,
};
pub use env::{LoginItem, LoginItemStatus, SystemLoginItem};
pub use interface::{
    AboutView, Command, Effect, Event, LimitBar, MenuBarView, MenuItem, MenuView, ProfileRow,
    SettingsView, View,
};
pub use interface::{
    CliSwitchingView, DiscoveredPair, DiscoveredRow, FirstRunView, HiddenDiscovered,
    LaunchAtLoginView, NicknameTarget,
};
pub use logging::install_panic_hook;
pub use storage::{AppPaths, BUNDLE_ID};
