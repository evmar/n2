use std::collections::HashMap;

use anyhow;
use n2::{self, fs::MTime};

/// Implementation of Progress that prints nothing.
struct NoProgress {}
impl n2::progress::Progress for NoProgress {
    fn update(&mut self, _counts: &n2::work::StateCounts) {}
    fn flush(&mut self) {}
    fn task_state(
        &mut self,
        _id: n2::graph::BuildId,
        _build: &n2::graph::Build,
        _state: n2::work::BuildState,
    ) {
    }
    fn failed(&mut self, _build: &n2::graph::Build, _output: &[u8]) {}
    fn finish(&mut self) {}
}

struct File {
    content: String,
    mtime: MTime,
}

/// Implementation of fs::FileSystem that is memory-backed.
struct TestFileSystem {
    files: HashMap<String, File>,
}
impl TestFileSystem {
    fn new() -> Self {
        TestFileSystem {
            files: HashMap::new(),
        }
    }

    fn add(&mut self, path: &str, content: impl Into<String>) {
        self.files.insert(
            path.to_string(),
            File {
                content: content.into(),
                mtime: MTime::Stamp(1),
            },
        );
    }
}

impl n2::fs::FileSystem for TestFileSystem {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        match self.files.get(path) {
            Some(file) => Ok(file.content.as_bytes().to_vec()),
            None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        }
    }

    fn stat(&self, path: &str) -> std::io::Result<n2::fs::MTime> {
        match self.files.get(path) {
            Some(file) => Ok(file.mtime),
            None => Ok(MTime::Missing),
        }
    }
}

fn build(fs: &mut TestFileSystem, target: &str) -> anyhow::Result<Option<usize>> {
    let n2::load::State {
        mut graph,
        mut db,
        hashes: last_hashes,
        pools,
        ..
    } = n2::load::read(fs)?;
    let mut progress = NoProgress {};
    let parallelism = 1;
    let mut work = n2::work::Work::new(
        fs,
        &mut graph,
        &last_hashes,
        &mut db,
        &mut progress,
        pools,
        parallelism,
    );
    work.want_file(target)?;
    work.run()
}

#[test]
fn basic() -> anyhow::Result<()> {
    let mut fs = TestFileSystem::new();
    fs.add(
        "build.ninja",
        "
rule touch
  command = touch $out
build out: touch in
",
    );
    fs.add("in", "");
    assert_eq!(build(&mut fs, "out")?, Some(1));
    Ok(())
}
