//! A single hash over input attributes is recorded and used to determine when
//! those inputs change.
//!
//! See "Manifests instead of mtime order" in
//!   https://neugierig.org/software/blog/2022/03/n2.html

use crate::graph::{Build, FileId, FileState, Graph, MTime};
use std::hash::{self, Hasher};

/// Hash value used to identify a given instance of a Build's execution;
/// compared to verify whether a Build is up to date.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Hash(pub u64);

const UNIT_SEPARATOR: u8 = 0x1F;

// Add a list of files to a hasher; used by hash_build.
fn hash_files(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    graph: &Graph,
    file_state: &FileState,
    ids: &[FileId],
) {
    for &id in ids {
        let name = &graph.file(id).name;
        let mtime = file_state
            .get(id)
            .unwrap_or_else(|| panic!("no state for {:?}", name));
        let mtime = match mtime {
            MTime::Stamp(mtime) => mtime,
            MTime::Missing => panic!("missing file: {:?}", name),
        };
        hasher.write(name.as_bytes());
        std::hash::Hash::hash(&mtime, hasher);
        hasher.write_u8(UNIT_SEPARATOR);
    }
}

// Hashes the inputs of a build to compute a signature.
// Prerequisite: all referenced files have already been stat()ed and are present.
// (It doesn't make sense to hash a build with missing files, because it's out
// of date regardless of the state of the other files.)
pub fn hash_build(graph: &Graph, file_state: &FileState, build: &Build) -> Hash {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_files(&mut hasher, graph, file_state, build.dirtying_ins());
    hasher.write_u8(UNIT_SEPARATOR);
    hash_files(&mut hasher, graph, file_state, build.discovered_ins());
    hasher.write_u8(UNIT_SEPARATOR);
    hasher.write(build.cmdline.as_ref().map(|c| c.as_bytes()).unwrap_or(b""));
    hasher.write_u8(UNIT_SEPARATOR);
    hash::Hash::hash(&build.rspfile, &mut hasher);
    hasher.write_u8(UNIT_SEPARATOR);
    hash_files(&mut hasher, graph, file_state, build.outs());
    Hash(hasher.finish())
}
