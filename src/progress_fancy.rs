//! Build progress reporting for a "fancy" console, with progress bar etc.

use crate::progress::{build_message, Progress};
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

/// Progress implementation for "fancy" console, with progress bar etc.
/// Each time it prints, it clears from the cursor to the end of the console,
/// prints the status text, and then moves moves the cursor back up to the
/// start position.  This means on errors etc. we can clear any status by
/// clearing the console too.
pub struct FancyConsoleProgress {
    state: Arc<Mutex<FancyState>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Screen updates happen after this duration passes, to reduce the amount
/// of printing in the case of rapid updates.  This helps with terminal flicker.
const UPDATE_DELAY: Duration = std::time::Duration::from_millis(50);

/// If there are no updates for this duration, the progress will print anyway.
/// This lets the progress show ticking timers for long-running tasks so things
/// do not appear hung.
const TIMEOUT_DELAY: Duration = std::time::Duration::from_millis(500);

impl FancyConsoleProgress {
    pub fn new(verbose: bool) -> Self {
        let dirty_cond = Arc::new(Condvar::new());
        let state = Arc::new(Mutex::new(FancyState {
            done: false,
            pending: Vec::new(),
            dirty: false,
            dirty_cond: dirty_cond.clone(),
            counts: StateCounts::default(),
            tasks: VecDeque::new(),
            verbose,
        }));

        // Thread to debounce status updates -- waits a bit, then prints after
        // any dirty state.
        let thread = std::thread::spawn({
            let state_lock = state.clone();
            move || loop {
                // Wait to be notified of a display update or timeout.
                {
                    let (state, _) = dirty_cond
                        .wait_timeout_while(
                            state_lock.lock().unwrap(),
                            TIMEOUT_DELAY - UPDATE_DELAY,
                            |state| !state.done && !state.dirty,
                        )
                        .unwrap();
                    if state.done {
                        std::io::stdout().write_all(&state.pending).unwrap();
                        break;
                    }
                }

                // Delay a little bit in case more display updates come in.
                // We know .dirty will only ever be cleared below, so we
                // can drop the lock here while we sleep.
                std::thread::sleep(UPDATE_DELAY);

                state_lock.lock().unwrap().print_progress();
            }
        });

        FancyConsoleProgress {
            state,
            thread: Some(thread),
        }
    }
}

impl Progress for FancyConsoleProgress {
    fn update(&self, counts: &StateCounts) {
        self.state.lock().unwrap().update(counts);
    }

    fn task_started(&self, id: BuildId, build: &Build) {
        self.state.lock().unwrap().task_started(id, build);
    }

    fn task_output(&self, id: BuildId, line: Vec<u8>) {
        self.state.lock().unwrap().task_output(id, line);
    }

    fn task_finished(&self, id: BuildId, build: &Build, result: &TaskResult) {
        self.state.lock().unwrap().task_finished(id, build, result);
    }

    fn log(&self, msg: &str) {
        self.state.lock().unwrap().log(msg);
    }
}

impl Drop for FancyConsoleProgress {
    fn drop(&mut self) {
        self.state.lock().unwrap().cleanup();
        self.thread.take().unwrap().join().unwrap();
    }
}

struct FancyState {
    done: bool,

    /// Text to print on the next update.
    /// Typically starts with the "clear any existing progress bar" sequence.
    pending: Vec<u8>,

    /// True when there is new progress to display.
    /// When set, will notify dirty_cond.
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
            write!(&mut self.pending, "{}\n", build.cmdline.as_ref().unwrap()).ok();
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

        // Show task name, status, and output.
        let buf = &mut self.pending;
        match result.termination {
            Termination::Success if result.output.is_empty() => {
                // Common case: don't show anything.
                return;
            }
            Termination::Success => write!(buf, "{}\n", build_message(build)).ok(),
            Termination::Interrupted => write!(buf, "interrupted: {}\n", build_message(build)).ok(),
            Termination::Failure => write!(buf, "failed: {}\n", build_message(build)).ok(),
        };
        buf.extend_from_slice(&result.output);
        if !result.output.ends_with(b"\n") {
            buf.push(b'\n');
        }

        self.dirty();
    }

    fn log(&mut self, msg: &str) {
        self.pending.extend_from_slice(msg.as_bytes());
        self.pending.push(b'\n');
        self.dirty();
    }

    fn cleanup(&mut self) {
        self.done = true;
        self.dirty(); // let thread print final time
    }

    fn print_progress(&mut self) {
        let failed = self.counts.get(BuildState::Failed);
        let mut buf: &mut Vec<u8> = &mut self.pending;
        write!(
            &mut buf,
            "[{}] {}/{} done, ",
            progress_bar(&self.counts, 40),
            self.counts.get(BuildState::Done) + failed,
            self.counts.total()
        )
        .ok();
        if failed > 0 {
            write!(&mut buf, "{} failed, ", failed).ok();
        }
        write!(
            &mut buf,
            "{}/{} running\n",
            self.tasks.len(),
            self.counts.get(BuildState::Queued)
                + self.counts.get(BuildState::Running)
                + self.counts.get(BuildState::Ready),
        )
        .ok();
        let mut lines = 1;

        let max_cols = terminal::get_cols().unwrap_or(80);
        let max_tasks = 8;
        let now = Instant::now();
        for task in self.tasks.iter().take(max_tasks) {
            let delta = now.duration_since(task.start).as_secs() as usize;
            write!(
                &mut buf,
                "{}\n",
                task_message(&task.message, delta, max_cols)
            )
            .ok();
            lines += 1;
            if let Some(line) = &task.last_line {
                let max_len = max_cols - 2;
                write!(&mut buf, "  {}\n", truncate(line, max_len)).ok();
                lines += 1;
            }
        }

        if self.tasks.len() > max_tasks {
            let remaining = self.tasks.len() - max_tasks;
            write!(&mut buf, "...and {} more\n", remaining).ok();
            lines += 1;
        }

        // Move cursor up to the first printed line, for overprinting.
        write!(&mut buf, "\x1b[{}A", lines).ok();
        std::io::stdout().write_all(&buf).unwrap();

        // Set up buf for next print.
        // If the user hit ctl-c, it may have printed something on the line.
        // So \r to go to first column first, then clear anything below.
        buf.clear();
        buf.extend_from_slice(b"\r\x1b[J");

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

fn truncate(s: &str, mut max: usize) -> &str {
    if max >= s.len() {
        return s;
    }
    while !s.is_char_boundary(max) {
        max -= 1;
    }
    &s[..max]
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

    #[test]
    fn truncate_utf8() {
        let text = "utf8 progress bar: ━━━━━━━━━━━━";
        for len in 10..text.len() {
            // test passes if this doesn't panic
            truncate(text, len);
        }
    }
}
