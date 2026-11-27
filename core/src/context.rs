//! [`Context`]: what a part of the core needs from the rest of it for one
//! call.
//!
//! The Profile registry, the Limit monitor, the notifier and the Desktop
//! bridge each own their state but share the core's environment, paths, log
//! and Safety pauses. The core lends them those for one call at a time, so
//! its other parts stay free to change meanwhile.

use std::path::Path;

use crate::env::Environment;
use crate::interface::SafetyFeature;
use crate::logging::{Field, Level, Logger};
use crate::runner::Runner;
use crate::storage::AppPaths;

/// What a part of the core needs from the core for one call.
pub(crate) struct Context<'a> {
    pub env: &'a Environment,
    pub paths: &'a AppPaths,
    pub logger: &'a Logger,
    pub verbose: bool,
    /// The write features under a Safety pause: nothing they write is
    /// written.
    pub paused: Vec<SafetyFeature>,
    /// The `claude` spawns run; `None` while none is found.
    pub claude: Option<&'a Path>,
}

impl Context<'_> {
    pub(crate) fn is_paused(&self, feature: SafetyFeature) -> bool {
        self.paused.contains(&feature)
    }

    pub(crate) fn log(
        &self,
        level: Level,
        message: &'static str,
        fields: &[(&'static str, Field<'_>)],
    ) {
        self.logger
            .write(self.env.clock.now(), self.verbose, level, message, fields);
    }

    pub(crate) fn runner(&self) -> Runner<'_> {
        Runner::new(self.env, self.paths, self.claude)
    }
}
