use std::hash::Hasher;
use std::os::unix::fs::MetadataExt;

use crate::parse::NString;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(u64);

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
        String::from("TODO")
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MTime {
    Missing,
    Stamp(u32),
}

#[derive(Clone, Debug)]
pub struct FileState {
    mtime: Option<MTime>,
    hash: Option<Hash>,
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

    fn file(&self, id: FileId) -> &FileState {
        &self.files[id.index()]
    }
    fn file_mut(&mut self, id: FileId) -> &mut FileState {
        &mut self.files[id.index()]
    }

    pub fn get_hash(&self, id: BuildId) -> Option<Hash> {
        self.builds[id.index()]
    }

    pub fn hash(&mut self, graph: &Graph, id: BuildId) -> std::io::Result<Hash> {
        let hash = match self.get_hash(id) {
            Some(hash) => hash,
            None => {
                let hash = self.do_hash(graph, id)?;
                self.builds[id.index()] = Some(hash);
                hash
            }
        };
        Ok(hash)
    }

    fn do_hash(&mut self, graph: &Graph, id: BuildId) -> std::io::Result<Hash> {
        let build = graph.build(id);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for &id in &build.ins {
            h.write(graph.file(id).name.as_nstr().as_bytes());
            let mtime = self.mtime(graph, id)?;
            let mtime_int = match mtime {
                MTime::Missing => 0,
                MTime::Stamp(t) => t + 1,
            };
            h.write_u32(mtime_int);
            h.write_u8(UNIT_SEPARATOR);
        }
        h.write(build.cmdline().as_bytes());
        Ok(Hash(h.finish()))
    }

    fn mtime(&mut self, graph: &Graph, id: FileId) -> std::io::Result<MTime> {
        if let Some(mtime) = self.file(id).mtime {
            return Ok(mtime);
        }
        let mtime = self.stat(graph, id)?;
        self.file_mut(id).mtime = Some(mtime);
        return Ok(mtime);
    }

    fn stat(&self, graph: &Graph, id: FileId) -> std::io::Result<MTime> {
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
        Ok(mtime)
    }
}

/*pub fn stat_recursive(graph: &Graph, state: &mut State, id: FileId) -> std::io::Result<()> {
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
*/
