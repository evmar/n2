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
    order_only_ins: usize,
    deps_ins: Vec<FileId>,
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
    pub fn depend_ins(&self) -> impl Iterator<Item = FileId> + '_ {
        self.ins.iter().chain(self.deps_ins.iter()).map(|id| *id)
    }
    pub fn order_only_ins(&self) -> &[FileId] {
        &self.ins[(self.explicit_ins + self.implicit_ins)..self.ins.len()]
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

#[derive(Clone, Debug)]
pub struct FileState {
    // used by downstream builds for computing their hash.
    pub mtime: MTime,
    // hash of input + mtime, used to tell if file is up to date.
    pub hash: Hash,
}

pub struct State {
    files: Vec<Option<FileState>>,
}

impl State {
    pub fn new(graph: &Graph) -> Self {
        let mut files = Vec::new();
        files.resize(graph.files.len(), None);
        State { files: files }
    }

    pub fn file(&self, id: FileId) -> Option<&FileState> {
        if id.index() >= self.files.len() {
            return None;
        }
        self.files[id.index()].as_ref()
    }
    pub fn set_state(&mut self, id: FileId, state: FileState) {
        if id.index() >= self.files.len() {
            self.files.resize(id.index() + 1, None);
        }
        self.files[id.index()] = Some(state)
    }

    pub fn hash_changed(&self, last_state: &State, id: FileId) -> bool {
        let hash = match self.file(id) {
            None => return true,
            Some(filestate) => filestate.hash,
        };
        let last_hash = match last_state.file(id) {
            None => return true,
            Some(filestate) => filestate.hash,
        };
        return hash != last_hash;
    }

    pub fn hash_outputs(&mut self, graph: &Graph, id: BuildId) -> std::io::Result<()> {
        let build = graph.build(id);
        let mut in_hash = std::collections::hash_map::DefaultHasher::new();
        for id in build.dirtying_ins() {
            in_hash.write(graph.file(id).name.as_bytes());
            let mtime = self.file(id).unwrap().mtime;
            let mtime_int = match mtime {
                MTime::Missing => 0,
                MTime::Stamp(t) => t + 1,
            };
            in_hash.write_u32(mtime_int);
            in_hash.write_u8(UNIT_SEPARATOR);
        }
        in_hash.write(build.cmdline.as_ref().map(|c| c.as_bytes()).unwrap_or(b""));
        in_hash.write_u8(UNIT_SEPARATOR);

        for &id in build.outs() {
            let file = graph.file(id);
            let mtime = stat(&file.name)?;
            let mtime_int = match mtime {
                MTime::Missing => 0,
                MTime::Stamp(t) => t + 1,
            };
            let mut hash = in_hash.clone();
            hash.write(file.name.as_bytes());
            hash.write_u32(mtime_int);
            self.set_state(
                id,
                FileState {
                    mtime: mtime,
                    hash: Hash(hash.finish()),
                },
            );
        }
        Ok(())
    }
}
