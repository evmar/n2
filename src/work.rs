//! Build runner, choosing and executing tasks as determined by out of date inputs.

use crate::db;
use crate::graph::*;
use crate::progress::Progress;
use crate::run::FinishedBuild;
use crate::run::Runner;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Duration;

/// Build steps go through this sequence of states.
#[derive(Clone, Copy, PartialEq)]
pub enum BuildState {
    /// Default initial state, for Builds unneeded by the current build.
    Unknown,
    /// Builds we want to ensure are up to date, but which aren't ready yet.
    Want,
    /// Builds whose generated inputs are up to date and are ready to be
    /// checked/hashed.
    ///
    /// Preconditions:
    /// - generated inputs: have already been stat()ed as part of completing
    ///   the step that generated those inputs
    /// - non-generated inputs: may not have yet stat()ed, so doing those
    ///   stat()s is part of the work of running these builds
    /// Note per these definitions, a build with missing non-generated inputs
    /// is still considered ready (but will then fail to run).
    Ready,
    /// Builds who have been determined not up to date and which are ready
    /// to be executed.
    Queued,
    /// Currently executing.
    Running,
    /// Finished executing.
    Done,
}

// Counters tracking number of builds in each state
struct StateCounts([usize; 5]);
impl StateCounts {
    fn new() -> Self {
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
    fn get(&self, state: BuildState) -> usize {
        self.0[StateCounts::idx(state)]
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
struct BuildStates<'a> {
    /// Maps BuildId to BuildState.
    states: Vec<BuildState>,

    /// Number of builds that are desired but not complete yet.
    pending: usize,

    // Counts of builds in each state.
    counts: StateCounts,

    /// Builds in the ready state, stored redundantly for quick access.
    ready: HashSet<BuildId>,

    /// Named pools of queued and running builds.
    /// Builds otherwise default to using an unnamed infinite pool.
    /// We expect a relatively small number of pools, such that a Vec is more
    /// efficient than a HashMap.
    pools: Vec<(String, PoolState)>,

    progress: &'a mut dyn Progress,
}

impl<'a> BuildStates<'a> {
    fn new(progress: &'a mut dyn Progress, depths: Vec<(String, usize)>) -> Self {
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
            pending: 0,
            counts: StateCounts::new(),
            ready: HashSet::new(),
            progress,
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
            BuildState::Want => self.pending += 1,
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
            BuildState::Done => self.pending -= 1,
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
        self.progress.build_state(id, build, prev, state);
    }

    fn unfinished(&self) -> bool {
        self.pending > 0
    }

    /// Visits a BuildId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    fn want_build(&mut self, graph: &Graph, id: BuildId) {
        if self.get(id) != BuildState::Unknown {
            return; // Already visited.
        }

        let build = graph.build(id);
        self.set(id, build, BuildState::Want);

        // Any Build that doesn't depend on an output of another Build is ready.
        let mut ready = true;
        for id in build.depend_ins() {
            self.want_file(graph, id);
            ready = ready && graph.file(id).input.is_none();
        }

        if ready {
            self.set(id, build, BuildState::Ready);
        }
    }

    /// Visits a FileId that is an input to the desired output.
    /// Will recursively visit its own inputs.
    pub fn want_file(&mut self, graph: &Graph, id: FileId) {
        if let Some(bid) = graph.file(id).input {
            self.want_build(graph, bid);
        }
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

    file_state: FileState,
    last_hashes: &'a Hashes,
    build_states: BuildStates<'a>,
    runner: Runner,
}

impl<'a> Work<'a> {
    pub fn new(
        graph: &'a mut Graph,
        last_hashes: &'a Hashes,
        db: &'a mut db::Writer,
        progress: &'a mut dyn Progress,
        pools: Vec<(String, usize)>,
    ) -> Self {
        let file_state = FileState::new(graph);
        Work {
            graph,
            db,
            file_state,
            last_hashes,
            build_states: BuildStates::new(progress, pools),
            runner: Runner::new(),
        }
    }

    pub fn want_file(&mut self, id: FileId) {
        self.build_states.want_file(self.graph, id)
    }

    /// Check whether a given build is ready, generally after one of its inputs
    /// has been updated.
    fn recheck_ready(&self, id: BuildId) -> bool {
        let build = self.graph.build(id);
        // println!("  recheck {:?} {}", id, build.location);
        for id in build.depend_ins() {
            let file = self.graph.file(id);
            match file.input {
                None => {
                    // Only generated inputs contribute to readiness.
                    continue;
                }
                Some(id) => {
                    if self.build_states.get(id) != BuildState::Done {
                        // println!("    {:?} {} not done", id, file.name);
                        return false;
                    }
                }
            }
        }
        // println!("{:?} now ready", id);
        true
    }

    /// Given a build that just finished, record its new deps and hash.
    fn record_finished(&mut self, fin: FinishedBuild) -> anyhow::Result<()> {
        let id = fin.id;
        let deps = match fin.deps {
            None => Vec::new(),
            Some(names) => names.iter().map(|name| self.graph.file_id(name)).collect(),
        };
        let deps_changed = self.graph.build_mut(id).update_deps(deps);
        let build = self.graph.build(id);

        // We may have discovered new deps, so ensure we have mtimes for those.
        if deps_changed {
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
    /// Prereq: any generated input is already generated.
    /// Non-generated inputs may not have been stat()ed yet.
    fn check_build_dirty(&mut self, id: BuildId) -> anyhow::Result<bool> {
        let build = self.graph.build(id);

        // Ensure all dependencies are in place.
        for id in build.depend_ins() {
            let file = self.graph.file(id);
            // stat any non-generated inputs if needed.
            // Generated inputs should already have their state gathered by
            // running them.
            let mtime = match self.file_state.get(id) {
                Some(mtime) => mtime,
                None => {
                    if file.input.is_none() {
                        self.file_state.restat(id, &file.name)?
                    } else {
                        panic!("expected file state for {} to be ready", file.name);
                    }
                }
            };
            // All inputs must be present.
            match mtime {
                MTime::Stamp(_) => {}
                MTime::Missing => {
                    anyhow::bail!("{}: input {} missing", build.location, file.name);
                }
            };
        }

        if build.cmdline.is_none() {
            // TODO: require the rule name 'phony'.
            // Phony build; mark the output "files" as present.
            for &id in build.outs() {
                if self.file_state.get(id).is_none() {
                    self.file_state.mark_present(id);
                }
            }
            return Ok(false);
        }

        let hash = hash_build(self.graph, &mut self.file_state, build)?;
        Ok(self.last_hashes.changed(id, hash))
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        while self.build_states.unfinished() {
            self.build_states.progress.tick(BuildState::Running);

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

            let fin = match self.runner.wait(Duration::from_millis(500)) {
                None => continue, // timeout
                Some(fin) => fin,
            };
            let id = fin.id;

            if !fin.success {
                self.build_states
                    .progress
                    .failed(self.graph.build(id), &fin.output);
                anyhow::bail!("build failed");
            }

            self.record_finished(fin)?;
            self.ready_dependents(id);
        }

        self.build_states.progress.tick(BuildState::Done);
        Ok(())
    }
}
