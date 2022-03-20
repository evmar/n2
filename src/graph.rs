//! The build graph, a graph between files and commands.

use crate::canon::canon_path;
use crate::fs::{FileSystem, MTime};
use std::collections::HashMap;
use std::hash::Hasher;

/// Hash value used to identify a given instance of a Build's execution;
/// compared to verify whether a Build is up to date.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(pub u64);

/// Id for File nodes in the Graph.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct FileId(usize);
impl FileId {
    fn index(&self) -> usize {
        self.0
    }
}

/// Id for Build nodes in the Graph.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct BuildId(usize);
impl BuildId {
    pub fn index(&self) -> usize {
        self.0
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

/// A single build action, generating File outputs from File inputs with a command.
#[derive(Debug)]
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

    /// Input files.
    /// Internally we stuff explicit/implicit/order-only ins all into one Vec.
    /// This is mostly to simplify some of the iteration and is a little more
    /// memory efficient than three separate Vecs, but it is kept internal to
    /// Build and only exposed via different methods like .dirtying_ins() below.
    ins: Vec<FileId>,
    explicit_ins: usize,
    implicit_ins: usize,
    order_only_ins: usize,

    /// Additional inputs discovered from a previous build.
    discovered_ins: Vec<FileId>,

    /// Output files.
    /// Similar to ins, we keep both explicit and implicit outs in one Vec.
    outs: Vec<FileId>,
    explicit_outs: usize,
}
impl Build {
    pub fn new(loc: FileLoc) -> Self {
        Build {
            location: loc,
            desc: None,
            cmdline: None,
            depfile: None,
            pool: None,
            ins: Vec::new(),
            explicit_ins: 0,
            implicit_ins: 0,
            order_only_ins: 0,
            discovered_ins: Vec::new(),
            outs: Vec::new(),
            explicit_outs: 0,
        }
    }

    pub fn set_ins(&mut self, ins: Vec<FileId>, exp: usize, imp: usize, ord: usize) {
        self.ins = ins;
        self.explicit_ins = exp;
        self.implicit_ins = imp;
        self.order_only_ins = ord;
    }

    pub fn set_outs(&mut self, outs: Vec<FileId>, exp: usize) {
        self.outs = outs;
        self.explicit_outs = exp;
    }

    /// Input paths that appear in `$in`.
    pub fn explicit_ins(&self) -> &[FileId] {
        &self.ins[0..self.explicit_ins]
    }

    /// Input paths that, if changed, invalidate the output.
    /// Note this omits discovered_ins, which also invalidate the output.
    pub fn dirtying_ins(&self) -> &[FileId] {
        &self.ins[0..(self.explicit_ins + self.implicit_ins)]
    }

    /// Order-only inputs: inputs that are only used for ordering execution.
    pub fn order_only_ins(&self) -> &[FileId] {
        &self.ins[(self.explicit_ins + self.implicit_ins)..]
    }

    /// Inputs that are needed before building.
    /// Distinct from dirtying_ins in that it includes order-only dependencies.
    /// Note that we don't order on discovered_ins, because they're not allowed to
    /// affect build order.
    pub fn ordering_ins(&self) -> &[FileId] {
        &self.ins
    }

    /// Potentially update discovered_ins with a new set of deps, returning true if they changed.
    pub fn update_discovered(&mut self, mut deps: Vec<FileId>) -> bool {
        // Filter out any deps that were already listed in the build file.
        deps.retain(|id| !self.ins.contains(id));
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
        &self.outs[0..self.explicit_outs]
    }

    /// Output paths that are updated when the build runs.
    pub fn outs(&self) -> &[FileId] {
        &self.outs
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
    files: Vec<File>,
    builds: Vec<Build>,
    file_to_id: HashMap<String, FileId>,
}

impl Graph {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Graph {
            files: Vec::new(),
            builds: Vec::new(),
            file_to_id: HashMap::new(),
        }
    }

    /// Add a new file, generating a new FileId for it.
    fn add_file(&mut self, name: String) -> FileId {
        let id = self.files.len();
        self.files.push(File {
            name,
            input: None,
            dependents: Vec::new(),
        });
        FileId(id)
    }

    /// Look up a file by its FileId.
    pub fn file(&self, id: FileId) -> &File {
        &self.files[id.index()]
    }

    /// Canonicalize a path and get/generate its FileId.
    pub fn file_id(&mut self, f: impl Into<String>) -> FileId {
        let canon = canon_path(f);
        match self.file_to_id.get(&canon) {
            Some(id) => *id,
            None => {
                // TODO: so many string copies :<
                let id = self.add_file(canon.clone());
                self.file_to_id.insert(canon, id);
                id
            }
        }
    }

    /// Canonicalize a path and look up its FileId.
    pub fn get_file_id(&self, f: &str) -> Option<FileId> {
        let canon = canon_path(f);
        self.file_to_id.get(&canon).copied()
    }

    /// Add a new Build, generating a BuildId for it.
    pub fn add_build(&mut self, build: Build) {
        let id = BuildId(self.builds.len());
        for inf in &build.ins {
            self.files[inf.index()].dependents.push(id);
        }
        for out in &build.outs {
            let f = &mut self.files[out.index()];
            match f.input {
                Some(b) => panic!("double link {:?}", b),
                None => f.input = Some(id),
            }
        }
        self.builds.push(build);
    }

    /// Look up a Build by BuildId.
    pub fn build(&self, id: BuildId) -> &Build {
        &self.builds[id.index()]
    }
    /// Look up a Build by BuildId.
    pub fn build_mut(&mut self, id: BuildId) -> &mut Build {
        &mut self.builds[id.index()]
    }
}

/// Gathered state of on-disk files, indexed by FileId.
pub struct FileState(Vec<Option<MTime>>);

impl FileState {
    pub fn new(graph: &Graph) -> Self {
        let mut files = Vec::new();
        files.resize(graph.files.len(), None);
        FileState(files)
    }

    pub fn get(&self, id: FileId) -> Option<MTime> {
        if id.index() >= self.0.len() {
            return None;
        }
        self.0[id.index()]
    }

    fn set_mtime(&mut self, id: FileId, mtime: MTime) {
        // The set of files may grow after initialization time due to discovering deps after builds.
        if id.index() >= self.0.len() {
            self.0.resize(id.index() + 1, None);
        }
        self.0[id.index()] = Some(mtime)
    }

    pub fn restat(
        &mut self,
        fs: &dyn FileSystem,
        id: FileId,
        path: &str,
    ) -> std::io::Result<MTime> {
        let mtime = fs.stat(path)?;
        self.set_mtime(id, mtime);
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

pub struct Hashes(Vec<Option<Hash>>);

impl Hashes {
    pub fn new(graph: &Graph) -> Hashes {
        let mut v = Vec::new();
        v.resize(graph.builds.len(), None);
        Hashes(v)
    }

    pub fn set(&mut self, id: BuildId, hash: Hash) {
        self.0[id.index()] = Some(hash);
    }

    pub fn changed(&self, id: BuildId, hash: Hash) -> bool {
        let last_hash = match self.0[id.0] {
            None => return true,
            Some(h) => h,
        };
        hash != last_hash
    }
}
