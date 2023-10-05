//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use crate::{
    graph::Build, graph::BuildId, process::Termination, task::TaskResult, terminal,
    work::BuildState, work::StateCounts,
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
        .filter(|desc| !desc.is_empty())
        .unwrap_or_else(|| build.cmdline.as_ref().unwrap())
}

/// Trait for build progress notifications.
pub trait Progress {
    /// Called as individual build tasks progress through build states.
    fn update(&mut self, counts: &StateCounts);

    /// Called when a task starts.
    fn task_started(&mut self, id: BuildId, build: &Build);

    /// Called when a task's last line of output changes.
    fn task_output(&mut self, id: BuildId, line: Vec<u8>);

    /// Called when a task completes.
    fn task_finished(&mut self, id: BuildId, build: &Build, result: &TaskResult);

    /// Log a line of output without corrupting the progress display.
    /// This line is persisted beyond further progress updates.  For example,
    /// used when a task fails; we want the final output to show that failed
    /// task's output even if we do more work after it fails.
    fn log(&mut self, msg: &str);
}

/// Currently running build task, as tracked for progress updates.
struct Task {
    id: BuildId,
    /// When the task started running.
    start: Instant,
    /// Build status message for the task.
    message: String,
    /// Last line of output from the task.
    last_line: Option<String>,
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

    fn task_started(&mut self, id: BuildId, build: &Build) {
        self.log(if self.verbose {
            build.cmdline.as_ref().unwrap()
        } else {
            build_message(build)
        });
        self.last_started = Some(id);
    }

    fn task_output(&mut self, _id: BuildId, _line: Vec<u8>) {
        // ignore
    }

    fn task_finished(&mut self, id: BuildId, build: &Build, result: &TaskResult) {
        match result.termination {
            Termination::Success => {
                if result.output.is_empty() || self.last_started == Some(id) {
                    // Output is empty, or we just printed the command, don't print it again.
                } else {
                    self.log(build_message(build))
                }
            }
            Termination::Interrupted => self.log(&format!("interrupted: {}", build_message(build))),
            Termination::Failure => self.log(&format!("failed: {}", build_message(build))),
        };
        if !result.output.is_empty() {
            std::io::stdout().write_all(&result.output).unwrap();
        }
    }

    fn log(&mut self, msg: &str) {
        println!("{}", msg);
    }
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
const UPDATE_DELAY: Duration = std::time::Duration::from_millis(50);

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
                // Wait to be notified of a display update, or timeout at 500ms.
                // The timeout is for the case where there are lengthy build
                // steps and the progress will show how long they've been
                // running.
                {
                    let (state, _) = dirty_cond
                        .wait_timeout_while(
                            state.lock().unwrap(),
                            Duration::from_millis(500),
                            |state| !state.dirty,
                        )
                        .unwrap();
                    if state.done {
                        break;
                    }
                }

                // Delay a little bit in case more display updates come in.
                std::thread::sleep(UPDATE_DELAY);

                // Update regardless of whether we timed out or not.
                state.lock().unwrap().print_progress();
            }
        });

        FancyConsoleProgress { state }
    }
}

impl Progress for FancyConsoleProgress {
    fn update(&mut self, counts: &StateCounts) {
        self.state.lock().unwrap().update(counts);
    }

    fn task_started(&mut self, id: BuildId, build: &Build) {
        self.state.lock().unwrap().task_started(id, build);
    }

    fn task_output(&mut self, id: BuildId, line: Vec<u8>) {
        self.state.lock().unwrap().task_output(id, line);
    }

    fn task_finished(&mut self, id: BuildId, build: &Build, result: &TaskResult) {
        self.state.lock().unwrap().task_finished(id, build, result);
    }

    fn log(&mut self, msg: &str) {
        self.state.lock().unwrap().log(msg);
    }
}

impl Drop for FancyConsoleProgress {
    fn drop(&mut self) {
        self.state.lock().unwrap().cleanup();
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

    fn task_started(&mut self, id: BuildId, build: &Build) {
        if self.verbose {
            self.log(build.cmdline.as_ref().unwrap());
        }
        let message = build_message(build);
        self.tasks.push_back(Task {
            id,
            start: Instant::now(),
            message: message.to_string(),
            last_line: None,
        });
        self.dirty();
    }

    fn task_output(&mut self, id: BuildId, line: Vec<u8>) {
        let task = self.tasks.iter_mut().find(|t| t.id == id).unwrap();
        task.last_line = Some(String::from_utf8_lossy(&line).into_owned());
        self.dirty();
    }

    fn task_finished(&mut self, id: BuildId, build: &Build, result: &TaskResult) {
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
            Termination::Interrupted => self.log(&format!("interrupted: {}", build_message(build))),
            Termination::Failure => self.log(&format!("failed: {}", build_message(build))),
        };
        if !result.output.is_empty() {
            std::io::stdout().write_all(&result.output).unwrap();
        }
        self.dirty();
    }

    fn log(&mut self, msg: &str) {
        self.clear_progress();
        println!("{}", msg);
        self.dirty();
    }

    fn cleanup(&mut self) {
        self.clear_progress();
        self.done = true;
        self.dirty(); // let thread quit
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
            progress_bar(&self.counts, 40),
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
        let mut lines = 1;

        let max_cols = terminal::get_cols().unwrap_or(80);
        let max_tasks = 8;
        let now = Instant::now();
        for task in self.tasks.iter().take(max_tasks) {
            let delta = now.duration_since(task.start).as_secs() as usize;
            println!("{}", task_message(&task.message, delta, max_cols));
            lines += 1;
            if let Some(line) = &task.last_line {
                let max_len = max_cols - 2;
                let substring = if line.len() >= max_len {
                    &line[..max_len]
                } else {
                    line
                };
                println!("  {}", substring);
                lines += 1;
            }
        }

        if self.tasks.len() > max_tasks {
            let remaining = self.tasks.len() - max_tasks;
            println!("...and {} more", remaining);
            lines += 1;
        }

        // Move cursor up to the first printed line, for overprinting.
        print!("\x1b[{}A", lines);
        self.dirty = false;
    }
}

/// Format a task's status message to optionally include how long it has been running
/// and also to fit within a maximum number of terminal columns.
fn task_message(message: &str, seconds: usize, max_cols: usize) -> String {
    let time_note = if seconds > 2 {
        format!(" ({}s)", seconds)
    } else {
        "".into()
    };
    let mut out = message.to_owned();
    if out.len() + time_note.len() >= max_cols {
        out.truncate(max_cols - time_note.len() - 3);
        out.push_str("...");
    }
    out.push_str(&time_note);
    out
}

/// Render a StateCounts as an ASCII progress bar.
fn progress_bar(counts: &StateCounts, bar_size: usize) -> String {
    let mut bar = String::with_capacity(bar_size);
    let mut sum: usize = 0;
    let total = counts.total();
    if total == 0 {
        return " ".repeat(bar_size);
    }
    for (count, ch) in [
        (
            counts.get(BuildState::Done) + counts.get(BuildState::Failed),
            '=',
        ),
        (
            counts.get(BuildState::Queued)
                + counts.get(BuildState::Running)
                + counts.get(BuildState::Ready),
            '-',
        ),
        (counts.get(BuildState::Want), ' '),
    ] {
        sum += count;
        let mut target_size = sum * bar_size / total;
        if count > 0 && target_size == bar.len() && target_size < bar_size {
            // Special case: for non-zero count, ensure we always get at least
            // one tick.
            target_size += 1;
        }
        while bar.len() < target_size {
            bar.push(ch);
        }
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_rendering() {
        let mut counts = StateCounts::default();

        // Don't crash if we show progress before having any tasks.
        assert_eq!(progress_bar(&counts, 10), "          ");

        counts.add(BuildState::Want, 100);
        assert_eq!(progress_bar(&counts, 10), "          ");

        // Half want -> ready.
        counts.add(BuildState::Want, -50);
        counts.add(BuildState::Ready, 50);
        assert_eq!(progress_bar(&counts, 10), "-----     ");

        // One ready -> done.
        counts.add(BuildState::Ready, -1);
        counts.add(BuildState::Done, 1);
        assert_eq!(progress_bar(&counts, 10), "=----     ");

        // All but one want -> ready.
        counts.add(BuildState::Want, -49);
        counts.add(BuildState::Ready, 49);
        assert_eq!(progress_bar(&counts, 10), "=-------- ");

        // All want -> ready.
        counts.add(BuildState::Want, -1);
        counts.add(BuildState::Ready, 1);
        assert_eq!(progress_bar(&counts, 10), "=---------");
    }

    #[test]
    fn task_rendering() {
        assert_eq!(task_message("building foo.o", 0, 80), "building foo.o");
        assert_eq!(task_message("building foo.o", 0, 10), "buildin...");
        assert_eq!(task_message("building foo.o", 0, 5), "bu...");
    }

    #[test]
    fn task_rendering_with_time() {
        assert_eq!(task_message("building foo.o", 5, 80), "building foo.o (5s)");
        assert_eq!(task_message("building foo.o", 5, 10), "bu... (5s)");
    }
}
