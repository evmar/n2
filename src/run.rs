//! Runs build tasks, potentially in parallel.

use crate::depfile;
use crate::graph::BuildId;
use crate::graph::Graph;
use crate::scanner::Scanner;
use anyhow::{anyhow, bail};
use std::io::Write;

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
    ready: Vec<BuildId>,
}

impl Runner {
    pub fn new() -> Self {
        Runner { ready: Vec::new() }
    }

    pub fn can_start_more(&self) -> bool {
        self.ready.len() == 0
    }

    pub fn start(&mut self, id: BuildId) {
        self.ready.push(id);
    }

    fn run_one(&self, graph: &Graph, id: BuildId) -> anyhow::Result<FinishedBuild> {
        let build = graph.build(id);
        let cmdline = match &build.cmdline {
            None => return Ok(FinishedBuild { id: id, deps: None }),
            Some(c) => c,
        };
        let fin = run_build(
            id,
            cmdline,
            build.depfile.as_ref().map(|path| path.as_str()),
        )?;
        Ok(fin)
    }

    pub fn wait(&mut self, graph: &Graph) -> anyhow::Result<Option<FinishedBuild>> {
        let id = match self.ready.pop() {
            None => return Ok(None),
            Some(id) => id,
        };
        Ok(Some(self.run_one(graph, id)?))
    }
}
