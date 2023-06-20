//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::{
    densemap, densemap::DenseMap, graph::BuildId, graph::FileId, graph::Graph, graph::Hashes,
    hash::BuildHash,
};
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::mem::MaybeUninit;

const VERSION: u32 = 1;

/// Files are identified by integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy)]
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
/// We use this instead of a BufWrite because we want to write one full record
/// at a time if possible.
struct WriteBuf {
    buf: [MaybeUninit<u8>; 16 << 10],
    len: usize,
}

impl WriteBuf {
    fn new() -> Self {
        WriteBuf {
            buf: unsafe { MaybeUninit::uninit().assume_init() },
            len: 0,
        }
    }

    // Perf note: I tinkered with these writes in godbolt and using
    // copy_from_slice generated better code than alternatives that did
    // different kinds of indexing.

    fn write(&mut self, buf: &[u8]) {
        // Safety: self.buf and buf are non-overlapping; bounds checks.
        unsafe {
            let ptr = self.buf.as_mut_ptr().add(self.len);
            self.len += buf.len();
            if self.len > self.buf.len() {
                panic!("oversized WriteBuf");
            }
            std::ptr::copy_nonoverlapping(buf.as_ptr(), ptr as *mut u8, buf.len());
        }
    }

    fn write_u16(&mut self, n: u16) {
        self.write(&n.to_le_bytes());
    }

    fn write_u24(&mut self, n: u32) {
        self.write(&n.to_le_bytes()[..3]);
    }

    fn write_u64(&mut self, n: u64) {
        self.write(&n.to_le_bytes());
    }

    fn write_str(&mut self, s: &str) {
        self.write_u16(s.len() as u16);
        self.write(s.as_bytes());
    }

    fn write_id(&mut self, id: Id) {
        if id.0 > (1 << 24) {
            panic!("too many fileids");
        }
        self.write_u24(id.0);
    }

    fn flush<W: Write>(self, w: &mut W) -> std::io::Result<()> {
        // Safety: invariant is that self.buf up to self.len is initialized.
        let buf: &[u8] = unsafe { std::mem::transmute(&self.buf[..self.len]) };
        w.write_all(buf)?;
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
        let mut w = Writer {
            ids: IdMap::default(),
            w: f,
        };
        w.write_signature()?;
        Ok(w)
    }

    fn from_opened(ids: IdMap, w: File) -> Self {
        Writer { ids, w }
    }

    fn write_signature(&mut self) -> std::io::Result<()> {
        write!(&mut self.w, "n2db")?;
        self.w.write_all(&u32::to_le_bytes(VERSION))
    }

    fn write_path(&mut self, name: &str) -> std::io::Result<()> {
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
                self.write_path(&graph.file(fileid).name)?;
                id
            }
        };
        Ok(id)
    }

    pub fn write_build(
        &mut self,
        graph: &Graph,
        id: BuildId,
        hash: BuildHash,
    ) -> std::io::Result<()> {
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

struct Reader<'a> {
    r: BufReader<&'a mut File>,
    ids: IdMap,
    graph: &'a mut Graph,
    hashes: &'a mut Hashes,
}

impl<'a> Reader<'a> {
    fn read_u16(&mut self) -> std::io::Result<u16> {
        let mut buf: [u8; 2] = [0; 2];
        self.r.read_exact(&mut buf[..])?;
        Ok(u16::from_le_bytes(buf))
    }

    fn read_u24(&mut self) -> std::io::Result<u32> {
        let mut buf: [u8; 4] = [0; 4];
        self.r.read_exact(&mut buf[..3])?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_u64(&mut self) -> std::io::Result<u64> {
        let mut buf: [u8; 8] = [0; 8];
        self.r.read_exact(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn read_id(&mut self) -> std::io::Result<Id> {
        self.read_u24().map(Id)
    }

    fn read_str(&mut self, len: usize) -> std::io::Result<String> {
        let mut buf = vec![0; len];
        self.r.read_exact(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }

    fn read_path(&mut self, len: usize) -> std::io::Result<()> {
        let name = self.read_str(len)?;
        // No canonicalization needed, paths were written canonicalized.
        let fileid = self.graph.file_id(name);
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
            let fileid = self.read_id()?;
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

        let len = self.read_u16()?;
        let mut deps = Vec::new();
        for _ in 0..len {
            let id = self.read_id()?;
            deps.push(*self.ids.fileids.get(id));
        }

        let hash = BuildHash(self.read_u64()?);

        // unique_bid is set here if this record is valid.
        if let Some(id) = unique_bid {
            // Common case: only one associated build.
            self.graph.build_mut(id).set_discovered_ins(deps);
            self.hashes.set(id, hash);
        }
        Ok(())
    }

    fn read_signature(&mut self) -> anyhow::Result<()> {
        let mut buf: [u8; 4] = [0; 4];
        self.r.read_exact(&mut buf[..])?;
        if buf.as_slice() != "n2db".as_bytes() {
            bail!("invalid db signature");
        }
        self.r.read_exact(&mut buf[..])?;
        let version = u32::from_le_bytes(buf);
        if version != VERSION {
            bail!("db version mismatch: got {version}, expected {VERSION}; TODO: db upgrades etc");
        }
        Ok(())
    }

    fn read_file(&mut self) -> anyhow::Result<()> {
        self.read_signature()?;
        loop {
            let mut len = match self.read_u16() {
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
            r: std::io::BufReader::new(f),
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
