//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use crate::{
    graph::Build, graph::BuildId, task::TaskResult, task::Termination, terminal, work::BuildState,
    work::StateCounts,
};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

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

    /// Called when a task starts or completes.
    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>);

    /// Log some (debug) information, without corrupting the progress display.
    fn log(&mut self, msg: &str);

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

/// Progress implementation for "dumb" console, without any overprinting.
#[derive(Default)]
pub struct DumbConsoleProgress {
    /// Whether to print command lines of started programs.
    verbose: bool,

    /// The id of the last command printed, used to avoid printing it twice
    /// when we have two updates from the same command in a row.
    last_started: Option<BuildId>,
}

impl DumbConsoleProgress {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            last_started: None,
        }
    }
}

impl Progress for DumbConsoleProgress {
    fn update(&mut self, _counts: &StateCounts) {
        // ignore
    }

    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>) {
        match result {
            None => {
                // Starting.
                self.log(if self.verbose {
                    build.cmdline.as_ref().unwrap()
                } else {
                    build_message(build)
                });
                self.last_started = Some(id);
            }
            Some(result) => {
                // Finished.
                match result.termination {
                    Termination::Success => {
                        if result.output.is_empty() {
                            // Common success case, no need to print.
                        } else if self.last_started == Some(id) {
                            // We just printed the command, don't print it again.
                        } else {
                            self.log(build_message(build))
                        }
                    }
                    Termination::Interrupted => {
                        self.log(&format!("interrupted: {}", build_message(build)))
                    }
                    Termination::Failure => self.log(&format!("failed: {}", build_message(build))),
                };
                if !result.output.is_empty() {
                    std::io::stdout().write_all(&result.output).unwrap();
                }
            }
        }
    }

    fn log(&mut self, msg: &str) {
        println!("{}", msg);
    }

    fn finish(&mut self) {}
}

/// Progress implementation for "fancy" console, with progress bar etc.
/// Each time it prints, it clears from the cursor to the end of the console,
/// prints the status text, and then moves moves the cursor back up to the
/// start position.  This means on errors etc. we can clear any status by
/// clearing the console too.
pub struct FancyConsoleProgress {
    state: Arc<Mutex<FancyState>>,
}

/// Screen updates happen after this duration passes, to reduce the amount
/// of printing in the case of rapid updates.  This helps with terminal flicker.
const UPDATE_DELAY: Duration = std::time::Duration::from_millis(100);

impl FancyConsoleProgress {
    pub fn new(verbose: bool) -> Self {
        let dirty_cond = Arc::new(Condvar::new());
        let state = Arc::new(Mutex::new(FancyState {
            done: false,
            dirty: false,
            dirty_cond: dirty_cond.clone(),
            counts: StateCounts::default(),
            tasks: VecDeque::new(),
            verbose,
        }));

        // Thread to debounce status updates -- waits a bit, then prints after
        // any dirty state.
        std::thread::spawn({
            let state = state.clone();
            move || loop {
                // Sleep before doing anything, so that we delay slightly
                // before our first print.  This reduces flicker in the case where
                // the work immediately completes.
                std::thread::sleep(UPDATE_DELAY);

                // Wait to be notified of a display update, or timeout at 500ms.
                // The timeout is for the case where there are lengthy build
                // steps and the progress will show how long they've been
                // running.
                let (mut state, _) = dirty_cond
                    .wait_timeout_while(
                        state.lock().unwrap(),
                        Duration::from_millis(500),
                        |state| !state.dirty,
                    )
                    .unwrap();
                if state.done {
                    break;
                }

                // Update regardless of whether we timed out or not.
                state.print_progress();
            }
        });

        FancyConsoleProgress { state }
    }
}

impl Progress for FancyConsoleProgress {
    fn update(&mut self, counts: &StateCounts) {
        self.state.lock().unwrap().update(counts);
    }

    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>) {
        self.state.lock().unwrap().task_state(id, build, result);
    }

    fn log(&mut self, msg: &str) {
        self.state.lock().unwrap().log(msg);
    }

    fn finish(&mut self) {
        self.state.lock().unwrap().finish();
    }
}

struct FancyState {
    done: bool,
    dirty: bool,
    dirty_cond: Arc<Condvar>,

    /// Counts of tasks in each state.  TODO: pass this as function args?
    counts: StateCounts,
    /// Build tasks that are currently executing.
    /// Pushed to as tasks are started, so it's always in order of age.
    tasks: VecDeque<Task>,
    /// Whether to print command lines of started programs.
    verbose: bool,
}

impl FancyState {
    fn dirty(&mut self) {
        self.dirty = true;
        self.dirty_cond.notify_one();
    }

    fn update(&mut self, counts: &StateCounts) {
        self.counts = counts.clone();
        self.dirty();
    }

    fn task_state(&mut self, id: BuildId, build: &Build, result: Option<&TaskResult>) {
        match result {
            None => {
                // Task starting.
                if self.verbose {
                    self.log(build.cmdline.as_ref().unwrap());
                }
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
                match result.termination {
                    Termination::Success => {
                        if result.output.is_empty() {
                            // Common case: don't show anything.
                        } else {
                            self.log(build_message(build))
                        }
                    }
                    Termination::Interrupted => {
                        self.log(&format!("interrupted: {}", build_message(build)))
                    }
                    Termination::Failure => self.log(&format!("failed: {}", build_message(build))),
                };
                if !result.output.is_empty() {
                    std::io::stdout().write_all(&result.output).unwrap();
                }
            }
        }
        self.dirty();
    }

    fn log(&mut self, msg: &str) {
        self.clear_progress();
        println!("{}", msg);
        self.dirty();
    }

    fn finish(&mut self) {
        self.clear_progress();
        self.done = true;
        self.dirty(); // let thread quit
    }

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
        // If the user hit ctl-c, it may have printed something on the line.
        // So \r to go to first column first, then clear anything below.
        std::io::stdout().write_all(b"\r\x1b[J").unwrap();
    }

    fn print_progress(&mut self) {
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
            self.counts.get(BuildState::Queued)
                + self.counts.get(BuildState::Running)
                + self.counts.get(BuildState::Ready),
        ));
        println!("{}", progress_line);

        let max_cols = terminal::get_cols().unwrap_or(80);
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
        self.dirty = false;
    }
}
