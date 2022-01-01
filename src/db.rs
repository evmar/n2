//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::graph::BuildId;
use crate::graph::FileId;
use crate::graph::Graph;
use crate::graph::Hash;
use crate::graph::Hashes;
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
pub struct IdMap {
    /// Maps db::Id to FileId.
    fileids: Vec<FileId>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<FileId, Id>,
}
impl IdMap {
    pub fn new() -> Self {
        IdMap {
            fileids: Vec::new(),
            db_ids: HashMap::new(),
        }
    }
}

/// Buffer that accumulates a single record's worth of writes.
/// Caller calls various .write_*() methods and then flush()es it to a Write.
struct WriteBuf {
    buf: [u8; 4096],
    len: usize,
}

#[allow(clippy::erasing_op)]
#[allow(clippy::identity_op)]
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

    fn write_u16(&mut self, n: u16) {
        self.buf[self.len..(self.len + 2)]
            .copy_from_slice(&[((n >> (8 * 1)) & 0xFF) as u8, ((n >> (8 * 0)) & 0xFF) as u8]);
        self.len += 2;
    }

    fn write_u24(&mut self, n: u32) {
        self.buf[self.len..(self.len + 3)].copy_from_slice(&[
            ((n >> (8 * 2)) & 0xFF) as u8,
            ((n >> (8 * 1)) & 0xFF) as u8,
            ((n >> (8 * 0)) & 0xFF) as u8,
        ]);
        self.len += 3;
    }

    fn write_u64(&mut self, n: u64) {
        // Perf note: I tinkered with this in godbolt and using this form of
        // copy_from_slice generated much better code (generating a bswap
        // instruction!) than alternatives that did different kinds of indexing.
        self.buf[self.len..(self.len + 8)].copy_from_slice(&[
            ((n >> (8 * 7)) & 0xFF) as u8,
            ((n >> (8 * 6)) & 0xFF) as u8,
            ((n >> (8 * 5)) & 0xFF) as u8,
            ((n >> (8 * 4)) & 0xFF) as u8,
            ((n >> (8 * 3)) & 0xFF) as u8,
            ((n >> (8 * 2)) & 0xFF) as u8,
            ((n >> (8 * 1)) & 0xFF) as u8,
            ((n >> (8 * 0)) & 0xFF) as u8,
        ]);
        self.len += 8;
    }

    fn write_str(&mut self, s: &str) {
        self.write_u16(s.len() as u16);
        self.buf[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
    }

    fn write_id(&mut self, id: Id) {
        if id.0 > (1 << 24) {
            panic!("too many fileids");
        }
        self.write_u24(id.0 as u32);
    }

    fn flush<W: Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&self.buf[0..self.len])?;
        self.len = 0;
        Ok(())
    }
}

/// An opened database, ready for writes.
pub struct Writer {
    ids: IdMap,
    w: File,
}

impl Writer {
    fn new(ids: IdMap, w: File) -> Self {
        Writer { ids, w }
    }

    fn write_file(&mut self, name: &str) -> std::io::Result<()> {
        if name.len() >= 0b1000_0000_0000_0000 {
            panic!("filename too long");
        }
        let mut buf = WriteBuf::new();
        buf.write_str(name);
        buf.flush(&mut self.w)
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> std::io::Result<Id> {
        let id = match self.ids.db_ids.get(&fileid) {
            Some(&id) => id,
            None => {
                let id = Id(self.ids.fileids.len());
                self.ids.db_ids.insert(fileid, id);
                self.ids.fileids.push(fileid);
                self.write_file(&graph.file(fileid).name)?;
                id
            }
        };
        Ok(id)
    }

    pub fn write_build(&mut self, graph: &Graph, id: BuildId, hash: Hash) -> std::io::Result<()> {
        let build = graph.build(id);
        let mut buf = WriteBuf::new();
        let outs = build.outs();
        let mark = (outs.len() as u16) | 0b1000_0000_0000_0000;
        buf.write_u16(mark);
        for &out in outs {
            let id = self.ensure_id(graph, out)?;
            buf.write_id(id);
        }

        let deps = build.deps_ins();
        buf.write_u16(deps.len() as u16);
        for &dep in deps {
            let id = self.ensure_id(graph, dep)?;
            buf.write_id(id);
        }

        buf.write_u64(hash.0);

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

    #[allow(clippy::erasing_op)]
    #[allow(clippy::identity_op)]
    fn read_u64(&mut self) -> std::io::Result<u64> {
        let mut buf: [u8; 8];
        unsafe {
            buf = std::mem::uninitialized();
            self.r.read_exact(&mut buf)?;
        }
        Ok(((buf[0] as u64) << (8 * 7))
            | ((buf[1] as u64) << (8 * 6))
            | ((buf[2] as u64) << (8 * 5))
            | ((buf[3] as u64) << (8 * 4))
            | ((buf[4] as u64) << (8 * 3))
            | ((buf[5] as u64) << (8 * 2))
            | ((buf[6] as u64) << (8 * 1))
            | ((buf[7] as u64) << (8 * 0)))
    }
    fn read_id(&mut self) -> std::io::Result<Id> {
        self.read_u24().map(|n| Id(n as usize))
    }
    fn read_str(&mut self, len: usize) -> std::io::Result<String> {
        // TODO: use uninit memory here
        let mut buf = Vec::new();
        buf.resize(len as usize, 0);
        self.r.read_exact(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }
}

fn read(mut f: File, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Writer> {
    let mut r = BReader {
        r: std::io::BufReader::new(&mut f),
    };
    let mut ids = IdMap::new();

    loop {
        let mut len = match r.read_u16() {
            Ok(r) => r,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => bail!(err),
        };
        let mask = 0b1000_0000_0000_0000;
        if len & mask == 0 {
            let name = r.read_str(len as usize)?;
            let fileid = graph.file_id(name);
            let dbid = Id(ids.fileids.len());
            ids.db_ids.insert(fileid, dbid);
            ids.fileids.push(fileid);
        } else {
            len &= !mask;

            // Map each output to the associated build.
            // In the common case, there is only one.
            let mut bids = HashSet::new();
            for _ in 0..len {
                let id = r.read_id()?;
                if let Some(bid) = graph.file(ids.fileids[id.0]).input {
                    bids.insert(bid);
                }
            }

            let len = r.read_u16()?;
            let mut deps = Vec::new();
            for _ in 0..len {
                let id = r.read_id()?;
                deps.push(ids.fileids[id.0]);
            }

            let hash = Hash(r.read_u64()?);
            if bids.len() == 1 {
                // Common case: only one associated build.
                let &id = bids.iter().next().unwrap();
                graph.build_mut(id).set_deps(deps);
                hashes.set(id, hash);
            } else {
                // The graph layout has changed since this build was recorded.
                // The hashes won't line up anyway so it will be treated as dirty.
            }
        }
    }

    Ok(Writer::new(ids, f))
}

/// Opens an on-disk database, loading its state into the provided Graph.
pub fn open(path: &str, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Writer> {
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Ok(f) => read(f, graph, hashes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let f = std::fs::File::create(path)?;
            Ok(Writer::new(IdMap::new(), f))
        }
        Err(err) => Err(anyhow!(err)),
    }
}
