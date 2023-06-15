//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::{
    densemap, densemap::DenseMap, graph::BuildId, graph::FileId, graph::Graph, graph::Hash,
    graph::Hashes,
};
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;

const VERSION: u32 = 1;

/// Files are identified by integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
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

/// The on-disk format is a series of records containing either paths or build state.
#[derive(serde::Serialize, serde::Deserialize)]
enum Record {
    Path(String),
    Build {
        outs: Vec<Id>,
        deps: Vec<Id>,
        hash: Hash,
    },
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

    fn write_path(&mut self, name: &str) -> anyhow::Result<()> {
        let entry = Record::Path(name.to_owned());
        bincode::serialize_into(&mut self.w, &entry)?;
        // XXX buf.flush(&mut self.w)
        Ok(())
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> anyhow::Result<Id> {
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

    pub fn write_build(&mut self, graph: &Graph, id: BuildId, hash: Hash) -> anyhow::Result<()> {
        let build = graph.build(id);
        let outs = build
            .outs()
            .iter()
            .map(|&file_id| self.ensure_id(graph, file_id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let deps = build
            .discovered_ins()
            .iter()
            .map(|&file_id| self.ensure_id(graph, file_id))
            .collect::<anyhow::Result<Vec<_>>>()?;

        let entry = Record::Build { outs, deps, hash };
        bincode::serialize_into(&mut self.w, &entry)?;
        // XXX buf.flush(&mut self.w)
        Ok(())
    }
}

struct Reader<'a> {
    r: BufReader<&'a mut File>,
    ids: IdMap,
    graph: &'a mut Graph,
    hashes: &'a mut Hashes,
}

impl<'a> Reader<'a> {
    fn read_entry(&mut self) -> bincode::Result<Option<Record>> {
        let result = bincode::deserialize_from(&mut self.r);
        if let Err(boxed) = &result {
            if let bincode::ErrorKind::Io(err) = boxed.as_ref() {
                if err.kind() == std::io::ErrorKind::UnexpectedEof {
                    return Ok(None);
                }
            }
        }
        result.map(Some)
    }

    fn add_path(&mut self, name: String) -> std::io::Result<()> {
        // No canonicalization needed, paths were written canonicalized.
        let fileid = self.graph.file_id(name);
        let dbid = self.ids.fileids.push(fileid);
        self.ids.db_ids.insert(fileid, dbid);
        Ok(())
    }

    fn add_build(&mut self, outs: Vec<Id>, deps: Vec<Id>, hash: Hash) -> std::io::Result<()> {
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
        for fileid in outs {
            match self.graph.file(*self.ids.fileids.get(fileid)).input {
                None => {
                    // The graph doesn't believe this is an input; discard.
                    return Ok(());
                }
                Some(bid) => {
                    match unique_bid {
                        None => unique_bid = Some(bid),
                        Some(unique_bid) if unique_bid == bid => {
                            // Ok, matches the existing id.
                        }
                        Some(_) => {
                            // Some outputs have differing inputs; discard.
                            return Ok(());
                        }
                    }
                }
            }
        }

        let deps = deps
            .into_iter()
            .map(|id| *self.ids.fileids.get(id))
            .collect::<Vec<_>>();

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

        while let Some(entry) = self.read_entry()? {
            match entry {
                Record::Path(path) => self.add_path(path)?,
                Record::Build { outs, deps, hash } => self.add_build(outs, deps, hash)?,
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
