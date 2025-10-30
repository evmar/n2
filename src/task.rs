//! Runs build tasks, potentially in parallel.
//! Unaware of the build graph, pools, etc.; just command execution.
//!
//! We use one thread per subprocess.  This differs from Ninja which goes to
//! some effort to use ppoll-like behavior.  Because the threads are mostly
//! blocked in IO I don't expect this to be too costly in terms of CPU, but it's
//! worth considering how much RAM it costs.  On the positive side, the logic
//! is significantly simpler than Ninja and we get free behaviors like parallel
//! parsing of depfiles.

use crate::{
    depfile,
    graph::{Build, BuildId, RspFile},
    process,
    scanner::{self, Scanner},
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
    let bytes = match scanner::read_file_with_nul(path) {
        Ok(b) => b,
        // See discussion of missing depfiles in #80.
        // TODO(#99): warn or error in this circumstance?
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => bail!("read {}: {}", path.display(), e),
    };

    let mut scanner = Scanner::new(&bytes);
    let parsed_deps = depfile::parse(&mut scanner)
        .map_err(|err| anyhow!(scanner.format_parse_error(path, err)))?;
    // TODO verify deps refers to correct output
    let deps: Vec<String> = parsed_deps
        .values()
        .flat_map(|x| x.iter())
        .map(|&dep| dep.to_owned())
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

/// Parse some subcommand output to extract "Note: including file:" lines as
/// emitted by MSVC/clang-cl.
fn extract_showincludes(output: Vec<u8>) -> (Vec<String>, Vec<u8>) {
    let mut filtered_output = Vec::new();
    let mut includes = Vec::new();
    for line in output.split(|&c| c == b'\n') {
        if let Some(include) = line.strip_prefix(b"Note: including file: ") {
            let start = include.iter().position(|&c| c != b' ').unwrap_or(0);
            let end = if include.ends_with(&[b'\r']) {
                include.len() - 1
            } else {
                include.len()
            };
            let include = &include[start..end];
            includes.push(unsafe { String::from_utf8_unchecked(include.to_vec()) });
        } else {
            if !filtered_output.is_empty() {
                filtered_output.push(b'\n');
            }
            filtered_output.extend_from_slice(line);
        }
    }
    (includes, filtered_output)
}

/// Find the span of the last line of text in buf, ignoring trailing empty
/// lines.
fn find_last_line(buf: &[u8]) -> &[u8] {
    fn is_nl(c: u8) -> bool {
        c == b'\r' || c == b'\n'
    }

    let end = match buf.iter().rposition(|&c| !is_nl(c)) {
        Some(pos) => pos + 1,
        None => buf.len(),
    };
    let start = match buf[..end].iter().rposition(|&c| is_nl(c)) {
        Some(pos) => pos + 1,
        None => 0,
    };
    &buf[start..end]
}

/// Executes a build task as a subprocess.
/// Returns an Err() if we failed outside of the process itself.
/// This is run as a separate thread from the main n2 process and will block
/// on the subprocess, so any additional per-subprocess work we can do belongs
/// here.
fn run_task(
    cmdline: &str,
    depfile: Option<&Path>,
    parse_showincludes: bool,
    rspfile: Option<&RspFile>,
    mut last_line_cb: impl FnMut(&[u8]),
) -> anyhow::Result<TaskResult> {
    if let Some(rspfile) = rspfile {
        write_rspfile(rspfile)?;
    }

    let mut output = Vec::new();
    let termination = process::run_command(cmdline, |buf| {
        output.extend_from_slice(buf);
        last_line_cb(find_last_line(&output));
    })?;

    let mut discovered_deps = None;
    if parse_showincludes {
        // Remove /showIncludes lines from output, regardless of success/fail.
        let (includes, filtered) = extract_showincludes(output);
        output = filtered;
        discovered_deps = Some(includes);
    }
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

enum Message {
    Output((BuildId, Vec<u8>)),
    Done(FinishedTask),
}

pub struct Runner {
    tx: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    pub running: usize,
    tids: ThreadIds,
    parallelism: usize,
}

impl Runner {
    pub fn new(parallelism: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        Runner {
            tx,
            rx,
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

    pub fn start(&mut self, id: BuildId, build: &Build) {
        let cmdline = build.cmdline.clone().unwrap();
        let depfile = build.depfile.clone().map(PathBuf::from);
        let rspfile = build.rspfile.clone();
        let parse_showincludes = build.parse_showincludes();
        let hide_progress = build.hide_progress;

        let tid = self.tids.claim();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = run_task(
                &cmdline,
                depfile.as_deref(),
                parse_showincludes,
                rspfile.as_ref(),
                |line| {
                    if !hide_progress {
                        let _ = tx.send(Message::Output((id, line.to_owned())));
                    }
                },
            )
            .unwrap_or_else(|err| TaskResult {
                termination: process::Termination::Failure,
                output: format!("{}\n", err).into_bytes(),
                discovered_deps: None,
            });
            let finish = Instant::now();

            let task = FinishedTask {
                tid,
                buildid: id,
                span: (start, finish),
                result,
            };
            // The send will only fail if the receiver disappeared, e.g. due to shutting down.
            let _ = tx.send(Message::Done(task));
        });
        self.running += 1;
    }

    /// Wait for a build to complete.  May block for a long time.
    pub fn wait(&mut self, mut output: impl FnMut(BuildId, Vec<u8>)) -> FinishedTask {
        loop {
            match self.rx.recv().unwrap() {
                Message::Output((bid, line)) => output(bid, line),
                Message::Done(task) => {
                    self.tids.release(task.tid);
                    self.running -= 1;
                    return task;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_includes() {
        let (includes, output) = extract_showincludes(
            b"some text
Note: including file: a
other text
Note: including file: b\r
more text
"
            .to_vec(),
        );
        assert_eq!(includes, &["a", "b"]);
        assert_eq!(
            output,
            b"some text
other text
more text
"
        );
    }

    #[test]
    fn find_last() {
        assert_eq!(find_last_line(b""), b"");
        assert_eq!(find_last_line(b"\n"), b"");

        assert_eq!(find_last_line(b"hello"), b"hello");
        assert_eq!(find_last_line(b"hello\n"), b"hello");

        assert_eq!(find_last_line(b"hello\nt"), b"t");
        assert_eq!(find_last_line(b"hello\nt\n"), b"t");

        assert_eq!(find_last_line(b"hello\n\n"), b"hello");
        assert_eq!(find_last_line(b"hello\nt\n\n"), b"t");
    }

    #[test]
    fn missing_depfile_allowed() {
        let deps = read_depfile(Path::new("/missing/dep/file")).unwrap();
        assert_eq!(deps.len(), 0);
    }
}
