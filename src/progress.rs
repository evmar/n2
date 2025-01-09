//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use crate::{graph::Build, graph::BuildId, task::TaskResult, work::StateCounts};

/// Compute the message to display on the console for a given build.
pub fn build_message(build: &Build) -> &str {
    build
        .desc
        .as_ref()
        .filter(|desc| !desc.is_empty())
        .unwrap_or_else(|| build.cmdline.as_ref().unwrap())
}

/// Trait for build progress notifications.
pub trait Progress {
    /// Called as individual build tasks progress through build states.
    fn update(&self, counts: &StateCounts);

    /// Called when a task starts.
    fn task_started(&self, id: BuildId, build: &Build);

    /// Called when a task's last line of output changes.
    fn task_output(&self, id: BuildId, line: Vec<u8>);

    /// Called when a task completes.
    fn task_finished(&self, id: BuildId, build: &Build, result: &TaskResult);

    /// Log a line of output without corrupting the progress display.
    /// This line is persisted beyond further progress updates.  For example,
    /// used when a task fails; we want the final output to show that failed
    /// task's output even if we do more work after it fails.
    fn log(&self, msg: &str);
}
