//! The n2 database stores information about previous builds for determining
//! which files are up to date.

use crate::densemap;
use crate::densemap::DenseMap;
use crate::graph::BuildId;
use crate::graph::FileId;
use crate::graph::Graph;
use crate::graph::Hash;
use crate::graph::Hashes;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::{BufReader, BufWriter, Write};

/// Files are identified by integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
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
pub struct IdMap {
    /// Maps db::Id to FileId.
    file_ids: DenseMap<Id, FileId>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<FileId, Id>,
}

impl IdMap {
    pub fn new() -> Self {
        IdMap {
            file_ids: DenseMap::new(),
            db_ids: HashMap::new(),
        }
    }
}

/// An opened database, ready for writes.
pub struct Writer {
    ids: IdMap,
    w: BufWriter<fs::File>,
}

impl Writer {
    fn new(ids: IdMap, w: fs::File) -> Self {
        let w = BufWriter::new(w);
        Writer { ids, w }
    }

    fn ensure_id(&mut self, graph: &Graph, file_id: FileId) -> anyhow::Result<Id> {
        let id = match self.ids.db_ids.get(&file_id) {
            Some(&id) => id,
            None => {
                let id = self.ids.file_ids.push(file_id);
                self.ids.db_ids.insert(file_id, id);

                let entry = DbEntry::File(graph.file(file_id).name.to_owned());
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
    let mut f = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    let mut buf = BufReader::with_capacity(1usize << 16, &mut f);
    let mut de = serde_cbor::Deserializer::from_reader(&mut buf).into_iter();

    let mut ids = IdMap::new();

    loop {
        let entry = match de.next() {
            None => break,
            Some(Ok(entry)) => entry,
            Some(Err(err)) => return Err(err.into()),
        };
        match entry {
            DbEntry::File(mut name) => {
                let file_id = graph.file_id(&mut name);
                let db_id = ids.file_ids.push(file_id);
                ids.db_ids.insert(file_id, db_id);
            }
            DbEntry::Build { outs, deps, hash } => {
                // Map each output to the associated build.
                // In the common case, there is only one.
                let builds = outs
                    .into_iter()
                    .filter_map(|id| graph.file(*ids.file_ids.get(id)).input)
                    .collect::<HashSet<_>>();
                let deps = deps
                    .into_iter()
                    .map(|id| *ids.file_ids.get(id))
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

    Ok(Writer::new(ids, f))
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
