//! Build runner, choosing and executing tasks as determined by out of date inputs.

use crate::db;
use crate::depfile;
use crate::graph::*;
use crate::scanner::Scanner;
use anyhow::{anyhow, bail};
use std::collections::HashSet;
use std::io::Write;

// We maintain a frontier of builds that are "ready", which means that any
// builds they depend upon have already been brought up to date.

// A ready build still may not have yet stat()ed its (non-generated) inputs, so
// doing those stat()s is part of the work of doing that build.  But it's
// guaranteed that all generated inputs have already been stat()ed as outputs by
// the build that generated those inputs.

struct Plan {
    /// Builds we want to ensure are up to date.
    want: HashSet<BuildId>,
    /// Builds whose generated inputs are up to date and are ready to be checked/hashed/run.
    ready: HashSet<BuildId>,
}

impl Plan {
    fn new() -> Self {
        Plan {
            want: HashSet::new(),
            ready: HashSet::new(),
        }
    }

    /// Visits a BuildId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    fn add_build(&mut self, graph: &Graph, id: BuildId) -> anyhow::Result<()> {
        if self.want.contains(&id) {
            return Ok(());
        }

        // Any Build that doesn't depend on an output of another Build is ready.
        let mut ready = true;
        for id in graph.build(id).depend_ins() {
            self.add_file(graph, id)?;
            ready = ready && !graph.file(id).input.is_some();
        }

        self.want.insert(id);
        if ready {
            self.ready.insert(id);
        }

        Ok(())
    }

    /// Visits a FileId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    pub fn add_file(&mut self, graph: &Graph, id: FileId) -> anyhow::Result<()> {
        if let Some(bid) = graph.file(id).input {
            self.add_build(graph, bid)?;
        }
        Ok(())
    }

    pub fn pop(&mut self) -> Option<BuildId> {
        if self.want.is_empty() {
            None
        } else {
            let id = match self.ready.iter().next() {
                Some(&id) => id,
                None => {
                    panic!("no builds ready, but still want {:?}", self.want);
                }
            };
            self.want.remove(&id);
            self.ready.remove(&id);
            Some(id)
        }
    }
}

pub struct Work<'a> {
    graph: &'a mut Graph,
    db: &'a mut db::Writer,

    file_state: FileState,
    last_hashes: &'a Hashes,
    plan: Plan,
}

impl<'a> Work<'a> {
    pub fn new(graph: &'a mut Graph, last_hashes: &'a Hashes, db: &'a mut db::Writer) -> Self {
        let file_state = FileState::new(graph);
        Work {
            graph: graph,
            db: db,
            file_state: file_state,
            last_hashes: last_hashes,
            plan: Plan::new(),
        }
    }

    pub fn want_file(&mut self, id: FileId) -> anyhow::Result<()> {
        self.plan.add_file(self.graph, id)
    }

    /// Check whether a given build is ready, generally after one of its inputs
    /// has been updated.
    fn recheck_ready(&self, id: BuildId) -> bool {
        let build = self.graph.build(id);
        // println!("  recheck {:?} {}", id, build.location);
        for id in build.depend_ins() {
            let file = self.graph.file(id);
            if file.input.is_none() {
                // Only generated inputs contribute to readiness.
                continue;
            }
            if self.file_state.get(id).is_none() {
                // println!("    {:?} {} not ready", id, file.name);
                return false;
            }
        }
        // println!("    now ready");
        true
    }

    fn read_depfile(&mut self, id: BuildId, path: &str) -> anyhow::Result<bool> {
        let mut bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path, e),
        };
        bytes.push(0);

        let mut scanner = Scanner::new(unsafe { std::str::from_utf8_unchecked(&bytes) });
        let parsed_deps = depfile::parse(&mut scanner)
            .map_err(|err| anyhow!("in {}: {}", path, scanner.format_parse_error(err)))?;
        // TODO verify deps refers to correct output
        let deps: Vec<FileId> = parsed_deps
            .deps
            .iter()
            .map(|&dep| self.graph.file_id(dep))
            .collect();

        let changed = if self.graph.build_mut(id).update_deps(deps) {
            println!("deps changed {:?}", self.graph.build(id).deps_ins());
            true
        } else {
            false
        };
        Ok(changed)
    }

    fn run_one(&mut self, id: BuildId) -> anyhow::Result<()> {
        let build = self.graph.build(id);
        let cmdline = match &build.cmdline {
            None => return Ok(()),
            Some(c) => c,
        };
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
        if let Some(depfile) = &build.depfile {
            let depfile = &depfile.clone();
            self.read_depfile(id, depfile)?;
        }

        // Rust thinks self.read_depfile may have modified build, so reread here.
        let build = self.graph.build(id);

        // We may have discovered new deps, so ensure we have mtimes for those.
        for &id in build.deps_ins() {
            if self.file_state.get(id).is_some() {
                // Already have state for this file.
                continue;
            }
            let file = self.graph.file(id);
            if file.input.is_some() {
                panic!("discovered new dep on generated file {}", file.name);
            }
            self.file_state.restat(id, &file.name)?;
        }

        let hash = hash_build(self.graph, &mut self.file_state, id)?;
        self.db.write_build(self.graph, id, hash)?;

        Ok(())
    }

    /// Given a build that just finished, check whether its dependent builds are now ready.
    fn ready_dependents(&mut self, id: BuildId) -> anyhow::Result<()> {
        let build = self.graph.build(id);
        let mut dependents = HashSet::new();
        for &id in build.outs() {
            for &id in &self.graph.file(id).dependents {
                if !self.plan.want.contains(&id) {
                    continue;
                }
                dependents.insert(id);
            }
        }
        for id in dependents {
            if !self.recheck_ready(id) {
                continue;
            }
            self.plan.ready.insert(id);
        }
        Ok(())
    }

    /// Check and potentially run a ready build.
    /// Prereq: any generated input is already generated.
    /// Non-generated inputs may not have been stat()ed yet.
    fn update_build(&mut self, id: BuildId) -> anyhow::Result<()> {
        let build = self.graph.build(id);
        // stat all non-generated inputs.
        for id in build.depend_ins() {
            let file = self.graph.file(id);
            if file.input.is_none() && self.file_state.get(id).is_none() {
                self.file_state.restat(id, &file.name)?;
            }
        }

        let hash = hash_build(self.graph, &mut self.file_state, id)?;
        if self.last_hashes.changed(id, hash) {
            self.run_one(id)?;
            println!("finished {:?} {}", id, self.graph.build(id).location);
        } else {
            println!("cached {:?} {}", id, self.graph.build(id).location);
        }

        self.ready_dependents(id)?;
        Ok(())
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        while let Some(id) = self.plan.pop() {
            self.update_build(id)?;
        }
        Ok(())
    }
}
