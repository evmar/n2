//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::{
    densemap, densemap::DenseMap, graph::BuildId, graph::FileId, graph::Graph, graph::Hash,
    graph::Hashes,
};
use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;

const VERSION: u32 = 2;

/// Files are identified by integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

fn read_signature(file: &mut impl Read) -> anyhow::Result<()> {
    let mut buf: [u8; 4] = [0; 4];
    file.read_exact(&mut buf)?;
    if buf.as_slice() != "n2db".as_bytes() {
        bail!("invalid db signature");
    }
    file.read_exact(&mut buf)?;
    let version = u32::from_le_bytes(buf);
    if version != VERSION {
        bail!("db version mismatch: got {version}, expected {VERSION}; TODO: db upgrades etc");
    }
    Ok(())
}

fn write_signature(file: &mut impl Write) -> std::io::Result<()> {
    write!(file, "n2db")?;
    file.write_all(&u32::to_le_bytes(VERSION))
}

/// An opened database, ready for writes.
pub struct Writer {
    ids: IdMap,
    w: BufWriter<File>,
}

impl Writer {
    fn create(path: &str) -> anyhow::Result<Self> {
        let file = File::create(path)?;
        let mut w = BufWriter::new(file);
        write_signature(&mut w)?;
        Ok(Self {
            ids: Default::default(),
            w,
        })
    }

    fn open(mut f: File, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Self> {
        let mut reader = BufReader::with_capacity(1usize << 16, &mut f);
        read_signature(&mut reader)?;
        let mut ids = IdMap::default();

        for entry in serde_cbor::Deserializer::from_reader(&mut reader).into_iter() {
            match entry? {
                DbEntry::File(name) => {
                    let file_id = graph.file_id(name);
                    let db_id = ids.fileids.push(file_id);
                    ids.db_ids.insert(file_id, db_id);
                }
                DbEntry::Build { outs, deps, hash } => {
                    // Map each output to the associated build.
                    // In the common case, there is only one.
                    let builds = outs
                        .into_iter()
                        .filter_map(|id| graph.file(*ids.fileids.get(id)).input)
                        .collect::<HashSet<_>>();
                    let deps = deps
                        .into_iter()
                        .map(|id| *ids.fileids.get(id))
                        .collect::<Vec<_>>();
                    if builds.len() == 1 {
                        // Common case: only one associated build.
                        let bid = builds.into_iter().next().unwrap();
                        graph.build_mut(bid).set_discovered_ins(deps);
                        hashes.set(bid, hash);
                    } else {
                        // The graph layout has changed since this build was recorded.
                        // The hashes won't line up anyway so it will be treated as dirty.
                    }
                }
            }
        }
        Ok(Self {
            ids,
            w: BufWriter::new(f),
        })
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> anyhow::Result<Id> {
        let id = match self.ids.db_ids.get(&fileid) {
            Some(&id) => id,
            None => {
                let id = self.ids.fileids.push(fileid);
                self.ids.db_ids.insert(fileid, id);
                let entry = DbEntry::File(graph.file(fileid).name.to_owned());
                serde_cbor::ser::to_writer(&mut self.w, &entry)?;
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
        let entry = DbEntry::Build { outs, deps, hash };
        serde_cbor::ser::to_writer(&mut self.w, &entry)?;
        self.w.flush()?;
        Ok(())
    }
}

/// Opens or creates an on-disk database, loading its state into the provided Graph.
pub fn open(path: &str, graph: &mut Graph, hashes: &mut Hashes) -> anyhow::Result<Writer> {
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Ok(f) => Writer::open(f, graph, hashes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Writer::create(path),
        Err(err) => Err(anyhow!(err)),
    }
}

#[derive(Serialize, Deserialize)]
enum DbEntry {
    #[serde(rename = "f")]
    File(String),

    #[serde(rename = "b")]
    Build {
        #[serde(rename = "o")]
        outs: Vec<Id>,
        #[serde(rename = "d")]
        deps: Vec<Id>,
        #[serde(rename = "h")]
        hash: Hash,
    },
}
