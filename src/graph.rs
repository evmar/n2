use std::hash::Hasher;
use std::os::unix::fs::MetadataExt;

use crate::parse::NString;

#[derive(Debug, Copy, Clone)]
struct Hash(u64);

#[derive(Debug, Copy, Clone)]
pub struct FileId(usize);
impl FileId {
    fn index(&self) -> usize {
        self.0
    }
}

#[derive(Debug, Copy, Clone)]
pub struct BuildId(usize);
impl BuildId {
    fn index(&self) -> usize {
        self.0
    }
}

#[derive(Debug)]
pub struct File {
    pub name: NString,
    pub input: Option<BuildId>,
}

#[derive(Debug)]
pub struct Build {
    pub ins: Vec<FileId>,
    pub outs: Vec<FileId>,
}

const UNIT_SEPARATOR: u8 = 0x1F;

impl Build {
    fn cmdline(&self) -> String {
        String::new()
    }
    fn hash(&self, g: &Graph) -> Hash {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for id in &self.ins {
            let inf = &g.files[id.0];
            h.write(inf.name.as_bytes());
            // XXX h.write_u64(inf.mtime);
            h.write_u8(UNIT_SEPARATOR);
        }
        h.write(self.cmdline().as_bytes());
        Hash(h.finish())
    }
}

#[derive(Debug)]
pub struct Graph {
    files: Vec<File>,
    builds: Vec<Build>,
}

impl Graph {
    pub fn new() -> Graph {
        Graph {
            files: Vec::new(),
            builds: Vec::new(),
        }
    }

    pub fn add_file(&mut self, name: NString) -> FileId {
        let id = self.files.len();
        self.files.push(File {
            name: name,
            input: None,
        });
        FileId(id)
    }
    pub fn file(&self, id: FileId) -> &File {
        &self.files[id.index()]
    }

    pub fn add_build(&mut self, build: Build) {
        let id = BuildId(self.builds.len());
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum MTime {
    Unknown,
    Missing,
    Stamp(u32),
}

#[derive(Clone, Debug)]
pub struct FileState {
    mtime: MTime,
    hash: Option<Hash>,
}
impl FileState {
    fn empty() -> FileState {
        FileState {
            mtime: MTime::Unknown,
            hash: None,
        }
    }
}

pub struct State {
    files: Vec<FileState>,
}

impl State {
    pub fn new(graph: &Graph) -> Self {
        let mut files = Vec::new();
        files.resize(graph.files.len(), FileState::empty());
        State { files: files }
    }
    fn file(&self, id: FileId) -> &FileState {
        &self.files[id.index()]
    }
    fn file_mut(&mut self, id: FileId) -> &mut FileState {
        &mut self.files[id.index()]
    }
    fn stat(&mut self, graph: &Graph, id: FileId) -> std::io::Result<()> {
        let name = &graph.file(id).name;
        // Consider: mtime_nsec(?)
        let mtime = match std::fs::metadata(name.as_nstr().as_path()) {
            Ok(meta) => MTime::Stamp(meta.mtime() as u32),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    MTime::Missing
                } else {
                    return Err(err);
                }
            }
        };
        self.file_mut(id).mtime = mtime;
        Ok(())
    }
}

pub fn stat_recursive(graph: &Graph, state: &mut State, id: FileId) -> std::io::Result<()> {
    if state.file(id).mtime != MTime::Unknown {
        return Ok(());
    }
    state.stat(&graph, id)?;

    if let Some(bid) = graph.file(id).input {
        for fin in &graph.build(bid).ins {
            stat_recursive(graph, state, *fin)?;
        }
    }

    Ok(())
}
