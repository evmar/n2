//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::densemap;
use crate::densemap::DenseMap;
use crate::graph::BuildId;
use crate::graph::FileId;
use crate::graph::Graph;
use crate::graph::Hash;
use crate::graph::Hashes;
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;

/// Files are identified by integers that are stable across n2 executions.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Id(u32);
impl densemap::Index for Id {
    fn index(&self) -> usize {
        self.0 as usize
    }
}
impl From<usize> for Id {
    fn from(u: usize) -> Id {
        Id(u as u32)
    }
}

/// The loaded state of a database, as needed to make updates to the stored
/// state.  Other state is directly loaded into the build graph.
#[derive(Default)]
pub struct IdMap {
    /// Maps db::Id to FileId.
    fileids: DenseMap<Id, FileId>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<FileId, Id>,
}

/// Buffer that accumulates a single record's worth of writes.
/// Caller calls various .write_*() methods and then flush()es it to a Write.
struct WriteBuf {
    buf: [u8; 16 << 10],
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
    fn create(path: &str) -> std::io::Result<Self> {
        let f = std::fs::File::create(path)?;
        Ok(Writer {
            ids: IdMap::default(),
            w: f,
        })
    }

    fn from_opened(ids: IdMap, w: File) -> Self {
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
                let id = self.ids.fileids.push(fileid);
                self.ids.db_ids.insert(fileid, id);
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

        let deps = build.discovered_ins();
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
impl<'a> BReader<'a> {
    fn read_u16(&mut self) -> std::io::Result<u16> {
        let mut arr: [u8; 2] = unsafe { std::mem::MaybeUninit::uninit().assume_init() };
        let buf: &mut [u8] = unsafe { std::mem::transmute(&mut arr[..]) };
        self.r.read_exact(buf)?;
        Ok(((buf[0] as u16) << 8) | (buf[1] as u16))
    }

    #[allow(clippy::erasing_op)]
    #[allow(clippy::identity_op)]
    fn read_u24(&mut self) -> std::io::Result<u32> {
        let mut arr: [u8; 3] = unsafe { std::mem::MaybeUninit::uninit().assume_init() };
        let buf: &mut [u8] = unsafe { std::mem::transmute(&mut arr[..]) };
        self.r.read_exact(buf)?;
        Ok(((buf[0] as u32) << (8 * 2))
            | ((buf[1] as u32) << (8 * 1))
            | ((buf[2] as u32) << (8 * 0)))
    }

    #[allow(clippy::erasing_op)]
    #[allow(clippy::identity_op)]
    fn read_u64(&mut self) -> std::io::Result<u64> {
        let mut arr: [u8; 8] = unsafe { std::mem::MaybeUninit::uninit().assume_init() };
        let buf: &mut [u8] = unsafe { std::mem::transmute(&mut arr[..]) };
        self.r.read_exact(buf)?;
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
        self.read_u24().map(|n| Id(n as u32))
    }

    fn read_str(&mut self, len: usize) -> std::io::Result<String> {
        let mut buf = Vec::with_capacity(len);
        // Safety: buf contents are uninitialized here, but we never read them
        // before initialization.
        unsafe { buf.set_len(len) };
        self.r.read_exact(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }
}

struct Reader<'a> {
    r: BReader<'a>,
    ids: IdMap,
    graph: &'a mut Graph,
    hashes: &'a mut Hashes,
}

impl<'a> Reader<'a> {
    fn read_path(&mut self, len: usize) -> std::io::Result<()> {
        let mut name = self.r.read_str(len)?;
        let fileid = self.graph.file_id(&mut name);
        let dbid = self.ids.fileids.push(fileid);
        self.ids.db_ids.insert(fileid, dbid);
        Ok(())
    }

    fn read_build(&mut self, len: usize) -> std::io::Result<()> {
        // This record logs a build.  We expect all the outputs to be
        // outputs of the same build id; if not, that means the graph has
        // changed since this log, in which case we just ignore it.
        //
        // It's possible we log a build that generates files A B, then
        // change the build file such that it only generates file A; this
        // logic will still attach the old dependencies to A, but it
        // shouldn't matter because the changed command line will cause us
        // to rebuild A regardless, and these dependencies are only used
        // to affect dirty checking, not build order.

        let mut unique_bid = None;
        let mut obsolete = false;
        for _ in 0..len {
            let fileid = self.r.read_id()?;
            if obsolete {
                // Even though we know we don't want this record, we must
                // keep reading to parse through it.
                continue;
            }
            match self.graph.file(*self.ids.fileids.get(fileid)).input {
                None => {
                    obsolete = true;
                }
                Some(bid) => {
                    match unique_bid {
                        None => unique_bid = Some(bid),
                        Some(unique_bid) if unique_bid == bid => {
                            // Ok, matches the existing id.
                        }
                        Some(_) => {
                            // Mismatch.
                            unique_bid = None;
                            obsolete = true;
                        }
                    }
                }
            }
        }

        let len = self.r.read_u16()?;
        let mut deps = Vec::new();
        for _ in 0..len {
            let id = self.r.read_id()?;
            deps.push(*self.ids.fileids.get(id));
        }

        let hash = Hash(self.r.read_u64()?);

        // unique_bid is set here if this record is valid.
        if let Some(id) = unique_bid {
            // Common case: only one associated build.
            self.graph.build_mut(id).set_discovered_ins(deps);
            self.hashes.set(id, hash);
        }
        Ok(())
    }

    fn read_file(&mut self) -> anyhow::Result<()> {
        loop {
            let mut len = match self.r.read_u16() {
                Ok(r) => r,
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(err) => bail!(err),
            };
            let mask = 0b1000_0000_0000_0000;
            if len & mask == 0 {
                self.read_path(len as usize)?;
            } else {
                len &= !mask;
                self.read_build(len as usize)?;
            }
        }
        Ok(())
    }

    /// Reads an on-disk database, loading its state into the provided Graph/Hashes.
    fn read(f: &mut File, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<IdMap> {
        let mut r = Reader {
            r: BReader {
                r: std::io::BufReader::new(f),
            },
            ids: IdMap::default(),
            graph,
            hashes,
        };
        r.read_file()?;

        Ok(r.ids)
    }
}

/// Opens or creates an on-disk database, loading its state into the provided Graph.
pub fn open(path: &str, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Writer> {
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Ok(mut f) => {
            let ids = Reader::read(&mut f, graph, hashes)?;
            Ok(Writer::from_opened(ids, f))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let w = Writer::create(path)?;
            Ok(w)
        }
        Err(err) => Err(anyhow!(err)),
    }
}
