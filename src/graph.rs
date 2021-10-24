//! The build graph, a graph between files and commands.

use crate::canon::canon_path;
use std::collections::HashMap;
use std::hash::Hasher;
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(u64);

impl Hash {
    pub fn todo() -> Self {
        Hash(0)
    }
}

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
    fn index(&self) -> usize {
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
    ins: Vec<FileId>,
    explicit_ins: usize,
    implicit_ins: usize,
    deps_ins: usize,
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
            deps_ins: 0,
            outs: Vec::new(),
            explicit_outs: 0,
        }
    }
    pub fn set_ins(&mut self, ins: Vec<FileId>, exp: usize, imp: usize) {
        self.ins = ins;
        self.explicit_ins = exp;
        self.implicit_ins = imp;
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
    pub fn dirtying_ins(&self) -> &[FileId] {
        &self.ins[0..(self.explicit_ins + self.implicit_ins + self.deps_ins)]
    }
    /// Input paths that are needed before building.
    pub fn depend_ins(&self) -> &[FileId] {
        // TODO order-only
        &self.ins[0..(self.explicit_ins + self.implicit_ins + self.deps_ins)]
    }
    /// Potentially update deps with a new set of deps, returning true if they
    /// changed.
    pub fn update_deps(&mut self, mut deps: Vec<FileId>) -> bool {
        // Filter out any deps that were explicitly listed in the build file.
        // This is only a minor optimization.
        let to_filter = &self.ins[0..(self.explicit_ins + self.implicit_ins)];
        deps.retain(|id| !to_filter.contains(id));

        if deps == self.deps_ins() {
            return false;
        }
        self.set_deps(&deps);
        return true;
    }
    pub fn set_deps(&mut self, deps: &[FileId]) {
        if self.deps_ins > 0 {
            // TODO move order-only deps back
            self.ins.truncate(self.ins.len() - self.deps_ins);
            self.deps_ins = 0;
        }
        // TODO move order-only deps fwd
        for &dep in deps {
            self.ins.push(dep);
            self.deps_ins += 1;
        }
    }
    /// Input paths that were discovered after building, for use in the next build.
    pub fn deps_ins(&self) -> &[FileId] {
        &self.ins[(self.explicit_ins + self.implicit_ins)
            ..(self.explicit_ins + self.implicit_ins + self.deps_ins)]
    }
    /// Output paths that appear in `$in`.
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
    pub fn file_id(&mut self, f: &str) -> FileId {
        // TODO: so many string copies :<
        let canon = canon_path(f);
        match self.file_to_id.get(&canon) {
            Some(id) => *id,
            None => {
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

#[derive(Clone, Debug)]
pub struct FileState {
    // used by downstream builds for computing their hash.
    pub mtime: Option<MTime>,
    // hash of input + mtime, used to tell if file is up to date.
    pub hash: Option<Hash>,
}
impl FileState {
    fn empty() -> FileState {
        FileState {
            mtime: None,
            hash: None,
        }
    }
}

pub struct State {
    files: Vec<FileState>,
    builds: Vec<Option<Hash>>,
}

impl State {
    pub fn new(graph: &Graph) -> Self {
        let mut files = Vec::new();
        files.resize(graph.files.len(), FileState::empty());
        let mut builds = Vec::new();
        builds.resize(graph.builds.len(), None);
        State {
            files: files,
            builds: builds,
        }
    }

    pub fn file(&self, id: FileId) -> &FileState {
        &self.files[id.index()]
    }
    pub fn file_mut(&mut self, id: FileId) -> &mut FileState {
        &mut self.files[id.index()]
    }

    pub fn get_hash(&self, id: BuildId) -> Option<Hash> {
        self.builds[id.index()]
    }

    pub fn hash(&mut self, graph: &Graph, id: BuildId) -> Hash {
        match self.get_hash(id) {
            Some(hash) => hash,
            None => {
                let hash = self.do_hash(graph, id);
                self.builds[id.index()] = Some(hash);
                hash
            }
        }
    }

    fn do_hash(&mut self, graph: &Graph, id: BuildId) -> Hash {
        let build = graph.build(id);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for &id in build.dirtying_ins() {
            h.write(graph.file(id).name.as_bytes());
            let mtime = self.file(id).mtime.unwrap();
            let mtime_int = match mtime {
                MTime::Missing => 0,
                MTime::Stamp(t) => t + 1,
            };
            h.write_u32(mtime_int);
            h.write_u8(UNIT_SEPARATOR);
        }
        h.write(build.cmdline.as_ref().map(|c| c.as_bytes()).unwrap_or(b""));
        Hash(h.finish())
    }

    pub fn stat(&mut self, graph: &Graph, id: FileId) -> std::io::Result<MTime> {
        if let Some(mtime) = self.file(id).mtime {
            return Ok(mtime);
        }
        let name = &graph.file(id).name;
        // TODO: consider mtime_nsec(?)
        let mtime = match std::fs::metadata(name) {
            Ok(meta) => MTime::Stamp(meta.mtime() as u32),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    MTime::Missing
                } else {
                    return Err(err);
                }
            }
        };
        self.file_mut(id).mtime = Some(mtime);
        Ok(mtime)
    }
}
