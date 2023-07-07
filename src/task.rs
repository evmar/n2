//! Runs build tasks, potentially in parallel.
//! Unaware of the build graph, pools, etc.; just command execution.
//!
//! TODO: consider rewriting to use poll() etc. instead of threads.
//! The threads might be relatively cheap(?) because they just block on
//! the subprocesses though?

use crate::{
    depfile,
    graph::{BuildId, RspFile},
    process,
    scanner::Scanner,
};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

pub struct FinishedTask {
    /// A (faked) "thread id", used to put different finished builds in different
    /// tracks in a performance trace.
    pub tid: usize,
    pub buildid: BuildId,
    pub span: (Instant, Instant),
    pub result: TaskResult,
}

/// The result of running a build step.
pub struct TaskResult {
    pub termination: process::Termination,
    /// Console output.
    pub output: Vec<u8>,
    pub discovered_deps: Option<Vec<String>>,
}

/// Reads dependencies from a .d file path.
fn read_depfile(path: &Path) -> anyhow::Result<Vec<String>> {
    let mut bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => bail!("read {}: {}", path.display(), e),
    };
    let mut scanner = Scanner::new(&mut bytes);
    let parsed_deps = depfile::parse(&mut scanner)
        .map_err(|err| anyhow!(scanner.format_parse_error(path, err)))?;
    // TODO verify deps refers to correct output
    let deps: Vec<String> = parsed_deps
        .deps
        .iter()
        .map(|&dep| dep.to_string())
        .collect();
    Ok(deps)
}

fn write_rspfile(rspfile: &RspFile) -> anyhow::Result<()> {
    if let Some(parent) = rspfile.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&rspfile.path, &rspfile.content)?;
    Ok(())
}

/// Executes a build task as a subprocess.
/// Returns an Err() if we failed outside of the process itself.
fn run_task(
    cmdline: &str,
    depfile: Option<&Path>,
    rspfile: Option<&RspFile>,
) -> anyhow::Result<TaskResult> {
    if let Some(rspfile) = rspfile {
        write_rspfile(rspfile)?;
    }
    let (termination, output) = process::run_command(cmdline)?;
    let mut discovered_deps = None;
    if termination == process::Termination::Success {
        if let Some(depfile) = depfile {
            discovered_deps = Some(read_depfile(depfile)?);
        }
    }
    Ok(TaskResult {
        termination,
        output,
        discovered_deps,
    })
}

#[cfg(target_arch = "wasm32")]
fn run_command(cmdline: &str) -> anyhow::Result<TaskResult> {
    bail!("wasm cannot run commands");
}

/// Tracks faked "thread ids" -- integers assigned to build tasks to track
/// parallelism in perf trace output.
#[derive(Default)]
struct ThreadIds {
    /// An entry is true when claimed, false or nonexistent otherwise.
    slots: Vec<bool>,
}
impl ThreadIds {
    fn claim(&mut self) -> usize {
        match self.slots.iter().position(|&used| !used) {
            Some(idx) => {
                self.slots[idx] = true;
                idx
            }
            None => {
                let idx = self.slots.len();
                self.slots.push(true);
                idx
            }
        }
    }

    fn release(&mut self, slot: usize) {
        self.slots[slot] = false;
    }
}

pub struct Runner {
    finished_send: mpsc::Sender<FinishedTask>,
    finished_recv: mpsc::Receiver<FinishedTask>,
    pub running: usize,
    tids: ThreadIds,
    parallelism: usize,
}

impl Runner {
    pub fn new(parallelism: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        Runner {
            finished_send: tx,
            finished_recv: rx,
            running: 0,
            tids: ThreadIds::default(),
            parallelism,
        }
    }

    pub fn can_start_more(&self) -> bool {
        self.running < self.parallelism
    }

    pub fn is_running(&self) -> bool {
        self.running > 0
    }

    pub fn start(
        &mut self,
        id: BuildId,
        cmdline: String,
        depfile: Option<PathBuf>,
        rspfile: Option<RspFile>,
    ) {
        let tid = self.tids.claim();
        let tx = self.finished_send.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result =
                run_task(&cmdline, depfile.as_deref(), rspfile.as_ref()).unwrap_or_else(|err| {
                    TaskResult {
                        termination: process::Termination::Failure,
                        output: err.to_string().into_bytes(),
                        discovered_deps: None,
                    }
                });
            let finish = Instant::now();

            let task = FinishedTask {
                tid,
                buildid: id,
                span: (start, finish),
                result,
            };
            // The send will only fail if the receiver disappeared, e.g. due to shutting down.
            let _ = tx.send(task);
        });
        self.running += 1;
    }

    /// Wait for a build to complete.  May block for a long time.
    pub fn wait(&mut self) -> FinishedTask {
        let task = self.finished_recv.recv().unwrap();
        self.tids.release(task.tid);
        self.running -= 1;
        task
    }
}
