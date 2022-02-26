//! Build runner, choosing and executing tasks as determined by out of date inputs.

use crate::db;
use crate::graph::*;
use crate::progress;
use crate::progress::Progress;
use crate::run;
use crate::signal;
use crate::trace;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Duration;

/// Build steps go through this sequence of states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildState {
    /// Default initial state, for Builds unneeded by the current build.
    Unknown,
    /// Builds we want to ensure are up to date, but which aren't ready yet.
    Want,
    /// Builds whose dependencies are up to date and are ready to be
    /// checked.  This is purely a function of whether all builds before
    /// it have have run, and is independent of any file state.
    ///
    /// Preconditions:
    /// - generated inputs: have already been stat()ed as part of completing
    ///   the step that generated those inputs
    /// - non-generated inputs: may not have yet stat()ed, so doing those
    ///   stat()s is part of the work of running these builds
    Ready,
    /// Builds who have been determined not up to date and which are ready
    /// to be executed.
    Queued,
    /// Currently executing.
    Running,
    /// Finished executing.
    Done,
}

// Counters that track number of builds in each state.
// Only covers builds not in the "unknown" state, which means it's only builds
// that are considered part of the current build.
#[derive(Clone, Debug)]
pub struct StateCounts([usize; 5]);
impl StateCounts {
    pub fn new() -> Self {
        StateCounts([0; 5])
    }
    fn idx(state: BuildState) -> usize {
        match state {
            BuildState::Unknown => panic!("unexpected state"),
            BuildState::Want => 0,
            BuildState::Ready => 1,
            BuildState::Queued => 2,
            BuildState::Running => 3,
            BuildState::Done => 4,
        }
    }
    fn add(&mut self, state: BuildState, delta: isize) {
        self.0[StateCounts::idx(state)] =
            (self.0[StateCounts::idx(state)] as isize + delta) as usize;
    }
    pub fn get(&self, state: BuildState) -> usize {
        self.0[StateCounts::idx(state)]
    }
    pub fn total(&self) -> usize {
        self.0[0] + self.0[1] + self.0[2] + self.0[3] + self.0[4]
    }
}

/// Pools gather collections of running builds.
/// Each running build is running "in" a pool; there's a default unbounded
/// pool for builds that don't specify one.
struct PoolState {
    /// A queue of builds that are ready to be executed in this pool.
    queued: VecDeque<BuildId>,
    /// The number of builds currently running in this pool.
    running: usize,
    /// The total depth of the pool.  0 means unbounded.
    depth: usize,
}

impl PoolState {
    fn new(depth: usize) -> Self {
        PoolState {
            queued: VecDeque::new(),
            running: 0,
            depth,
        }
    }
}

/// BuildStates tracks progress of each Build step through the build.
struct BuildStates {
    /// Maps BuildId to BuildState.
    states: Vec<BuildState>,

    // Counts of builds in each state.
    counts: StateCounts,

    /// Builds in the ready state, stored redundantly for quick access.
    ready: HashSet<BuildId>,

    /// Named pools of queued and running builds.
    /// Builds otherwise default to using an unnamed infinite pool.
    /// We expect a relatively small number of pools, such that a Vec is more
    /// efficient than a HashMap.
    pools: Vec<(String, PoolState)>,
}

impl BuildStates {
    fn new(depths: Vec<(String, usize)>) -> Self {
        let mut pools: Vec<(String, PoolState)> = vec![
            // The implied default pool.
            (String::from(""), PoolState::new(0)),
        ];
        pools.extend(
            depths
                .into_iter()
                .map(|(name, depth)| (name, PoolState::new(depth))),
        );
        BuildStates {
            states: Vec::new(),
            counts: StateCounts::new(),
            ready: HashSet::new(),
            pools,
        }
    }

    fn get(&self, id: BuildId) -> BuildState {
        self.states
            .get(id.index())
            .map_or(BuildState::Unknown, |&s| s)
    }

    fn set(&mut self, id: BuildId, build: &Build, state: BuildState) {
        if id.index() >= self.states.len() {
            self.states.resize(id.index() + 1, BuildState::Unknown);
        }
        let prev = self.states[id.index()];
        // println!("{:?} {:?}=>{:?}", id, prev, state);
        self.states[id.index()] = state;
        match prev {
            BuildState::Ready => {
                self.ready.remove(&id);
            }
            BuildState::Running => {
                self.get_pool(build).unwrap().running -= 1;
            }
            _ => {}
        };
        if prev != BuildState::Unknown {
            self.counts.add(prev, -1);
        }
        match state {
            BuildState::Ready => {
                self.ready.insert(id);
            }
            BuildState::Running => {
                // Trace instants render poorly in the old Chrome UI, and
                // not at all in speedscope or Perfetto.
                // if self.counts.get(BuildState::Running) == 0 {
                //     trace::if_enabled(|t| t.write_instant("first build"));
                // }
                self.get_pool(build).unwrap().running += 1;
            }
            _ => {}
        };
        self.counts.add(state, 1);
        /*
        This is too expensive to log on every individual state change...
        trace::if_enabled(|t| {
            t.write_counts(
                "builds",
                [
                    ("want", self.counts.get(BuildState::Want)),
                    ("ready", self.counts.get(BuildState::Ready)),
                    ("queued", self.counts.get(BuildState::Queued)),
                    ("running", self.counts.get(BuildState::Running)),
                    ("done", self.counts.get(BuildState::Done)),
                ]
                .iter(),
            )
        });*/
    }

    fn unfinished(&self) -> bool {
        self.counts.get(BuildState::Want) > 0
            || self.counts.get(BuildState::Ready) > 0
            || self.counts.get(BuildState::Running) > 0
            || self.counts.get(BuildState::Queued) > 0
    }

    /// Visits a BuildId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    fn want_build(
        &mut self,
        graph: &Graph,
        stack: &mut Vec<FileId>,
        id: BuildId,
    ) -> anyhow::Result<()> {
        if self.get(id) != BuildState::Unknown {
            return Ok(()); // Already visited.
        }

        let build = graph.build(id);
        self.set(id, build, BuildState::Want);

        // Any Build that doesn't depend on an output of another Build is ready.
        let mut ready = true;
        for &id in build.ordering_ins() {
            self.want_file(graph, stack, id)?;
            ready = ready && graph.file(id).input.is_none();
        }

        if ready {
            self.set(id, build, BuildState::Ready);
        }
        Ok(())
    }

    /// Visits a FileId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    pub fn want_file(
        &mut self,
        graph: &Graph,
        stack: &mut Vec<FileId>,
        id: FileId,
    ) -> anyhow::Result<()> {
        // Check for a dependency cycle.
        if let Some(cycle) = stack.iter().position(|&sid| sid == id) {
            let mut err = "dependency cycle: ".to_string();
            for &id in stack[cycle..].iter() {
                err.push_str(&format!("{} -> ", graph.file(id).name));
            }
            err.push_str(&graph.file(id).name);
            anyhow::bail!(err);
        }

        stack.push(id);
        if let Some(bid) = graph.file(id).input {
            self.want_build(graph, stack, bid)?;
        }
        stack.pop();
        Ok(())
    }

    pub fn pop_ready(&mut self) -> Option<BuildId> {
        // Here is where we might consider prioritizing from among the available
        // ready set.
        let id = match self.ready.iter().next() {
            Some(&id) => id,
            None => return None,
        };
        Some(id)
    }

    /// Look up a PoolState by name.
    fn get_pool(&mut self, build: &Build) -> Option<&mut PoolState> {
        let name = build.pool.as_deref().unwrap_or("");
        for (key, pool) in self.pools.iter_mut() {
            if key == name {
                return Some(pool);
            }
        }
        None
    }

    /// Mark a build as ready to run.
    /// May fail if the build references an unknown pool.
    pub fn enqueue(&mut self, id: BuildId, build: &Build) -> anyhow::Result<()> {
        self.set(id, build, BuildState::Queued);
        let pool = self.get_pool(build).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: unknown pool {:?}",
                build.location,
                // Unnamed pool lookups always succeed, this error is about
                // named pools.
                build.pool.as_ref().unwrap()
            )
        })?;
        pool.queued.push_back(id);
        Ok(())
    }

    /// Pop a ready to run queued build.
    pub fn pop_queued(&mut self) -> Option<BuildId> {
        for (_, pool) in self.pools.iter_mut() {
            if pool.depth == 0 || pool.running < pool.depth {
                if let Some(id) = pool.queued.pop_front() {
                    return Some(id);
                }
            }
        }
        None
    }
}

pub struct Work<'a> {
    graph: &'a mut Graph,
    db: &'a mut db::Writer,

    progress: &'a mut dyn Progress,
    file_state: FileState,
    last_hashes: &'a Hashes,
    build_states: BuildStates,
    runner: run::Runner,
}

impl<'a> Work<'a> {
    pub fn new(
        graph: &'a mut Graph,
        last_hashes: &'a Hashes,
        db: &'a mut db::Writer,
        progress: &'a mut dyn Progress,
        pools: Vec<(String, usize)>,
        parallelism: usize,
    ) -> Self {
        let file_state = FileState::new(graph);
        Work {
            graph,
            db,
            progress,
            file_state,
            last_hashes,
            build_states: BuildStates::new(pools),
            runner: run::Runner::new(parallelism),
        }
    }

    pub fn want_file(&mut self, id: FileId) -> anyhow::Result<()> {
        let mut stack = Vec::new();
        self.build_states.want_file(self.graph, &mut stack, id)
    }

    /// Check whether a given build is ready, generally after one of its inputs
    /// has been updated.
    fn recheck_ready(&self, id: BuildId) -> bool {
        let build = self.graph.build(id);
        // println!("recheck {:?} {} ({}...)", id, build.location, self.graph.file(build.outs()[0]).name);
        for &id in build.ordering_ins() {
            let file = self.graph.file(id);
            match file.input {
                None => {
                    // Only generated inputs contribute to readiness.
                    continue;
                }
                Some(id) => {
                    if self.build_states.get(id) != BuildState::Done {
                        // println!("  {:?} {} not done, it's {:?}", id, file.name, self.build_states.get(id));
                        return false;
                    }
                }
            }
        }
        // println!("{:?} now ready", id);
        true
    }

    /// Given a build that just finished, record any discovered deps and hash.
    /// Postcondition: all outputs have been stat()ed.
    fn record_finished(&mut self, id: BuildId, result: run::BuildResult) -> anyhow::Result<()> {
        let deps = match result.discovered_deps {
            None => Vec::new(),
            Some(names) => names.iter().map(|name| self.graph.file_id(name)).collect(),
        };
        let deps_changed = self.graph.build_mut(id).update_discovered(deps);
        let build = self.graph.build(id);

        // We may have discovered new deps, so ensure we have mtimes for those.
        if deps_changed {
            for &id in build.discovered_ins() {
                if self.file_state.get(id).is_some() {
                    // Already have state for this file.
                    // Assume there's already some ordering dependency on this
                    // file.  TODO: should we verify this?
                    continue;
                }
                let file = self.graph.file(id);
                if file.input.is_some() {
                    panic!("discovered new dep on generated file {}", file.name);
                }
                self.file_state.restat(id, &file.name)?;
            }
        }

        // Stat all the outputs.  This step just finished, so we need to update
        // any cached state of the output files to reflect their new state.
        let mut output_missing = false;
        for &id in build.outs() {
            let file = self.graph.file(id);
            let mtime = self.file_state.restat(id, &file.name)?;
            if mtime == MTime::Missing {
                output_missing = true;
            }
        }

        if output_missing {
            // If a declared output is missing, don't record the build in
            // in the db.  It will be considered dirty next time anyway due
            // to the missing output.
            return Ok(());
        }

        let hash = hash_build(self.graph, &mut self.file_state, build)?;
        self.db.write_build(self.graph, id, hash)?;

        Ok(())
    }

    /// Given a build that just finished, check whether its dependent builds are now ready.
    fn ready_dependents(&mut self, id: BuildId) {
        let build = self.graph.build(id);
        self.build_states.set(id, build, BuildState::Done);

        let mut dependents = HashSet::new();
        for &id in build.outs() {
            for &id in &self.graph.file(id).dependents {
                if self.build_states.get(id) != BuildState::Want {
                    continue;
                }
                dependents.insert(id);
            }
        }
        for id in dependents {
            if !self.recheck_ready(id) {
                continue;
            }
            self.build_states.set(id, build, BuildState::Ready);
        }
    }

    /// Check a ready build for whether it needs to run, returning true if so.
    /// This is the place where we actually stat files.
    /// Prereq: any dependent input is already generated.
    fn check_build_dirty(&mut self, id: BuildId) -> anyhow::Result<bool> {
        let build = self.graph.build(id);

        let phony = build.cmdline.is_none();
        // TODO: do we just return true immediately if phony?
        // There are likely weird interactions with builds that depend on
        // a phony output, despite that not really making sense.

        // True if we need to work around
        //   https://github.com/ninja-build/ninja/issues/1779
        // which is a bug that a phony rule with a missing input
        // dependency doesn't fail the build.
        // TODO: key this behavior off of the "ninja compat" flag.
        // TODO: reconsider how phony deps work, maybe we should always promote
        // phony deps to order-only?
        let workaround_missing_phony_deps = phony;

        // stat any non-generated inputs if needed.
        // Note that generated inputs should already have been stat()ed when
        // they were visited as outputs.

        // For dirtying_ins, ensure we both have mtimes and that the files are present.
        for &id in build.dirtying_ins() {
            let file = self.graph.file(id);
            let mtime = match self.file_state.get(id) {
                Some(mtime) => mtime,
                None => {
                    if file.input.is_none() {
                        self.file_state.restat(id, &file.name)?
                    } else {
                        // This is a logic error in ninja; any generated file should
                        // already have been visited by this point.
                        panic!(
                            "{}: should already have file state for generated input {}",
                            build.location, &file.name
                        );
                    }
                }
            };
            if mtime == MTime::Missing {
                if workaround_missing_phony_deps {
                    continue;
                }
                anyhow::bail!("{}: input {} missing", build.location, file.name);
            }
        }

        // For order_only_ins, ensure that non-generated files are present.
        for &id in build.order_only_ins() {
            let file = self.graph.file(id);
            if file.input.is_some() {
                // Generated order-only input: we don't care if the file
                // exists or not, we only used it for ordering.
                continue;
            }
            let mtime = match self.file_state.get(id) {
                Some(mtime) => mtime,
                None => self.file_state.restat(id, &file.name)?,
            };
            if mtime == MTime::Missing {
                if workaround_missing_phony_deps {
                    continue;
                }
                anyhow::bail!("{}: input {} missing", build.location, file.name);
            }
        }

        // For discovered_ins, ensure we have mtimes for them.
        // But if they're missing, it isn't an error, it just means the build
        // is dirty.
        let mut file_missing = false;
        for &id in build.discovered_ins() {
            let file = self.graph.file(id);
            let mtime = match self.file_state.get(id) {
                Some(mtime) => mtime,
                None => {
                    if file.input.is_none() {
                        self.file_state.restat(id, &file.name)?
                    } else {
                        // This dep is generated by some other build step, but the
                        // build graph didn't cause that other build step to be
                        // visited first.  This is an error in the build file.
                        // For example, imagine:
                        //   build generated.h: codegen_headers ...
                        //   build generated.stamp: stamp || generated.h
                        //   build foo.o: cc ...
                        // If we deps discover that foo.o depends on generated.h,
                        // we must have some dependency path from foo.o to generated.h,
                        // either direct or indirect (like the stamp).  If that
                        // were present, then we'd already have file_state for this
                        // file and wouldn't get here.
                        anyhow::bail!(
                            "{} used {}, but has no dependency path to it",
                            build.location,
                            file.name
                        );
                    }
                }
            };
            if mtime == MTime::Missing {
                // It's ok if it's missing, but it means the build is dirty.
                file_missing = true;
            }
        }

        // Stat all the outputs.
        // We know this build is solely responsible for updating these outputs,
        // and if we're checking if it's dirty we are visiting it the first
        // time, so we stat unconditionally.
        // This is looking at if the outputs are already present.
        for &id in build.outs() {
            let file = self.graph.file(id);
            if self.file_state.get(id).is_some() {
                panic!("expected no file state for {}", file.name);
            }
            let mtime = self.file_state.restat(id, &file.name)?;
            if mtime == MTime::Missing {
                file_missing = true;
            }
        }

        if file_missing {
            // If any files are missing, the build is dirty without needing
            // to consider hashes.  But a phony build can never be dirty.
            let dirty = !phony;
            return Ok(dirty);
        }

        // If we get here, all the relevant files are present and stat()ed,
        // so compare the hash against the last hash.
        // TODO: skip this whole function if no previous hash is present.
        let hash = hash_build(self.graph, &mut self.file_state, build)?;
        Ok(self.last_hashes.changed(id, hash))
    }

    // Runs the build.
    // Returns a Result for failures, but we must clean up the progress before
    // returning the result to the caller.
    fn run_without_cleanup(&mut self) -> anyhow::Result<bool> {
        signal::register_sigint();
        while self.build_states.unfinished() {
            self.progress.update(&self.build_states.counts);

            // Approach:
            // - First make sure we're running as many queued tasks as the runner
            //   allows.
            // - Next make sure we've finished or enqueued any tasks that are
            //   ready.
            // - If either one of those made progress, loop, to ensure the other
            //   one gets to work from the result.
            // - If neither made progress, wait for a task to complete and
            //   loop.

            let mut made_progress = false;
            while self.runner.can_start_more() {
                let id = match self.build_states.pop_queued() {
                    Some(id) => id,
                    None => break,
                };
                let build = self.graph.build(id);
                self.build_states.set(id, build, BuildState::Running);
                self.runner
                    .start(id, build.cmdline.clone().unwrap(), build.depfile.clone());
                self.progress.task_state(id, build, BuildState::Running);
                made_progress = true;
            }

            while let Some(id) = self.build_states.pop_ready() {
                if !self.check_build_dirty(id)? {
                    // Not dirty; go directly to the Done state.
                    self.ready_dependents(id);
                } else {
                    self.build_states.enqueue(id, self.graph.build(id))?;
                }
                made_progress = true;
            }

            if made_progress {
                continue;
            }

            if !self.runner.is_running() {
                panic!("no work to do and runner not running?");
            }

            // Flush progress here, to ensure that the progress is the most up
            // to date before we wait.  Otherwise the progress might seem like
            // we're doing nothing while we wait.
            self.progress.flush();
            let fin = match self.runner.wait(Duration::from_millis(500)) {
                None => continue, // timeout
                Some(fin) => fin,
            };
            let build = self.graph.build(fin.id);
            trace::if_enabled(|t| {
                let desc = progress::build_message(build);
                t.write_complete(desc, fin.tid + 1, fin.span.0, fin.span.1);
            });

            if !fin.result.success {
                self.progress.failed(build, &fin.result.output);
                return Ok(false);
            }

            self.record_finished(fin.id, fin.result)?;
            self.progress
                .task_state(fin.id, self.graph.build(fin.id), BuildState::Done);
            self.ready_dependents(fin.id);
        }

        Ok(true)
    }

    pub fn run(&mut self) -> anyhow::Result<bool> {
        let result = self.run_without_cleanup();
        // Clean up progress before returning.
        self.progress.update(&self.build_states.counts);
        self.progress.finish();
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_cycle() -> Result<(), anyhow::Error> {
        let file = "
build a: phony b
build b: phony c
build c: phony a
";
        let mut graph = crate::load::parse("build.ninja", file.as_bytes().to_vec())?;
        let a_id = graph.file_id("a");
        let mut states = crate::work::BuildStates::new(vec![]);
        let mut stack = Vec::new();
        match states.want_file(&graph, &mut stack, a_id) {
            Ok(_) => panic!("expected build cycle error"),
            Err(err) => assert_eq!(err.to_string(), "dependency cycle: a -> b -> c -> a"),
        }
        Ok(())
    }
}
