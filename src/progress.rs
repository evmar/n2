//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use crate::graph::Build;
use crate::graph::BuildId;
use crate::work::BuildState;
use crate::work::StateCounts;

#[allow(clippy::uninit_assumed_init)]
pub fn get_terminal_cols() -> Option<usize> {
    unsafe {
        let mut winsize: libc::winsize = std::mem::MaybeUninit::uninit().assume_init();
        if libc::ioctl(0, libc::TIOCGWINSZ, &mut winsize) < 0 {
            return None;
        }
        Some(winsize.ws_col as usize)
    }
}

/// Compute the message to display on the console for a given build.
fn build_message(build: &Build) -> &str {
    build
        .desc
        .as_ref()
        .unwrap_or_else(|| build.cmdline.as_ref().unwrap())
}

/// Trait for build progress notifications.
pub trait Progress {
    /// Called as individual build tasks progress through build states.
    fn update(&mut self, counts: &StateCounts);

    /// Called when we expect to be waiting for a while before another update.
    fn flush(&mut self);

    /// Called when a task starts.
    /// Not called for every BuildId, just the ones that start and complete.
    fn task_state(&mut self, id: BuildId, build: &Build, state: BuildState);

    /// Called when a build has failed.
    /// TODO: maybe this should just be part of task_state?
    /// In particular, consider the case where builds output progress as they run,
    /// as well as the case where multiple build steps are allowed to fail.
    fn failed(&mut self, build: &Build, output: &[u8]);

    /// Called when the overall build has completed (success or failure), to allow
    /// cleaning up the display.
    fn finish(&mut self);
}

/// Currently running build task, as tracked for progress updates.
struct Task {
    id: BuildId,
    /// When the task started running.
    start: Instant,
    /// Build status message for the task.
    message: String,
}

/// Console progress pretty-printer.
/// Each time it prints, it clears from the cursor to the end of the console,
/// prints the status text, and then moves moves the cursor back up to the
/// start position.  This means on errors etc. we can clear any status by
/// clearing the console too.
pub struct ConsoleProgress {
    /// Last time we updated the console, used to throttle updates.
    last_update: Instant,
    /// Counts of tasks in each state.  TODO: pass this as function args?
    counts: StateCounts,
    /// Build tasks that are currently executing.
    /// Pushed to as tasks are started, so it's always in order of age.
    tasks: VecDeque<Task>,
    /// Count of build tasks that have finished.
    pub tasks_done: usize,
}

#[allow(clippy::new_without_default)]
impl ConsoleProgress {
    pub fn new() -> Self {
        ConsoleProgress {
            // Act like our last update was now, so that we delay slightly
            // before our first print.  This reduces flicker in the case where
            // the work immediately.
            last_update: Instant::now(),
            counts: StateCounts::new(),
            tasks: VecDeque::new(),
            tasks_done: 0,
        }
    }
}

impl Progress for ConsoleProgress {
    fn update(&mut self, counts: &StateCounts) {
        self.counts = counts.clone();
        self.maybe_print();
    }

    fn task_state(&mut self, id: BuildId, build: &Build, state: BuildState) {
        match state {
            BuildState::Running => {
                let message = build_message(build);
                self.tasks.push_back(Task {
                    id,
                    start: Instant::now(),
                    message: message.to_string(),
                });
            }
            BuildState::Done => {
                self.tasks
                    .remove(self.tasks.iter().position(|t| t.id == id).unwrap());
                self.tasks_done += 1;
            }
            _ => {}
        }
        self.maybe_print();
    }

    fn flush(&mut self) {
        self.print();
    }

    fn failed(&mut self, build: &Build, output: &[u8]) {
        let message = build_message(build);
        // If the user hit ctl-c, it may have printed something on the line.
        // So \r to go to first column first, then the same clear we use elsewhere.
        println!("\r\x1b[Jfailed: {}", message);
        println!("{}", String::from_utf8_lossy(output));
    }

    fn finish(&mut self) {
        print!("\x1b[J");
    }
}

impl ConsoleProgress {
    fn progress_bar(&self) -> String {
        let bar_size = 40;
        let mut bar = String::with_capacity(bar_size);
        let mut sum: usize = 0;
        let total = self.counts.total();
        for (count, ch) in [
            (self.counts.get(BuildState::Done), '='),
            (
                self.counts.get(BuildState::Queued)
                    + self.counts.get(BuildState::Running)
                    + self.counts.get(BuildState::Ready),
                '-',
            ),
            (self.counts.get(BuildState::Want), ' '),
        ] {
            sum += count;
            while bar.len() <= (sum * bar_size / total) {
                bar.push(ch);
            }
        }
        bar
    }

    fn print(&self) {
        println!(
            "\x1b[J[{}] {}/{} done, {}/{} running",
            self.progress_bar(),
            self.counts.get(BuildState::Done),
            self.counts.total(),
            self.tasks.len(),
            self.counts.get(BuildState::Queued) + self.tasks.len(),
        );

        let max_cols = get_terminal_cols().unwrap_or(80);
        let mut lines = 1;
        let max_lines = 8;
        let now = Instant::now();
        for task in self.tasks.iter().take(max_lines) {
            if lines == max_lines && self.tasks.len() > max_lines {
                println!("...and {} more", self.tasks.len() - max_lines + 1);
            } else {
                let delta = now.duration_since(task.start).as_secs();
                let line = format!("{}s {}", delta, task.message);
                if line.len() >= max_cols {
                    println!("{}...", &line[0..max_cols - 4]);
                } else {
                    println!("{}", line);
                }
            }
            lines += 1;
        }

        // Move cursor up to the first printed line, for overprinting.
        print!("\x1b[{}A", lines);
    }

    fn maybe_print(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_update);
        if delta < Duration::from_millis(50) {
            return;
        }
        self.print();
        self.last_update = now;
    }
}
