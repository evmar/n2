use std::os::unix::prelude::MetadataExt;

/// MTime info gathered for a file.  This also models "file is absent".
/// It's not using an Option<> just because it makes the code using it easier
/// to follow.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MTime {
    Missing,
    Stamp(u32),
}

pub trait FileSystem {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
    /// stat() an on-disk path, producing its MTime.
    fn stat(&self, path: &str) -> std::io::Result<MTime>;
}

pub struct RealFileSystem {}
impl RealFileSystem {
    pub fn new() -> Self {
        RealFileSystem {}
    }
}

impl FileSystem for RealFileSystem {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn stat(&self, path: &str) -> std::io::Result<MTime> {
        Ok(match std::fs::metadata(path) {
            Ok(meta) => MTime::Stamp(meta.mtime() as u32),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    MTime::Missing
                } else {
                    return Err(err);
                }
            }
        })
    }
}
