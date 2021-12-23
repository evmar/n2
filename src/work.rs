//! Build runner, choosing and executing tasks as determined by out of date inputs.

use crate::db;
use crate::graph::*;
use crate::run::FinishedBuild;
use crate::run::Runner;
use std::collections::HashSet;

/// Plan tracks progress through the build.
/// Builds go through a sequence of states, as tracked by membership in the sets
/// in this struct.  Any given build lives in only one of these sets.
struct Plan {
    /// Builds we want to ensure are up to date, but which aren't ready yet.
    want: HashSet<BuildId>,

    /// Builds whose generated inputs are up to date and are ready to be
    /// checked/hashed/run.
    /// Preconditions:
    /// - generated inputs: have already been stat()ed as part of completing
    ///   the step that generated those inputs
    /// - non-generated inputs: may not have yet stat()ed, so doing those
    ///   stat()s is part of the work of running these builds
    /// Note per these definitions, a build with missing non-generated inputs
    /// is still considered ready (but will then fail to run).
    ready: HashSet<BuildId>,

    // TODO: running?
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
        if self.want.contains(&id) || self.ready.contains(&id) {
            return Ok(());
        }

        // Any Build that doesn't depend on an output of another Build is ready.
        let mut ready = true;
        for id in graph.build(id).depend_ins() {
            self.add_file(graph, id)?;
            ready = ready && !graph.file(id).input.is_some();
        }

        if ready {
            self.ready.insert(id);
        } else {
            self.want.insert(id);
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

    pub fn pop_ready(&mut self) -> Option<BuildId> {
        // Here is where we might consider prioritizing from among the available
        // ready set.
        let id = match self.ready.iter().next() {
            Some(&id) => id,
            None => {
                panic!("no builds ready, but still want {:?}", self.want);
            }
        };
        self.ready.remove(&id);
        Some(id)
    }
}

pub struct Work<'a> {
    graph: &'a mut Graph,
    db: &'a mut db::Writer,

    file_state: FileState,
    last_hashes: &'a Hashes,
    plan: Plan,
    runner: Runner,
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
            runner: Runner::new(),
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

    /// Given a build that just finished, record its new deps and hash.
    fn record_finished(&mut self, fin: FinishedBuild) -> anyhow::Result<()> {
        let id = fin.id;
        let deps = match fin.deps {
            None => Vec::new(),
            Some(names) => names
                .iter()
                .map(|name| self.graph.file_id(name))
                .collect(),
        };
        let deps_changed = self.graph.build_mut(id).update_deps(deps);

        // We may have discovered new deps, so ensure we have mtimes for those.
        if deps_changed {
            for &id in self.graph.build(id).deps_ins() {
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
        }

        let hash = hash_build(self.graph, &mut self.file_state, id)?;
        self.db.write_build(self.graph, id, hash)?;

        Ok(())
    }

    /// Given a build that just finished, check whether its dependent builds are now ready.
    fn ready_dependents(&mut self, id: BuildId) {
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
            self.plan.want.remove(&id);
            self.plan.ready.insert(id);
        }
    }

    /// Check and potentially run a ready build.
    /// Prereq: any generated input is already generated.
    /// Non-generated inputs may not have been stat()ed yet.
    fn check_build(&mut self, id: BuildId) -> anyhow::Result<bool> {
        let build = self.graph.build(id);
        // stat all non-generated inputs.
        for id in build.depend_ins() {
            let file = self.graph.file(id);
            if file.input.is_none() && self.file_state.get(id).is_none() {
                self.file_state.restat(id, &file.name)?;
            }
        }

        let hash = hash_build(self.graph, &mut self.file_state, id)?;
        Ok(!self.last_hashes.changed(id, hash))
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        while self.plan.want.len() > 0 {
            while !self.runner.can_start_more() {
                let fin = self.runner.wait(self.graph)?.unwrap();
                let id = fin.id;
                println!("finished {:?} {}", id, self.graph.build(id).location);
                self.record_finished(fin)?;
                self.ready_dependents(id);
            }

            let id = self.plan.pop_ready().unwrap();
            if self.check_build(id)? {
                println!("cached {:?} {}", id, self.graph.build(id).location);
                self.ready_dependents(id);
            } else {
                self.runner.start(id);
            };
        }
        Ok(())
    }
}
