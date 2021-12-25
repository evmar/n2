//! Runs build tasks, potentially in parallel.
//!
//! TODO: consider rewriting to use poll() etc. instead of threads.
//! The threads might be relatively cheap(?) because they just block on
//! the subprocesses though?

use crate::depfile;
use crate::graph::BuildId;
use crate::scanner::Scanner;
use anyhow::{anyhow, bail};
use std::io::Write;
use std::sync::mpsc;

pub struct FinishedBuild {
    pub id: BuildId,
    // TODO: console output
    pub deps: Option<Vec<String>>,
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
        .map_err(|err| anyhow!("in {}: {}", path, scanner.format_parse_error(err)))?;
    // TODO verify deps refers to correct output
    let deps: Vec<String> = parsed_deps
        .deps
        .iter()
        .map(|&dep| dep.to_string())
        .collect();
    Ok(deps)
}

/// Executes a build step as a subprocess.
fn run_build(id: BuildId, cmdline: &str, depfile: Option<&str>) -> anyhow::Result<FinishedBuild> {
    println!("$ {}", cmdline);
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmdline)
        .output()?;
    if !output.stdout.is_empty() {
        std::io::stdout().write_all(&output.stdout)?;
    }
    if !output.stderr.is_empty() {
        std::io::stdout().write_all(&output.stderr)?;
    }
    if !output.status.success() {
        bail!("subcommand failed");
    }
    let deps = match depfile {
        None => None,
        Some(deps) => Some(read_depfile(deps)?),
    };

    Ok(FinishedBuild { id: id, deps: deps })
}

pub struct Runner {
    finished_send: mpsc::Sender<anyhow::Result<FinishedBuild>>,
    finished_recv: mpsc::Receiver<anyhow::Result<FinishedBuild>>,
    pub running: usize,
}

impl Runner {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Runner {
            finished_send: tx,
            finished_recv: rx,
            running: 0,
        }
    }

    pub fn can_start_more(&self) -> bool {
        self.running < 8
    }

    pub fn is_running(&self) -> bool {
        self.running > 0
    }

    pub fn start(&mut self, id: BuildId, cmdline: &str, depfile: Option<&str>) {
        let cmdline = cmdline.to_string();
        let depfile = depfile.map(|path| path.to_string());
        let tx = self.finished_send.clone();
        std::thread::spawn(move || {
            let fin = run_build(id, &cmdline, depfile.as_ref().map(|s| s.as_str()));
            tx.send(fin).unwrap();
        });
        self.running += 1;
    }

    pub fn wait(&mut self) -> anyhow::Result<FinishedBuild> {
        // The unwrap() checks the recv() call (panics on mpsc error).
        let r = self.finished_recv.recv().unwrap();
        self.running -= 1;
        r
    }
}
