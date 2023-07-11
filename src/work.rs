//! Build runner, choosing and executing tasks as determined by out of date inputs.

use crate::{
    canon::canon_path, db, densemap::DenseMap, graph::*, hash, process, progress,
    progress::Progress, signal, smallmap::SmallMap, task, trace,
};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;

/// Build steps go through this sequence of states.
/// See "Build states" in the design notes.
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
    /// Finished executing successfully.
    Done,
    /// Finished executing but failed.
    Failed,
}

/// Counters that track builds in each state, excluding phony builds.
/// This is only for display to the user and should not be used as a source of
/// truth for tracking progress.
/// Only covers builds not in the "unknown" state, which means it's only builds
/// that are considered part of the current build.
#[derive(Clone, Debug, Default)]
pub struct StateCounts([usize; 6]);
impl StateCounts {
    fn idx(state: BuildState) -> usize {
        match state {
            BuildState::Unknown => panic!("unexpected state"),
            BuildState::Want => 0,
            BuildState::Ready => 1,
            BuildState::Queued => 2,
            BuildState::Running => 3,
            BuildState::Done => 4,
            BuildState::Failed => 5,
        }
    }
    pub fn add(&mut self, state: BuildState, delta: isize) {
        self.0[StateCounts::idx(state)] =
            (self.0[StateCounts::idx(state)] as isize + delta) as usize;
    }
    pub fn get(&self, state: BuildState) -> usize {
        self.0[StateCounts::idx(state)]
    }
    pub fn total(&self) -> usize {
        self.0[0] + self.0[1] + self.0[2] + self.0[3] + self.0[4] + self.0[5]
    }
}

/// Pools gather collections of running builds.
/// Each running build is running "in" a pool; there's a default unbounded
/// pool for builds that don't specify one.
/// See "Tracking build state" in the design notes.
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
/// See "Tracking build state" in the design notes.
struct BuildStates {
    states: DenseMap<BuildId, BuildState>,

    /// Counts of builds in each state.
    counts: StateCounts,

    /// Total number of builds that haven't been driven to completion
    /// (done or failed).
    total_pending: usize,

    /// Builds in the ready state, stored redundantly for quick access.
    ready: VecDeque<BuildId>,

    /// Named pools of queued and running builds.
    /// Builds otherwise default to using an unnamed infinite pool.
    pools: SmallMap<String, PoolState>,
}

impl BuildStates {
    fn new(size: BuildId, depths: SmallMap<String, usize>) -> Self {
        let mut pools = SmallMap::default();
        // The implied default pool.
        pools.insert(String::from(""), PoolState::new(0));
        // TODO: the console pool is just a depth-1 pool for now.
        pools.insert(String::from("console"), PoolState::new(1));
        for (name, depth) in depths.into_iter() {
            pools.insert(name, PoolState::new(depth));
        }
        BuildStates {
            states: DenseMap::new_sized(size, BuildState::Unknown),
            counts: StateCounts::default(),
            total_pending: 0,
            ready: VecDeque::new(),
            pools,
        }
    }

    fn get(&self, id: BuildId) -> BuildState {
        self.states[id]
    }

    fn set(&mut self, id: BuildId, build: &Build, state: BuildState) {
        // This function is called on all state transitions.
        // We get 'prev', the previous state, and 'state', the new state.
        let prev = std::mem::replace(&mut self.states[id], state);

        // We skip user-facing counters for phony builds.
        let skip_ui_count = build.cmdline.is_none();

        // println!("{:?} {:?}=>{:?} {:?}", id, prev, state, self.counts);
        if prev == BuildState::Unknown {
            self.total_pending += 1;
        } else {
            if prev == BuildState::Running {
                self.get_pool(build).unwrap().running -= 1;
            }
            if !skip_ui_count {
                self.counts.add(prev, -1);
            }
        }

        match state {
            BuildState::Ready => {
                self.ready.push_back(id);
            }
            BuildState::Running => {
                // Trace instants render poorly in the old Chrome UI, and
                // not at all in speedscope or Perfetto.
                // if self.counts.get(BuildState::Running) == 0 {
                //     trace::if_enabled(|t| t.write_instant("first build"));
                // }
                self.get_pool(build).unwrap().running += 1;
            }
            BuildState::Done | BuildState::Failed => {
                self.total_pending -= 1;
            }
            _ => {}
        };
        if !skip_ui_count {
            self.counts.add(state, 1);
        }

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
        self.total_pending > 0
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

        let build = &graph.builds[id];
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
        self.ready.pop_front()
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

pub struct Options {
    pub failures_left: Option<usize>,
    pub parallelism: usize,
    /// When true, verbosely explain why targets are considered dirty.
    pub explain: bool,
    /// When true, just mark targets up to date without running anything.
    pub adopt: bool,
}

pub struct Work<'a> {
    graph: Graph,
    db: db::Writer,
    progress: &'a mut dyn Progress,
    failures_left: Option<usize>,
    explain: bool,
    adopt: bool,
    file_state: FileState,
    last_hashes: Hashes,
    build_states: BuildStates,
    runner: task::Runner,
}

impl<'a> Work<'a> {
    pub fn new(
        graph: Graph,
        last_hashes: Hashes,
        db: db::Writer,
        options: &Options,
        progress: &'a mut dyn Progress,
        pools: SmallMap<String, usize>,
    ) -> Self {
        let file_state = FileState::new(&graph);
        let build_count = graph.builds.next_id();
        Work {
            graph,
            db,
            progress,
            failures_left: options.failures_left,
            explain: options.explain,
            adopt: options.adopt,
            file_state,
            last_hashes,
            build_states: BuildStates::new(build_count, pools),
            runner: task::Runner::new(options.parallelism),
        }
    }

    pub fn lookup(&mut self, name: &str) -> Option<FileId> {
        self.graph.files.lookup(&canon_path(name))
    }

    pub fn want_file(&mut self, id: FileId) -> anyhow::Result<()> {
        let mut stack = Vec::new();
        self.build_states.want_file(&self.graph, &mut stack, id)
    }

    /// Check whether a given build is ready, generally after one of its inputs
    /// has been updated.
    fn recheck_ready(&self, id: BuildId) -> bool {
        let build = &self.graph.builds[id];
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

    /// Return the id of any input file to a ready build step that is missing.
    /// Assumes the input dependencies have already executed, but otherwise
    /// may stat the file on disk.
    fn ensure_input_files(
        &mut self,
        id: BuildId,
        discovered: bool,
    ) -> anyhow::Result<Option<FileId>> {
        let build = &self.graph.builds[id];
        let ids = if discovered {
            build.discovered_ins()
        } else {
            build.dirtying_ins()
        };
        for &id in ids {
            let mtime = match self.file_state.get(id) {
                Some(mtime) => mtime,
                None => {
                    let file = self.graph.file(id);
                    if file.input.is_some() {
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
                            "used generated file {}, but has no dependency path to it",
                            file.name
                        );
                    }
                    self.file_state.stat(id, file.path())?
                }
            };
            if mtime == MTime::Missing {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Given a task that just finished, record any discovered deps and hash.
    /// Postcondition: all outputs have been stat()ed.
    fn record_finished(&mut self, id: BuildId, result: task::TaskResult) -> anyhow::Result<()> {
        // Clean up the deps discovered from the task.
        let mut deps = Vec::new();
        if let Some(names) = result.discovered_deps {
            for name in names {
                let fileid = self.graph.files.id_from_canonical(canon_path(name));
                // Filter duplicates from the file list.
                if deps.contains(&fileid) {
                    continue;
                }
                // Filter out any deps that were already dirtying in the build file.
                // Note that it's allowed to have a duplicate against an order-only
                // dep; see `discover_existing_dep` test.
                if self.graph.builds[id].dirtying_ins().contains(&fileid) {
                    continue;
                }
                deps.push(fileid);
            }
        }

        // We may have discovered new deps, so ensure we have mtimes for those.
        let deps_changed = self.graph.builds[id].update_discovered(deps);
        if deps_changed {
            if let Some(missing) = self.ensure_input_files(id, true)? {
                anyhow::bail!(
                    "{} depfile references nonexistent {}",
                    self.graph.builds[id].location,
                    self.graph.file(missing).name
                );
            }
        }

        let build = &self.graph.builds[id];

        let input_was_missing = build
            .dirtying_ins()
            .iter()
            .any(|&id| self.file_state.get(id).unwrap() == MTime::Missing);

        // Stat all the outputs.  This step just finished, so we need to update
        // any cached state of the output files to reflect their new state.
        let mut output_missing = false;
        for &id in build.outs() {
            let file = self.graph.file(id);
            let mtime = self.file_state.stat(id, file.path())?;
            if mtime == MTime::Missing {
                output_missing = true;
            }
        }

        if input_was_missing || output_missing {
            // If a file is missing, don't record the build in in the db.
            // It will be considered dirty next time anyway due to the missing file.
            return Ok(());
        }

        let hash = hash::hash_build(&self.graph.files, &self.file_state, build);
        self.db.write_build(&self.graph, id, hash)?;

        Ok(())
    }

    /// Given a build that just finished, check whether its dependent builds are now ready.
    fn ready_dependents(&mut self, id: BuildId) {
        let build = &self.graph.builds[id];
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
            self.build_states
                .set(id, &self.graph.builds[id], BuildState::Ready);
        }
    }

    /// Stat all the input/output files for a given build in anticipation of
    /// deciding whether it needs to be run again.
    /// Prereq: any dependent input is already generated.
    /// Returns a build error if any required input files are missing.
    /// Otherwise returns the missing id if any expected but not required files,
    /// e.g. outputs, are missing, implying that the build needs to be executed.
    fn check_build_files_missing(&mut self, id: BuildId) -> anyhow::Result<Option<FileId>> {
        let phony = self.graph.builds[id].cmdline.is_none();
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

        // Ensure we have state for all input files.
        if let Some(missing) = self.ensure_input_files(id, false)? {
            let file = self.graph.file(missing);
            if file.input.is_none() && !workaround_missing_phony_deps {
                let build = &self.graph.builds[id];
                anyhow::bail!("{}: input {} missing", build.location, file.name);
            }
            return Ok(Some(missing));
        }
        if let Some(missing) = self.ensure_input_files(id, true)? {
            return Ok(Some(missing));
        }

        // Stat all the outputs.
        // We know this build is solely responsible for updating these outputs,
        // and if we're checking if it's dirty we are visiting it the first
        // time, so we stat unconditionally.
        // This is looking at if the outputs are already present.
        for &id in self.graph.builds[id].outs() {
            let file = self.graph.file(id);
            if self.file_state.get(id).is_some() {
                panic!("expected no file state for {}", file.name);
            }
            let mtime = self.file_state.stat(id, file.path())?;
            if mtime == MTime::Missing {
                return Ok(Some(id));
            }
        }

        // All files accounted for.
        Ok(None)
    }

    /// Check a ready build for whether it needs to run, returning true if so.
    /// Prereq: any dependent input is already generated.
    fn check_build_dirty(&mut self, id: BuildId) -> anyhow::Result<bool> {
        let file_missing = self.check_build_files_missing(id)?;

        let build = &self.graph.builds[id];

        // A phony build can never be dirty.
        let phony = build.cmdline.is_none();
        if phony {
            return Ok(false);
        }

        // If any files are missing, the build is dirty without needing
        // to consider hashes.
        if let Some(missing) = file_missing {
            if self.explain {
                self.progress.log(&format!(
                    "explain: {}: input {} missing",
                    build.location,
                    self.graph.file(missing).name
                ));
            }
            return Ok(true);
        }

        // If we get here, all the relevant files are present and stat()ed,
        // so compare the hash against the last hash.

        // TODO: skip this whole function if no previous hash is present.
        // More complex than just moving this block up, because we currently
        // assume that we've always checked inputs after we've run a build.
        let prev_hash = match self.last_hashes.get(id) {
            None => {
                if self.explain {
                    self.progress.log(&format!(
                        "explain: {}: no previous state known",
                        build.location
                    ));
                }
                return Ok(true);
            }
            Some(prev_hash) => prev_hash,
        };

        let hash = hash::hash_build(&self.graph.files, &self.file_state, build);
        if prev_hash != hash {
            if self.explain {
                self.progress
                    .log(&format!("explain: {}: manifest changed", build.location));
                self.progress.log(&hash::explain_hash_build(
                    &self.graph.files,
                    &self.file_state,
                    build,
                ));
            }
            return Ok(true);
        }

        Ok(false)
    }

    /// Create the parent directories of a given list of fileids.
    /// Used to create directories used for outputs.
    /// TODO: do this within the thread executing the subtask?
    fn create_parent_dirs(&self, ids: &[FileId]) -> anyhow::Result<()> {
        let mut dirs: Vec<&std::path::Path> = Vec::new();
        for &out in ids {
            if let Some(parent) = self.graph.file(out).path().parent() {
                if dirs.iter().any(|&p| p == parent) {
                    continue;
                }
                std::fs::create_dir_all(parent)?;
                dirs.push(parent);
            }
        }
        Ok(())
    }

    /// Runs the build.
    /// Returns the number of tasks executed on successful builds, or None on failed builds.
    pub fn run(&mut self) -> anyhow::Result<Option<usize>> {
        #[cfg(unix)]
        signal::register_sigint();
        let mut tasks_done = 0;
        let mut tasks_failed = 0;
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
                let build = &self.graph.builds[id];
                self.build_states.set(id, build, BuildState::Running);
                self.create_parent_dirs(build.outs())?;
                self.runner.start(
                    id,
                    build.cmdline.clone().unwrap(),
                    build.depfile.clone().map(PathBuf::from),
                    build.rspfile.clone(),
                );
                self.progress.task_state(id, build, None);
                made_progress = true;
            }

            while let Some(id) = self.build_states.pop_ready() {
                if !self.check_build_dirty(id)? {
                    // Not dirty; go directly to the Done state.
                    self.ready_dependents(id);
                } else {
                    if self.adopt {
                        // Act as if the target already finished.
                        self.record_finished(
                            id,
                            task::TaskResult {
                                termination: process::Termination::Success,
                                output: vec![],
                                discovered_deps: None,
                            },
                        )?;
                        self.ready_dependents(id);
                    } else {
                        self.build_states.enqueue(id, &self.graph.builds[id])?;
                    }
                }
                made_progress = true;
            }

            if made_progress {
                continue;
            }

            if !self.runner.is_running() {
                if tasks_failed > 0 {
                    // No more progress can be made, hopefully due to tasks that failed.
                    break;
                }
                panic!("BUG: no work to do and runner not running");
            }

            let task = self.runner.wait();
            let build = &self.graph.builds[task.buildid];
            trace::if_enabled(|t| {
                let desc = progress::build_message(build);
                t.write_complete(desc, task.tid + 1, task.span.0, task.span.1);
            });

            self.progress
                .task_state(task.buildid, build, Some(&task.result));
            match task.result.termination {
                process::Termination::Failure => {
                    if let Some(failures_left) = &mut self.failures_left {
                        *failures_left -= 1;
                        if *failures_left == 0 {
                            return Ok(None);
                        }
                    }
                    tasks_failed += 1;
                    self.build_states
                        .set(task.buildid, build, BuildState::Failed);
                }
                process::Termination::Interrupted => {
                    // If the task was interrupted bail immediately.
                    return Ok(None);
                }
                process::Termination::Success => {
                    tasks_done += 1;
                    self.record_finished(task.buildid, task.result)?;
                    self.ready_dependents(task.buildid);
                }
            };
        }

        // If the user ctl-c's, it likely caused a subtask to fail.
        // But at least for the LLVM test suite it can catch sigint and print
        // "interrupted by user" and exit with success, and in that case we
        // don't want n2 to print a "succeeded" message afterwards.
        let success = tasks_failed == 0 && !signal::was_interrupted();
        Ok(success.then(|| tasks_done))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cycle() -> Result<(), anyhow::Error> {
        let file = "
build a: phony b
build b: phony c
build c: phony a
";
        let mut graph = crate::load::parse("build.ninja", file.as_bytes().to_vec())?;
        let a_id = graph.files.id_from_canonical("a");
        let mut states = BuildStates::new(graph.builds.next_id(), SmallMap::default());
        let mut stack = Vec::new();
        match states.want_file(&graph, &mut stack, a_id) {
            Ok(_) => panic!("expected build cycle error"),
            Err(err) => assert_eq!(err.to_string(), "dependency cycle: a -> b -> c -> a"),
        }
        Ok(())
    }
}
