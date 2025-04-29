use std::{
    cell::RefCell, fs::File, path::PathBuf, rc::Rc, sync::Arc
};
use libc::{
    c_void, mmap, munmap, sysconf, MAP_ANONYMOUS, MAP_FAILED, MAP_FIXED, MAP_PRIVATE,
    PROT_READ, PROT_WRITE, _SC_PAGESIZE,
};

use anyhow::{anyhow, bail, Result};
use ref_cell_hash_map::RefCellHashMapFileMap;
// use libc::{sysconf, _SC_PAGESIZE};
use memmap2::{Mmap, MmapOptions};

use crate::graph::{FileId, Graph};

pub mod ref_cell_hash_map;

pub trait FileMap {
    fn new() -> Self where Self : Sized;
    fn read_file(&self, file_id: FileId, file_path: &PathBuf) -> Result<FileHandle>;
}

pub struct FileLoader<FileMapT : FileMap = RefCellHashMapFileMap> {
    graph: Rc<RefCell<Graph>>,
    files: FileMapT,
}

impl FileLoader {
    pub fn new(graph: Rc<RefCell<Graph>>) -> Self {
        FileLoader { graph, files: RefCellHashMapFileMap::new() }
    }

    pub fn read_file(&self, file_id: FileId) -> Result<FileHandle> {
        let file_path = self.graph.borrow().file(file_id).path().to_path_buf();

        self.files.read_file(file_id, &file_path)
            .map_err(|err| anyhow!("read {}: {}", file_path.display(), err))
    }
}

#[derive(Clone)]
pub struct FileHandle {
    pub size: usize,
    pub path: PathBuf,
    
    mmap: Option<Arc<Mmap>>,
}

impl FileHandle {
    fn from_path(path: &PathBuf) -> Result<FileHandle> {
        let file = Arc::new(File::options().read(true).write(true).open(path)?);
        let metadata = file.metadata()?;
        let size = metadata.len() as usize;

        let mmap = if size > 0 {
            let mmap = unsafe {
                let page_size = sysconf(_SC_PAGESIZE) as usize;
                let mapping_size = (size + page_size).next_multiple_of(page_size);
                let mut mmap = MmapOptions::new().len(mapping_size).map_copy(&file)?;

                // TODO: upstream it to the memmap2 crate? Also what about other platforms?
                let addr2 = libc::mmap(
                    mmap.as_ptr_range().end.sub(page_size) as *mut c_void,
                    page_size,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                    -1,
                    0,
                );
                if addr2 == MAP_FAILED {
                    bail!("mmap failed");
                }

                // Ensure we have a 0 byte at the end
                mmap[size] = 0;

                mmap.make_read_only()?
            };

            Some(Arc::new(mmap))
        } else {
            None
        };

        Ok(FileHandle { mmap, path: path.to_path_buf(), size })
    }
}

impl AsRef<[u8]> for FileHandle {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match self.mmap {
            Some(ref mmap) => mmap[0 ..= self.size].as_ref(),
            None => &[0],
        }
    }
}
