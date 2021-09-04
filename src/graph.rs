use std::hash::Hasher;

use crate::parse::NString;

struct Hash(u64);

#[derive(Debug, Copy, Clone)]
pub struct FileId(usize);

pub struct File {
    name: NString,
    mtime: u64,
}

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
            h.write_u64(inf.mtime);
            h.write_u8(UNIT_SEPARATOR);
        }
        h.write(self.cmdline().as_bytes());
        Hash(h.finish())
    }
}

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
        self.files.push(File {
            name: name,
            mtime: 0,
        });
        FileId(self.files.len())
    }
    pub fn file(&mut self, id: FileId) -> &mut File {
        &mut self.files[id.0]
    }

    pub fn add_build(&mut self, build: Build) {
        self.builds.push(build);
    }
}
