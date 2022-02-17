//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use std::collections::VecDeque;
use std::ops::Sub;
use std::time::Duration;
use std::time::Instant;

use crate::graph::Build;
use crate::graph::BuildId;
use crate::work::BuildState;

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
    /// Cached builds may jump from BuildState::Ready directly to BuildState::Done.
    fn build_state(&mut self, id: BuildId, build: &Build, prev: BuildState, state: BuildState);

    /// Called periodically on a timer, and on build finish.
    /// state represents the overall completion state of the build.
    fn tick(&mut self, state: BuildState);

    /// Called when a build has failed.
    /// TODO: maybe this should just be part of build_state?
    /// In particular, consider the case where builds output progress as they run,
    /// as well as the case where multiple build steps are allowed to fail.
    fn failed(&mut self, build: &Build, output: &[u8]);
}

/// Currently running build task, as tracked for progress updates.
struct Task {
    /// When the task started running.
    start: Instant,
    id: BuildId,
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
    /// Total count of build tasks.
    total: usize,
    /// Count of build tasks that are ready to be checked for status.
    ready: usize,
    /// Count of build tasks that would execute if we had CPUs for them.
    queued: usize,
    /// Build tasks that are currently executing.
    /// Pushed to as tasks are started, so it's always in order of age.
    tasks: VecDeque<Task>,
    /// Count of build tasks that have finished.
    done: usize,
}

#[allow(clippy::new_without_default)]
impl ConsoleProgress {
    pub fn new() -> Self {
        ConsoleProgress {
            last_update: Instant::now().sub(Duration::from_secs(1)),
            total: 0,
            ready: 0,
            queued: 0,
            tasks: VecDeque::new(),
            done: 0,
        }
    }
}

impl Progress for ConsoleProgress {
    fn build_state(&mut self, id: BuildId, build: &Build, prev: BuildState, state: BuildState) {
        match prev {
            BuildState::Ready => self.ready -= 1,
            BuildState::Queued => self.queued -= 1,
            BuildState::Running => {
                self.tasks
                    .remove(self.tasks.iter().position(|t| t.id == id).unwrap());
            }
            _ => {}
        }
        match state {
            BuildState::Want => self.total += 1,
            BuildState::Ready => self.ready += 1,
            BuildState::Queued => self.queued += 1,
            BuildState::Running => {
                let message = build_message(build);
                self.tasks.push_back(Task {
                    start: Instant::now(),
                    id,
                    message: message.to_string(),
                });
            }
            BuildState::Done => self.done += 1,
            _ => {}
        }
        self.maybe_print();
    }

    fn tick(&mut self, state: BuildState) {
        match state {
            BuildState::Done => {
                // Unconditionally update the console a final time.
                self.print();
            }
            _ => self.maybe_print(),
        }
    }

    fn failed(&mut self, build: &Build, output: &[u8]) {
        let message = build_message(build);
        // If the user hit ctl-c, it may have printed something on the line.
        // So \r to go to first column first, then the same clear we use elsewhere.
        println!("\r\x1b[Jfailed: {}", message);
        println!("{}", String::from_utf8_lossy(output));
    }
}

impl ConsoleProgress {
    fn progress_bar(&self) -> String {
        let mut bar = String::new();
        let mut sum: usize = 0;
        for &(count, ch) in &[
            (self.done, '='),
            (self.ready + self.tasks.len() + self.queued, '-'),
            (self.total, ' '),
        ] {
            sum += count;
            if sum >= self.total {
                sum = self.total;
            }
            while bar.len() <= (sum * 40 / self.total) {
                bar.push(ch);
            }
        }
        bar
    }

    #[allow(dead_code)]
    fn dump(&self) {
        println!(
            "[{} {} {} {}]",
            self.done,
            self.ready,
            self.tasks.len(),
            self.total
        );
    }

    fn print(&self) {
        println!(
            "\x1b[J[{}] {}/{} done, {}/{} running",
            self.progress_bar(),
            self.done,
            self.total,
            self.tasks.len(),
            self.queued + self.tasks.len(),
        );

        let max_cols = get_terminal_cols().unwrap_or(80);
        let mut lines = 1;
        let now = Instant::now();
        for task in self.tasks.iter().take(8) {
            let delta = now.duration_since(task.start).as_secs();
            let line = format!("{}s {}", delta, task.message);
            if line.len() >= max_cols {
                println!("{}...", &line[0..max_cols - 4]);
            } else {
                println!("{}", line);
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
