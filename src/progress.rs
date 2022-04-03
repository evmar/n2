//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use std::collections::VecDeque;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use crate::graph::Build;
use crate::graph::BuildId;
use crate::task::TaskResult;
use crate::task::Termination;
use crate::work::BuildState;
use crate::work::StateCounts;

#[cfg(unix)]
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

#[cfg(windows)]
#[allow(clippy::uninit_assumed_init)]
pub fn get_terminal_cols() -> Option<usize> {
    extern crate winapi;
    extern crate kernel32;
    use kernel32::{GetConsoleScreenBufferInfo, GetStdHandle};
    let console = unsafe { GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE) };
    if console == winapi::um::handleapi::INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe {
        let mut csbi = ::std::mem::MaybeUninit::uninit().assume_init();
        if GetConsoleScreenBufferInfo(console, &mut csbi) == 0 {
            return None;
        }
        Some(csbi.dwSize.X as usize)
    }
}

/// Compute the message to display on the console for a given build.
pub fn build_message(build: &Build) -> &str {
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

    /// Called when a task starts or completes.
    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>);

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
    /// Whether to print command lines of completed programs.
    verbose: bool,
    /// Whether to print a progress bar and currently running tasks.
    fancy_terminal: bool,
}

#[allow(clippy::new_without_default)]
impl ConsoleProgress {
    pub fn new(verbose: bool, fancy_terminal: bool) -> Self {
        ConsoleProgress {
            // Act like our last update was now, so that we delay slightly
            // before our first print.  This reduces flicker in the case where
            // the work immediately completes.
            last_update: Instant::now(),
            counts: StateCounts::new(),
            tasks: VecDeque::new(),
            verbose,
            fancy_terminal,
        }
    }
}

impl Progress for ConsoleProgress {
    fn update(&mut self, counts: &StateCounts) {
        self.counts = counts.clone();
        self.maybe_print_progress();
    }

    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>) {
        match result {
            None => {
                // Task starting.
                let message = build_message(build);
                self.tasks.push_back(Task {
                    id,
                    start: Instant::now(),
                    message: message.to_string(),
                });
            }
            Some(result) => {
                // Task completed.
                self.tasks
                    .remove(self.tasks.iter().position(|t| t.id == id).unwrap());
                self.print_result(build, result);
            }
        }
        self.maybe_print_progress();
    }

    fn flush(&mut self) {
        self.print_progress();
    }

    fn finish(&mut self) {
        self.clear_progress();
    }
}

impl ConsoleProgress {
    fn progress_bar(&self) -> String {
        let bar_size = 40;
        let mut bar = String::with_capacity(bar_size);
        let mut sum: usize = 0;
        let total = self.counts.total();
        for (count, ch) in [
            (
                self.counts.get(BuildState::Done) + self.counts.get(BuildState::Failed),
                '=',
            ),
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

    fn clear_progress(&self) {
        if !self.fancy_terminal {
            return;
        }
        // If the user hit ctl-c, it may have printed something on the line.
        // So \r to go to first column first, then clear anything below.
        std::io::stdout().write_all(b"\r\x1b[J").unwrap();
    }

    fn print_progress(&self) {
        if !self.fancy_terminal {
            return;
        }
        self.clear_progress();
        let failed = self.counts.get(BuildState::Failed);
        let mut progress_line = format!(
            "[{}] {}/{} done, ",
            self.progress_bar(),
            self.counts.get(BuildState::Done) + failed,
            self.counts.total()
        );
        if failed > 0 {
            progress_line.push_str(&format!("{} failed, ", failed));
        }
        progress_line.push_str(&format!(
            "{}/{} running",
            self.tasks.len(),
            self.counts.get(BuildState::Queued) + self.tasks.len(),
        ));
        println!("{}", progress_line);

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

    fn maybe_print_progress(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_update);
        if delta < Duration::from_millis(50) {
            return;
        }
        self.print_progress();
        self.last_update = now;
    }

    fn print_result(&mut self, build: &Build, result: &TaskResult) {
        // By default we don't want to print anything when a task completes,
        // but we do want to print the completed task when:
        // - failed tasks
        // - when we opted in to verbose output
        // - when we aren't doing fancy terminal progress display
        // - when the task had output (even in non-failing cases)

        if result.termination == Termination::Success
            && !self.verbose
            && self.fancy_terminal
            && result.output.is_empty()
        {
            return;
        }

        self.clear_progress();
        let status = match result.termination {
            Termination::Success => "",
            Termination::Interrupted => "interrupted: ",
            Termination::Failure => "failed: ",
        };
        let message = if self.verbose {
            build.cmdline.as_ref().unwrap()
        } else {
            build_message(build)
        };
        println!("{}{}", status, message);

        if !result.output.is_empty() {
            std::io::stdout().write_all(&result.output).unwrap();
        }
    }
}
