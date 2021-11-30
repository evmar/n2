//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::graph::BuildId;
use crate::graph::FileId;
use crate::graph::Graph;
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;

/// Files are represented as integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Id(usize);

/// The loaded state of a database, as needed to make updates to the stored
/// state.  Other state is directly loaded into the build graph.
pub struct State {
    /// Maps db::Id to FileId.
    fileids: Vec<FileId>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<FileId, Id>,
}
impl State {
    pub fn new() -> Self {
        State {
            fileids: Vec::new(),
            db_ids: HashMap::new(),
        }
    }
}

struct WriteBuf {
    buf: [u8; 4096],
    len: usize,
}

impl WriteBuf {
    #[allow(deprecated)]
    fn new() -> Self {
        unsafe {
            WriteBuf {
                buf: std::mem::uninitialized(),
                len: 0,
            }
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.buf[self.len] = n;
        self.len += 1;
    }

    fn write_u16(&mut self, n: u16) {
        self.write_u8((n >> 8) as u8);
        self.write_u8((n & 0xFF) as u8);
    }

    fn write_str(&mut self, s: &str) {
        self.write_u16(s.len() as u16);
        self.buf[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
    }

    fn write_id(&mut self, id: Id) {
        let n = id.0 as u32;
        if n > (1 << 24) {
            panic!("too many fileids");
        }
        self.write_u8((n >> 16) as u8);
        self.write_u8((n >> 8) as u8);
        self.write_u8(n as u8);
    }

    fn flush<W: Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&self.buf[0..self.len])?;
        self.len = 0;
        Ok(())
    }
}

/// An opened database, ready for writes.
pub struct Writer {
    state: State,
    w: File,
}

impl Writer {
    fn new(state: State, w: File) -> Self {
        Writer { state: state, w: w }
    }

    fn write_file(&mut self, name: &str) -> std::io::Result<()> {
        if name.len() >= 0b1000_0000 {
            panic!("filename too long");
        }
        let mut buf = WriteBuf::new();
        buf.write_str(name);
        buf.flush(&mut self.w)
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> std::io::Result<Id> {
        let id = match self.state.db_ids.get(&fileid) {
            Some(&id) => id,
            None => {
                let id = Id(self.state.fileids.len());
                self.state.db_ids.insert(fileid, id);
                self.state.fileids.push(fileid);
                self.write_file(&graph.file(fileid).name)?;
                id
            }
        };
        Ok(id)
    }

    pub fn write_deps(
        &mut self,
        graph: &Graph,
        outs: &[FileId],
        deps: &[FileId],
    ) -> std::io::Result<()> {
        let mut buf = WriteBuf::new();
        let mark = (outs.len() as u16) | 0b1000_0000_0000_0000;
        buf.write_u16(mark);
        for &out in outs {
            let id = self.ensure_id(graph, out)?;
            buf.write_id(id);
        }

        buf.write_u16(deps.len() as u16);
        for &dep in deps {
            let id = self.ensure_id(graph, dep)?;
            buf.write_id(id);
        }

        buf.flush(&mut self.w)
    }
}

/// Provides lower-level methods for reading serialized data.
struct BReader<'a> {
    r: BufReader<&'a mut File>,
}
#[allow(deprecated)] // don't care about your fancy uninit API
impl<'a> BReader<'a> {
    fn read_u16(&mut self) -> std::io::Result<u16> {
        let mut buf: [u8; 2];
        unsafe {
            buf = std::mem::uninitialized();
            self.r.read_exact(&mut buf)?;
        }
        Ok(((buf[0] as u16) << 8) | (buf[1] as u16))
    }
    fn read_u24(&mut self) -> std::io::Result<u32> {
        let mut buf: [u8; 3];
        unsafe {
            buf = std::mem::uninitialized();
            self.r.read_exact(&mut buf)?;
        }
        Ok(((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32))
    }
    fn read_id(&mut self) -> std::io::Result<Id> {
        self.read_u24().map(|n| Id(n as usize))
    }
    fn read_str(&mut self, len: usize) -> std::io::Result<String> {
        // TODO: use uninit memory here
        let mut buf = Vec::new();
        buf.resize(len as usize, 0);
        self.r.read(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }
}

fn read(graph: &mut Graph, mut f: File) -> anyhow::Result<Writer> {
    let mut r = BReader {
        r: std::io::BufReader::new(&mut f),
    };
    let mut state = State::new();

    // Any given file may occur in the input multiple times, and we only want
    // to use the recorded state for the last one.  So for each file we see,
    // map it to the corresponding build and only store the most recent state
    // we have for it.

    let mut id_to_deps: HashMap<BuildId, Vec<FileId>> = HashMap::new();
    loop {
        let mut len = match r.read_u16() {
            Ok(r) => r,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => bail!(err),
        };
        let mask = 0b1000_0000_0000_0000;
        if len & mask == 0 {
            let name = r.read_str(len as usize)?;
            let fileid = graph.file_id(&name);
            state.db_ids.insert(fileid, Id(state.fileids.len()));
            state.fileids.push(fileid);
        } else {
            len = len & !mask;
            let mut bids = HashSet::new();
            for _ in 0..len {
                let id = r.read_id()?;
                if let Some(bid) = graph.file(state.fileids[id.0]).input {
                    bids.insert(bid);
                }
            }
            let len = r.read_u16()?;
            let mut deps = Vec::new();
            for _ in 0..len {
                let id = r.read_id()?;
                deps.push(state.fileids[id.0]);
            }
            if bids.len() == 1 {
                // Common case: only one associated build.
                let &id = bids.iter().next().unwrap();
                id_to_deps.insert(id, deps);
            } else {
                for &id in &bids {
                    id_to_deps.insert(id, deps.clone());
                }
            }
        }
    }

    for (id, deps) in id_to_deps {
        graph.build_mut(id).set_deps(deps);
    }

    Ok(Writer::new(state, f))
}

/// Opens an on-disk database, loading its state into the provided Graph.
pub fn open(graph: &mut Graph, path: &str) -> anyhow::Result<Writer> {
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Ok(f) => read(graph, f),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let f = std::fs::File::create(path)?;
            Ok(Writer::new(State::new(), f))
        }
        Err(err) => Err(anyhow!(err)),
    }
}
