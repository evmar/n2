//! Runs build tasks, potentially in parallel.
//! Unaware of the build graph, pools, etc.; just command execution.
//!
//! TODO: consider rewriting to use poll() etc. instead of threads.
//! The threads might be relatively cheap(?) because they just block on
//! the subprocesses though?

use crate::depfile;
use crate::graph::BuildId;
use crate::scanner::Scanner;
use anyhow::{anyhow, bail};
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct FinishedBuild {
    /// A (faked) "thread id", used to put different finished builds in different
    /// tracks in a performance trace.
    pub tid: usize,
    pub id: BuildId,
    pub span: (Instant, Instant),
    pub result: BuildResult,
}

/// The result of executing a build step.
pub struct BuildResult {
    pub success: bool,
    /// Console output.
    pub output: Vec<u8>,
    pub discovered_deps: Option<Vec<String>>,
}

/// Reads dependencies from a .d file path.
fn read_depfile(path: &str) -> anyhow::Result<Vec<String>> {
    let mut bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => bail!("read {}: {}", path, e),
    };
    bytes.push(0);

    let mut scanner = Scanner::new(unsafe { std::str::from_utf8_unchecked(&bytes) });
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

/// Executes a build step as a subprocess.
/// Returns an Err() if we failed outside of the process itself.
fn run_build(cmdline: &str, depfile: Option<&str>) -> anyhow::Result<BuildResult> {
    let mut cmd = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmdline)
        .output()?;
    let mut output = Vec::new();
    output.append(&mut cmd.stdout);
    output.append(&mut cmd.stderr);
    let success = cmd.status.success();

    let mut discovered_deps: Option<Vec<String>> = None;
    if success {
        discovered_deps = match depfile {
            None => None,
            Some(deps) => Some(read_depfile(deps)?),
        };
    } else {
        // Command failed.
        if let Some(sig) = cmd.status.signal() {
            match sig {
                libc::SIGINT => write!(output, "interrupted").unwrap(),
                _ => write!(output, "signal {}", sig).unwrap(),
            }
        }
    }

    Ok(BuildResult {
        success,
        output,
        discovered_deps,
    })
}

/// Tracks faked "thread ids" -- integers assigned to build tasks to track
/// paralllelism in perf trace output.
struct ThreadIds {
    /// An entry is true when claimed, false or nonexistent otherwise.
    slots: Vec<bool>,
}
impl ThreadIds {
    fn new() -> Self {
        ThreadIds { slots: Vec::new() }
    }

    fn claim(&mut self) -> usize {
        match self.slots.iter().position(|&used| !used) {
            Some(idx) => {
                self.slots[idx] = true;
                idx
            }
            None => {
                let idx = self.slots.len();
                self.slots.push(false);
                idx
            }
        }
    }

    fn release(&mut self, slot: usize) {
        self.slots[slot] = false;
    }
}

pub struct Runner {
    finished_send: mpsc::Sender<FinishedBuild>,
    finished_recv: mpsc::Receiver<FinishedBuild>,
    pub running: usize,
    tids: ThreadIds,
}

impl Runner {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Runner {
            finished_send: tx,
            finished_recv: rx,
            running: 0,
            tids: ThreadIds::new(),
        }
    }

    pub fn can_start_more(&self) -> bool {
        self.running < 8
    }

    pub fn is_running(&self) -> bool {
        self.running > 0
    }

    pub fn start(&mut self, id: BuildId, cmdline: String, depfile: Option<String>) {
        let tid = self.tids.claim();
        let tx = self.finished_send.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result =
                run_build(&cmdline, depfile.as_deref()).unwrap_or_else(|err| BuildResult {
                    success: false,
                    output: err.to_string().into_bytes(),
                    discovered_deps: None,
                });
            let finish = Instant::now();

            let fin = FinishedBuild {
                tid,
                id,
                span: (start, finish),
                result,
            };
            // The send will only fail if the receiver disappeared, e.g. due to shutting down.
            let _ = tx.send(fin);
        });
        self.running += 1;
    }

    /// Wait for a build to complete, with a timeout.
    /// If the timeout elapses return None.
    pub fn wait(&mut self, dur: Duration) -> Option<FinishedBuild> {
        let fin = match self.finished_recv.recv_timeout(dur) {
            Err(mpsc::RecvTimeoutError::Timeout) => return None,
            // The unwrap() checks the recv() call, to panic on mpsc errors.
            r => r.unwrap(),
        };
        self.tids.release(fin.tid);
        self.running -= 1;
        Some(fin)
    }
}
