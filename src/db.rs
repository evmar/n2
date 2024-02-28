//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::graph;
use crate::{
    densemap, densemap::DenseMap, graph::BuildId, graph::Graph, graph::Hashes, hash::BuildHash,
};
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

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
    fileids: DenseMap<Id, Arc<graph::File>>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<*const graph::File, Id>,
}

/// RecordWriter buffers writes into a Vec<u8>.
/// We attempt to write a full record per underlying finish() to lessen the chance of writing partial records.
#[derive(Default)]
struct RecordWriter(Vec<u8>);

impl RecordWriter {
    fn write(&mut self, buf: &[u8]) {
        self.0.extend_from_slice(buf);
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

    fn finish(&self, w: &mut impl Write) -> std::io::Result<()> {
        w.write_all(&self.0)
    }
}

/// An opened database, ready for writes.
pub struct Writer {
    ids: IdMap,
    w: File,
}

impl Writer {
    fn create(path: &Path) -> std::io::Result<Self> {
        let f = std::fs::File::create(path)?;
        let mut w = Self::from_opened(IdMap::default(), f);
        w.write_signature()?;
        Ok(w)
    }

    fn from_opened(ids: IdMap, w: File) -> Self {
        Writer { ids, w }
    }

    fn write_signature(&mut self) -> std::io::Result<()> {
        self.w.write_all("n2db".as_bytes())?;
        self.w.write_all(&u32::to_le_bytes(VERSION))
    }

    fn write_path(&mut self, name: &str) -> std::io::Result<()> {
        if name.len() >= 0b1000_0000_0000_0000 {
            panic!("filename too long");
        }
        let mut w = RecordWriter::default();
        w.write_str(&name);
        w.finish(&mut self.w)
    }

    fn ensure_id(&mut self, file: Arc<graph::File>) -> std::io::Result<Id> {
        let id = match self.ids.db_ids.get(&(file.as_ref() as *const graph::File)) {
            Some(&id) => id,
            None => {
                let id = self.ids.fileids.push(file.clone());
                self.ids
                    .db_ids
                    .insert(file.as_ref() as *const graph::File, id);
                self.write_path(&file.name)?;
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
        let build = &graph.builds[id];
        let mut w = RecordWriter::default();
        let outs = build.outs();
        let mark = (outs.len() as u16) | 0b1000_0000_0000_0000;
        w.write_u16(mark);
        for out in outs {
            let id = self.ensure_id(out.clone())?;
            w.write_id(id);
        }

        let deps = build.discovered_ins();
        w.write_u16(deps.len() as u16);
        for dep in deps {
            let id = self.ensure_id(dep.clone())?;
            w.write_id(id);
        }

        w.write_u64(hash.0);
        w.finish(&mut self.w)
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
        let file = self.graph.files.id_from_canonical(name);
        let dbid = self.ids.fileids.push(file.clone());
        self.ids
            .db_ids
            .insert(file.as_ref() as *const graph::File, dbid);
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
            match *self.ids.fileids[fileid].input.lock().unwrap() {
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
        let mut deps = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let id = self.read_id()?;
            deps.push(self.ids.fileids[id].clone());
        }

        let hash = BuildHash(self.read_u64()?);

        // unique_bid is set here if this record is valid.
        if let Some(id) = unique_bid {
            // Common case: only one associated build.
            self.graph.builds[id].set_discovered_ins(deps);
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
pub fn open(path: &Path, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Writer> {
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
