//! The build graph, a graph between files and commands.

use crate::canon::{canon_path, canon_path_in_place};
use crate::densemap::{self, DenseMap};
use std::collections::HashMap;
use std::hash::Hasher;
use std::os::unix::fs::MetadataExt;

/// Hash value used to identify a given instance of a Build's execution;
/// compared to verify whether a Build is up to date.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(pub u64);

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

/// A textual location within a build.ninja file, used in error messages.
#[derive(Debug)]
pub struct FileLoc {
    pub filename: std::rc::Rc<String>,
    pub line: usize,
}
impl std::fmt::Display for FileLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}:{}", self.filename, self.line)
    }
}

/// Input files to a Build.
pub struct BuildIns {
    /// Internally we stuff explicit/implicit/order-only ins all into one Vec.
    /// This is mostly to simplify some of the iteration and is a little more
    /// memory efficient than three separate Vecs, but it is kept internal to
    /// Build and only exposed via methods on Build.
    pub ids: Vec<FileId>,
    pub explicit: usize,
    pub implicit: usize,
    // order_only count implied by other counts.
    // pub order_only: usize,
}

/// Output files from a Build.
pub struct BuildOuts {
    /// Similar to ins, we keep both explicit and implicit outs in one Vec.
    pub ids: Vec<FileId>,
    pub explicit: usize,
}

/// A single build action, generating File outputs from File inputs with a command.
pub struct Build {
    /// Source location this Build was declared.
    pub location: FileLoc,

    /// User-provided description of the build step.
    pub desc: Option<String>,

    /// Command line to run.  Absent for phony builds.
    pub cmdline: Option<String>,

    /// Path to generated `.d` file, if any.
    pub depfile: Option<String>,

    /// Pool to execute this build in, if any.
    pub pool: Option<String>,

    pub ins: BuildIns,

    /// Additional inputs discovered from a previous build.
    discovered_ins: Vec<FileId>,

    /// Output files.
    pub outs: BuildOuts,
}
impl Build {
    pub fn new(loc: FileLoc, ins: BuildIns, outs: BuildOuts) -> Self {
        Build {
            location: loc,
            desc: None,
            cmdline: None,
            depfile: None,
            pool: None,
            ins,
            discovered_ins: Vec::new(),
            outs,
        }
    }

    /// Input paths that appear in `$in`.
    pub fn explicit_ins(&self) -> &[FileId] {
        &self.ins.ids[0..self.ins.explicit]
    }

    /// Input paths that, if changed, invalidate the output.
    /// Note this omits discovered_ins, which also invalidate the output.
    pub fn dirtying_ins(&self) -> &[FileId] {
        &self.ins.ids[0..(self.ins.explicit + self.ins.implicit)]
    }

    /// Order-only inputs: inputs that are only used for ordering execution.
    pub fn order_only_ins(&self) -> &[FileId] {
        &self.ins.ids[(self.ins.explicit + self.ins.implicit)..]
    }

    /// Inputs that are needed before building.
    /// Distinct from dirtying_ins in that it includes order-only dependencies.
    /// Note that we don't order on discovered_ins, because they're not allowed to
    /// affect build order.
    pub fn ordering_ins(&self) -> &[FileId] {
        &self.ins.ids
    }

    /// Potentially update discovered_ins with a new set of deps, returning true if they changed.
    pub fn update_discovered(&mut self, mut deps: Vec<FileId>) -> bool {
        // Filter out any deps that were already listed in the build file.
        deps.retain(|id| !self.ins.ids.contains(id));
        if deps == self.discovered_ins {
            false
        } else {
            self.set_discovered_ins(deps);
            true
        }
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

    pub fn debug_name(&self, graph: &Graph) -> String {
        format!(
            "{} ({}, ...)",
            self.location,
            graph.file(self.outs()[0]).name
        )
    }
}

/// The build graph: owns Files/Builds and maps FileIds/BuildIds to them,
/// as well as mapping string filenames to the underlying Files.
pub struct Graph {
    files: DenseMap<FileId, File>,
    pub builds: DenseMap<BuildId, Build>,
    file_to_id: HashMap<String, FileId>,
}

impl Graph {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Graph {
            files: DenseMap::new(),
            builds: DenseMap::new(),
            file_to_id: HashMap::new(),
        }
    }

    /// Add a new file, generating a new FileId for it.
    fn add_file(&mut self, name: String) -> FileId {
        self.files.push(File {
            name,
            input: None,
            dependents: Vec::new(),
        })
    }

    /// Look up a file by its FileId.
    pub fn file(&self, id: FileId) -> &File {
        self.files.get(id)
    }

    /// Canonicalize a path and get/generate its FileId.
    pub fn file_id(&mut self, canon: &mut String) -> FileId {
        canon_path_in_place(canon);
        match self.file_to_id.get(canon) {
            Some(id) => *id,
            None => {
                // TODO: so many string copies :<
                let id = self.add_file(canon.clone());
                self.file_to_id.insert(canon.clone(), id);
                id
            }
        }
    }

    /// Canonicalize a path and look up its FileId.
    pub fn lookup_file_id(&self, f: &str) -> Option<FileId> {
        let canon = canon_path(f);
        self.file_to_id.get(&canon).copied()
    }

    /// Add a new Build, generating a BuildId for it.
    pub fn add_build(&mut self, build: Build) {
        let id = self.builds.next_id();
        for &inf in &build.ins.ids {
            self.files.get_mut(inf).dependents.push(id);
        }
        for &out in &build.outs.ids {
            let f = self.files.get_mut(out);
            match f.input {
                Some(b) => panic!("double link {:?}", b),
                None => f.input = Some(id),
            }
        }
        self.builds.push(build);
    }

    /// Look up a Build by BuildId.
    pub fn build(&self, id: BuildId) -> &Build {
        &self.builds.get(id)
    }
    /// Look up a Build by BuildId.
    pub fn build_mut(&mut self, id: BuildId) -> &mut Build {
        self.builds.get_mut(id)
    }
}

/// MTime info gathered for a file.  This also models "file is absent".
/// It's not using an Option<> just because it makes the code using it easier
/// to follow.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MTime {
    Missing,
    Stamp(u32),
}

/// stat() an on-disk path, producing its MTime.
pub fn stat(path: &str) -> std::io::Result<MTime> {
    Ok(match std::fs::metadata(path) {
        Ok(meta) => MTime::Stamp(meta.mtime() as u32),
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
        FileState(DenseMap::new_sized(graph.files.next_id(), None))
    }

    pub fn get(&self, id: FileId) -> Option<MTime> {
        *self.0.lookup(id).unwrap_or(&None)
    }

    pub fn restat(&mut self, id: FileId, path: &str) -> std::io::Result<MTime> {
        let mtime = stat(path)?;
        self.0.set_grow(id, Some(mtime), None);
        Ok(mtime)
    }
}

const UNIT_SEPARATOR: u8 = 0x1F;

// Add a list of files to a hasher; used by hash_build.
fn hash_files(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    graph: &Graph,
    file_state: &mut FileState,
    ids: &[FileId],
) {
    for &id in ids {
        let mtime = file_state
            .get(id)
            .unwrap_or_else(|| panic!("no state for {:?}", graph.file(id).name));
        let mtime_int = match mtime {
            MTime::Missing => panic!("missing file {:?}", graph.file(id).name),
            MTime::Stamp(t) => t,
        };
        hasher.write(graph.file(id).name.as_bytes());
        hasher.write_u32(mtime_int);
        hasher.write_u8(UNIT_SEPARATOR);
    }
}

// Hashes the inputs of a build to compute a signature.
// Prerequisite: all referenced files have already been stat()ed and are present.
// (It doesn't make sense to hash a build with missing files, because it's out
// of date regardless of the state of the other files.)
pub fn hash_build(
    graph: &Graph,
    file_state: &mut FileState,
    build: &Build,
) -> std::io::Result<Hash> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_files(&mut hasher, graph, file_state, build.dirtying_ins());
    hasher.write_u8(UNIT_SEPARATOR);
    hash_files(&mut hasher, graph, file_state, build.discovered_ins());
    hasher.write_u8(UNIT_SEPARATOR);
    hasher.write(build.cmdline.as_ref().map(|c| c.as_bytes()).unwrap_or(b""));
    hasher.write_u8(UNIT_SEPARATOR);
    hash_files(&mut hasher, graph, file_state, build.outs());
    Ok(Hash(hasher.finish()))
}

pub struct Hashes(HashMap<BuildId, Hash>);

impl Hashes {
    pub fn new() -> Hashes {
        Hashes(HashMap::new())
    }

    pub fn set(&mut self, id: BuildId, hash: Hash) {
        self.0.insert(id, hash);
    }

    pub fn changed(&self, id: BuildId, hash: Hash) -> bool {
        let last_hash = match self.0.get(&id) {
            None => return true,
            Some(h) => *h,
        };
        hash != last_hash
    }
}
