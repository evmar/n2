//! The build graph, a graph between files and commands.

use crate::canon::canon_path;
use std::collections::HashMap;
use std::hash::Hasher;
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(pub u64);

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct FileId(usize);
impl FileId {
    fn index(&self) -> usize {
        self.0
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct BuildId(usize);
impl BuildId {
    pub fn index(&self) -> usize {
        self.0
    }
}

#[derive(Debug)]
pub struct File {
    pub name: String,
    pub input: Option<BuildId>,
    pub dependents: Vec<BuildId>,
}

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

#[derive(Debug)]
pub struct Build {
    pub location: FileLoc,
    pub cmdline: Option<String>,
    pub depfile: Option<String>,

    /// Internally we stuff explicit/implicit/order-only ins all into one Vec.
    /// This is mostly to simplify some of the iteration and is a little more
    /// memory efficient than three separate Vecs, but it is kept internal to
    /// Build and only exposed via different methods like .dirtying_ins() below.
    ins: Vec<FileId>,
    explicit_ins: usize,
    implicit_ins: usize,
    order_only_ins: usize,

    deps_ins: Vec<FileId>,

    /// Similar to ins, we keep both explicit and implicit outs in one Vec.
    outs: Vec<FileId>,
    explicit_outs: usize,
}
impl Build {
    pub fn new(loc: FileLoc) -> Self {
        Build {
            location: loc,
            cmdline: None,
            depfile: None,
            ins: Vec::new(),
            explicit_ins: 0,
            implicit_ins: 0,
            order_only_ins: 0,
            deps_ins: Vec::new(),
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
    pub fn dirtying_ins(&self) -> impl Iterator<Item = FileId> + '_ {
        self.ins[0..(self.explicit_ins + self.implicit_ins)]
            .iter()
            .chain(self.deps_ins.iter())
            .map(|id| *id)
    }
    /// Inputs that are needed before building.
    /// Distinct from dirtying_ins in that it includes order-only dependencies.
    pub fn depend_ins(&self) -> impl Iterator<Item = FileId> + '_ {
        self.ins.iter().chain(self.deps_ins.iter()).map(|id| *id)
    }
    /// Potentially update deps with a new set of deps, returning true if they changed.
    pub fn update_deps(&mut self, mut deps: Vec<FileId>) -> bool {
        // Filter out any deps that were already listed in the build file.
        deps.retain(|id| !self.ins.contains(id));
        if deps == self.deps_ins {
            return false;
        }
        self.set_deps(deps);
        return true;
    }
    pub fn set_deps(&mut self, deps: Vec<FileId>) {
        self.deps_ins = deps;
    }
    /// Input paths that were discovered after building, for use in the next build.
    pub fn deps_ins(&self) -> &[FileId] {
        &self.deps_ins
    }
    /// Output paths that appear in `$out`.
    pub fn explicit_outs(&self) -> &[FileId] {
        &self.outs[0..self.explicit_outs]
    }
    /// Output paths that are updated when the build runs.
    pub fn outs(&self) -> &[FileId] {
        &self.outs
    }
}

const UNIT_SEPARATOR: u8 = 0x1F;

pub struct Graph {
    files: Vec<File>,
    builds: Vec<Build>,
    file_to_id: HashMap<String, FileId>,
}

impl Graph {
    pub fn new() -> Graph {
        Graph {
            files: Vec::new(),
            builds: Vec::new(),
            file_to_id: HashMap::new(),
        }
    }

    fn add_file(&mut self, name: String) -> FileId {
        let id = self.files.len();
        self.files.push(File {
            name: name,
            input: None,
            dependents: Vec::new(),
        });
        FileId(id)
    }
    pub fn file(&self, id: FileId) -> &File {
        &self.files[id.index()]
    }
    pub fn file_id<F: Into<String>>(&mut self, f: F) -> FileId {
        let canon = canon_path(f);
        match self.file_to_id.get(&canon) {
            Some(id) => *id,
            None => {
                // TODO: so many string copies :<
                let id = self.add_file(canon.clone());
                self.file_to_id.insert(canon, id.clone());
                id
            }
        }
    }

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
    pub fn build(&self, id: BuildId) -> &Build {
        &self.builds[id.index()]
    }
    pub fn build_mut(&mut self, id: BuildId) -> &mut Build {
        &mut self.builds[id.index()]
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MTime {
    Missing,
    Stamp(u32),
}

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

    pub fn restat(&mut self, id: FileId, path: &str) -> std::io::Result<MTime> {
        let mtime = stat(path)?;
        self.set_mtime(id, mtime);
        Ok(mtime)
    }
}

pub fn hash_build(graph: &Graph, file_state: &mut FileState, id: BuildId) -> std::io::Result<Hash> {
    let build = graph.build(id);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for id in build.dirtying_ins() {
        hasher.write(graph.file(id).name.as_bytes());
        let mtime = file_state
            .get(id)
            .unwrap_or_else(|| panic!("no state for {:?}", graph.file(id).name));
        let mtime_int = match mtime {
            MTime::Missing => 0,
            MTime::Stamp(t) => t + 1,
        };
        hasher.write_u32(mtime_int);
        hasher.write_u8(UNIT_SEPARATOR);
    }
    hasher.write(build.cmdline.as_ref().map(|c| c.as_bytes()).unwrap_or(b""));
    hasher.write_u8(UNIT_SEPARATOR);

    for &id in build.outs() {
        let file = graph.file(id);
        let mtime = file_state.restat(id, &file.name)?;
        let mtime_int = match mtime {
            MTime::Missing => 0,
            MTime::Stamp(t) => t + 1,
        };
        hasher.write(file.name.as_bytes());
        hasher.write_u32(mtime_int);
    }

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
        return hash != last_hash;
    }
}
