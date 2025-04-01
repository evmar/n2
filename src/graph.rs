//! The build graph, a graph between files and commands.

use rustc_hash::FxHashMap;

use crate::{
    densemap::{self, DenseMap},
    hash::BuildHash,
};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{
    collections::{hash_map::Entry, HashMap},
    ops::{Deref, DerefMut},
};

/// Id for File nodes in the Graph.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct FileId(u32);
impl densemap::Index for FileId {
    fn index(&self) -> usize {
        self.0 as usize
    }
}
impl From<usize> for FileId {
    fn from(u: usize) -> FileId {
        FileId(u as u32)
    }
}

/// Id for Build nodes in the Graph.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct BuildId(u32);
impl densemap::Index for BuildId {
    fn index(&self) -> usize {
        self.0 as usize
    }
}
impl From<usize> for BuildId {
    fn from(u: usize) -> BuildId {
        BuildId(u as u32)
    }
}

/// A single file referenced as part of a build.
#[derive(Debug)]
pub struct File {
    /// Canonical path to the file.
    pub name: String,
    /// The Build that generates this file, if any.
    pub input: Option<BuildId>,
    /// The Builds that depend on this file as an input.
    pub dependents: Vec<BuildId>,
}

impl File {
    pub fn path(&self) -> &Path {
        Path::new(&self.name)
    }
}

/// A textual location within a build.ninja file, used in error messages.
#[derive(Debug)]
pub struct FileLoc {
    pub filename: std::rc::Rc<PathBuf>,
    pub line: usize,
}
impl std::fmt::Display for FileLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}:{}", self.filename.display(), self.line)
    }
}

#[derive(Debug, Clone, Hash)]
pub struct RspFile {
    pub path: std::path::PathBuf,
    pub content: String,
}

/// Input files to a Build.
#[derive(Clone)]
pub struct BuildIns {
    /// Internally we stuff explicit/implicit/order-only ins all into one Vec.
    /// This is mostly to simplify some of the iteration and is a little more
    /// memory efficient than three separate Vecs, but it is kept internal to
    /// Build and only exposed via methods on Build.
    pub ids: Vec<FileId>,
    pub explicit: usize,
    pub implicit: usize,
    pub order_only: usize,
    // validation count implied by other counts.
    // pub validation: usize,
}

/// Output files from a Build.
#[derive(Clone)]
pub struct BuildOuts {
    /// Similar to ins, we keep both explicit and implicit outs in one Vec.
    pub ids: Vec<FileId>,
    pub explicit: usize,
}

impl BuildOuts {
    /// CMake seems to generate build files with the same output mentioned
    /// multiple times in the outputs list.  Given that Ninja accepts these,
    /// this function removes duplicates from the output list.
    pub fn remove_duplicates(&mut self) {
        let mut ids = Vec::new();
        for (i, &id) in self.ids.iter().enumerate() {
            if self.ids[0..i].iter().any(|&prev| prev == id) {
                // Skip over duplicate.
                if i < self.explicit {
                    self.explicit -= 1;
                }
                continue;
            }
            ids.push(id);
        }
        self.ids = ids;
    }
}

#[cfg(test)]
mod tests {
    fn fileids(ids: Vec<usize>) -> Vec<FileId> {
        ids.into_iter().map(FileId::from).collect()
    }

    use super::*;
    #[test]
    fn remove_dups_explicit() {
        let mut outs = BuildOuts {
            ids: fileids(vec![1, 1, 2]),
            explicit: 2,
        };
        outs.remove_duplicates();
        assert_eq!(outs.ids, fileids(vec![1, 2]));
        assert_eq!(outs.explicit, 1);
    }

    #[test]
    fn remove_dups_implicit() {
        let mut outs = BuildOuts {
            ids: fileids(vec![1, 2, 1]),
            explicit: 2,
        };
        outs.remove_duplicates();
        assert_eq!(outs.ids, fileids(vec![1, 2]));
        assert_eq!(outs.explicit, 2);
    }
}

#[derive(Clone)]
pub struct BuildDependencies {
    /// Input files.
    pub ins: BuildIns,

    /// Additional inputs discovered from a previous build.
    discovered_ins: Vec<FileId>,

    /// Output files.
    pub outs: BuildOuts,
}

impl BuildDependencies {
    /// Input paths that appear in `$in`.
    pub fn explicit_ins(&self) -> &[FileId] {
        &self.ins.ids[0..self.ins.explicit]
    }

    /// Input paths that, if changed, invalidate the output.
    /// Note this omits discovered_ins, which also invalidate the output.
    pub fn dirtying_ins(&self) -> &[FileId] {
        &self.ins.ids[0..(self.ins.explicit + self.ins.implicit)]
    }

    /// Inputs that are needed before building.
    /// Distinct from dirtying_ins in that it includes order-only dependencies.
    /// Note that we don't order on discovered_ins, because they're not allowed to
    /// affect build order.
    pub fn ordering_ins(&self) -> &[FileId] {
        &self.ins.ids[0..(self.ins.order_only + self.ins.explicit + self.ins.implicit)]
    }

    /// Inputs that are needed before validating information.
    /// Validation inputs will be built whenever this Build is built, but this Build will not
    /// wait for them to complete before running. The validation inputs can fail to build, which
    /// will cause the overall build to fail.
    pub fn validation_ins(&self) -> &[FileId] {
        &self.ins.ids[(self.ins.order_only + self.ins.explicit + self.ins.implicit)..]
    }

    pub fn set_discovered_ins(&mut self, deps: Vec<FileId>) {
        self.discovered_ins = deps;
    }

    /// Input paths that were discovered after building, for use in the next build.
    pub fn discovered_ins(&self) -> &[FileId] {
        &self.discovered_ins
    }

    /// Output paths that appear in `$out`.
    pub fn explicit_outs(&self) -> &[FileId] {
        &self.outs.ids[0..self.outs.explicit]
    }

    /// Output paths that are updated when the build runs.
    pub fn outs(&self) -> &[FileId] {
        &self.outs.ids
    }
}

/// A single build action, generating File outputs from File inputs with a command.
pub struct Build {
    /// Source location this Build was declared.
    pub location: FileLoc,

    /// Inputs and outputs for this build.
    pub dependencies: BuildDependencies,

    /// User-provided description of the build step.
    pub desc: Option<String>,

    /// Command line to run.  Absent for phony builds.
    pub cmdline: Option<String>,

    /// Controls how dependency information is processed after compilation.
    pub deps: Option<String>,

    /// Path to generated `.d` file, if any.
    pub depfile: Option<String>,

    // Struct that contains the path to the rsp file and its contents, if any.
    pub rspfile: Option<RspFile>,

    /// Pool to execute this build in, if any.
    pub pool: Option<String>,

    /// True if output of command should be hidden on successful completion.
    pub hide_success: bool,
    /// True if last line of output should not be shown in status.
    pub hide_progress: bool,
}
impl Build {
    pub fn new(loc: FileLoc, ins: BuildIns, outs: BuildOuts) -> Self {
        Build {
            location: loc,
            dependencies: BuildDependencies {
                ins,
                discovered_ins: Vec::new(),
                outs,
            },
            desc: None,
            cmdline: None,
            deps: None,
            depfile: None,
            rspfile: None,
            pool: None,
            hide_success: false,
            hide_progress: false,
        }
    }

    /// If true, extract "/showIncludes" lines from output.
    pub fn parse_showincludes(&self) -> bool {
        match self.deps.as_deref() {
            Some("msvc") => true,
            _ => false,
        }
    }
}

impl Deref for Build {
    type Target = BuildDependencies;

    fn deref(&self) -> &Self::Target {
        &self.dependencies
    }
}

impl DerefMut for Build {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.dependencies
    }
}

/// The build graph: owns Files/Builds and maps FileIds/BuildIds to them.
#[derive(Default)]
pub struct Graph {
    pub builds: DenseMap<BuildId, Build>,
    pub files: GraphFiles,
}

/// Files identified by FileId, as well as mapping string filenames to them.
/// Split from Graph for lifetime reasons.
#[derive(Default)]
pub struct GraphFiles {
    pub by_id: DenseMap<FileId, File>,
    by_name: FxHashMap<String, FileId>,
}

impl Graph {
    /// Look up a file by its FileId.
    pub fn file(&self, id: FileId) -> &File {
        &self.files.by_id[id]
    }

    /// Add a new Build, generating a BuildId for it.
    pub fn add_build(&mut self, mut build: Build) -> anyhow::Result<()> {
        let new_id = self.builds.next_id();
        for &id in &build.ins.ids {
            self.files.by_id[id].dependents.push(new_id);
        }
        let mut fixup_dups = false;
        for &id in &build.outs.ids {
            let f = &mut self.files.by_id[id];
            match f.input {
                Some(prev) if prev == new_id => {
                    fixup_dups = true;
                    println!(
                        "n2: warn: {}: {:?} is repeated in output list",
                        build.location, f.name,
                    );
                }
                Some(prev) => {
                    anyhow::bail!(
                        "{}: {:?} is already an output at {}",
                        build.location,
                        f.name,
                        self.builds[prev].location
                    );
                }
                None => f.input = Some(new_id),
            }
        }
        if fixup_dups {
            build.outs.remove_duplicates();
        }
        self.builds.push(build);
        Ok(())
    }
}

impl GraphFiles {
    /// Look up a file by its name.  Name must have been canonicalized already.
    pub fn lookup(&self, file: &str) -> Option<FileId> {
        self.by_name.get(file).copied()
    }

    /// Look up a file by its name, adding it if not already present.
    /// Name must have been canonicalized already. Only accepting an owned
    /// string allows us to avoid a string copy and a hashmap lookup when we
    /// need to create a new id, but would also be possible to create a version
    /// of this function that accepts string references that is more optimized
    /// for the case where the entry already exists. But so far, all of our
    /// usages of this function have an owned string easily accessible anyways.
    pub fn id_from_canonical(&mut self, file: String) -> FileId {
        // TODO: so many string copies :<
        match self.by_name.entry(file) {
            Entry::Occupied(o) => *o.get(),
            Entry::Vacant(v) => {
                let id = self.by_id.push(File {
                    name: v.key().clone(),
                    input: None,
                    dependents: Vec::new(),
                });
                v.insert(id);
                id
            }
        }
    }

    pub fn all_ids(&self) -> impl Iterator<Item = FileId> {
        (0..self.by_id.next_id().0).map(|id| FileId(id))
    }
}

/// MTime info gathered for a file.  This also models "file is absent".
/// It's not using an Option<> just because it makes the code using it easier
/// to follow.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MTime {
    Missing,
    Stamp(SystemTime),
}

/// stat() an on-disk path, producing its MTime.
pub fn stat(path: &Path) -> std::io::Result<MTime> {
    // TODO: On Windows, use FindFirstFileEx()/FindNextFile() to get timestamps per
    //       directory, for better stat perf.
    Ok(match std::fs::metadata(path) {
        Ok(meta) => MTime::Stamp(meta.modified().unwrap()),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                MTime::Missing
            } else {
                return Err(err);
            }
        }
    })
}

/// Gathered state of on-disk files.
/// Due to discovered deps this map may grow after graph initialization.
pub struct FileState(DenseMap<FileId, Option<MTime>>);

impl FileState {
    pub fn new(graph: &Graph) -> Self {
        FileState(DenseMap::new_sized(graph.files.by_id.next_id(), None))
    }

    pub fn get(&self, id: FileId) -> Option<MTime> {
        self.0.lookup(id).copied().unwrap_or(None)
    }

    pub fn stat(&mut self, id: FileId, path: &Path) -> anyhow::Result<MTime> {
        let mtime = stat(path).map_err(|err| anyhow::anyhow!("stat {:?}: {}", path, err))?;
        self.0.set_grow(id, Some(mtime), None);
        Ok(mtime)
    }
}

#[derive(Default)]
pub struct Hashes(HashMap<BuildId, BuildHash>);

impl Hashes {
    pub fn set(&mut self, id: BuildId, hash: BuildHash) {
        self.0.insert(id, hash);
    }

    pub fn get(&self, id: BuildId) -> Option<BuildHash> {
        self.0.get(&id).copied()
    }
}

#[test]
fn stat_mtime_resolution() {
    use std::time::Duration;

    let temp_dir = tempfile::tempdir().unwrap();
    let filename = temp_dir.path().join("dummy");

    // Write once and stat.
    std::fs::write(&filename, "foo").unwrap();
    let mtime1 = match stat(&filename).unwrap() {
        MTime::Stamp(mtime) => mtime,
        _ => panic!("File not found: {}", filename.display()),
    };

    // Sleep for a short interval.
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Write twice and stat.
    std::fs::write(&filename, "foo").unwrap();
    let mtime2 = match stat(&filename).unwrap() {
        MTime::Stamp(mtime) => mtime,
        _ => panic!("File not found: {}", filename.display()),
    };

    let diff = mtime2.duration_since(mtime1).unwrap();
    assert!(diff > Duration::ZERO);
    assert!(diff < Duration::from_millis(100));
}
