//! A single hash over input attributes is recorded and used to determine when
//! those inputs change.
//!
//! See "Manifests instead of mtime order" in
//!   https://neugierig.org/software/blog/2022/03/n2.html

use crate::graph::{self, Build, FileState, MTime, RspFile};
use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    sync::Arc,
    time::SystemTime,
};

/// Hash value used to identify a given instance of a Build's execution;
/// compared to verify whether a Build is up to date.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct BuildHash(pub u64);

/// A trait for computing a build's manifest.  Indirected as a trait so we can
/// implement it a second time for "-d explain" debug purposes.
trait Manifest {
    /// Write a list of files+mtimes.  desc is used only for "-d explain" output.
    fn write_files(&mut self, desc: &str, file_state: &FileState, ids: &[Arc<graph::File>]);
    fn write_rsp(&mut self, rspfile: &RspFile);
    fn write_cmdline(&mut self, cmdline: &str);
}

fn get_fileid_status<'a>(file_state: &FileState, id: &'a graph::File) -> (&'a str, SystemTime) {
    let name = &id.name;
    let mtime = file_state
        .get(id)
        .unwrap_or_else(|| panic!("no state for {:?}", name));
    let mtime = match mtime {
        MTime::Stamp(mtime) => mtime,
        MTime::Missing => panic!("missing file: {:?}", name),
    };
    (name.as_str(), mtime)
}

/// The BuildHasher used during normal builds, designed to not serialize too much.
#[derive(Default)]
struct TerseHash(DefaultHasher);

const UNIT_SEPARATOR: u8 = 0x1F;

impl TerseHash {
    fn write_string(&mut self, string: &str) {
        string.hash(&mut self.0);
    }

    fn write_separator(&mut self) {
        self.0.write_u8(UNIT_SEPARATOR);
    }

    fn finish(&mut self) -> BuildHash {
        BuildHash(self.0.finish())
    }
}

impl Manifest for TerseHash {
    fn write_files<'a>(&mut self, _desc: &str, file_state: &FileState, ids: &[Arc<graph::File>]) {
        for id in ids {
            let (name, mtime) = get_fileid_status(file_state, &id);
            self.write_string(name);
            mtime.hash(&mut self.0);
        }
        self.write_separator();
    }

    fn write_cmdline(&mut self, cmdline: &str) {
        self.write_string(cmdline);
        self.write_separator();
    }

    fn write_rsp(&mut self, rspfile: &RspFile) {
        rspfile.hash(&mut self.0);
    }
}

fn build_manifest<M: Manifest>(
    manifest: &mut M,
    file_state: &FileState,
    build: &Build,
) -> anyhow::Result<()> {
    manifest.write_files("in", file_state, build.dirtying_ins());
    manifest.write_files("discovered", file_state, build.discovered_ins());
    manifest.write_cmdline(build.get_cmdline().as_deref().unwrap_or(""));
    if let Some(rspfile) = &build.get_rspfile()? {
        manifest.write_rsp(rspfile);
    }
    manifest.write_files("out", file_state, build.outs());
    Ok(())
}

// Hashes the inputs of a build to compute a signature.
// Prerequisite: all referenced files have already been stat()ed and are present.
// (It doesn't make sense to hash a build with missing files, because it's out
// of date regardless of the state of the other files.)
pub fn hash_build(file_state: &FileState, build: &Build) -> anyhow::Result<BuildHash> {
    let mut hasher = TerseHash::default();
    build_manifest(&mut hasher, file_state, build)?;
    Ok(hasher.finish())
}

/// A BuildHasher that records human-readable text for "-d explain" debugging.
#[derive(Default)]
struct ExplainHash {
    text: String,
}

impl Manifest for ExplainHash {
    fn write_files<'a>(&mut self, desc: &str, file_state: &FileState, ids: &[Arc<graph::File>]) {
        writeln!(&mut self.text, "{desc}:").unwrap();
        for id in ids {
            let (name, mtime) = get_fileid_status(file_state, &id);
            let millis = mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            writeln!(&mut self.text, "  {millis} {name}").unwrap();
        }
    }

    fn write_rsp(&mut self, rspfile: &RspFile) {
        writeln!(&mut self.text, "rspfile path: {}", rspfile.path.display()).unwrap();

        let mut h = DefaultHasher::new();
        h.write(rspfile.content.as_bytes());
        writeln!(&mut self.text, "rspfile hash: {:x}", h.finish()).unwrap();
    }

    fn write_cmdline(&mut self, cmdline: &str) {
        writeln!(&mut self.text, "cmdline: {}", cmdline).unwrap();
    }
}

/// Logs human-readable state of all the inputs used for hashing a given build.
/// Used for "-d explain" debugging output.
pub fn explain_hash_build(file_state: &FileState, build: &Build) -> anyhow::Result<String> {
    let mut explainer = ExplainHash::default();
    build_manifest(&mut explainer, file_state, build)?;
    Ok(explainer.text)
}
