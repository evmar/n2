use std::{cell::RefCell, path::PathBuf};
use std::collections::HashMap;

use anyhow::Result;

use crate::graph::{FileId};

use super::{FileHandle, FileMap};

#[derive(Default)]
pub struct RefCellHashMapFileMap(RefCell<HashMap<FileId, FileHandle>>);

impl FileMap for RefCellHashMapFileMap {
    fn new() -> Self where Self : Sized {
        RefCellHashMapFileMap(RefCell::new(HashMap::with_capacity(16)))
    }

    fn read_file(&self, file_id: FileId, file_path: &PathBuf) -> Result<FileHandle> {
        let file = { self.0.borrow().get(&file_id).map(Clone::clone) };
        match file {
            Some(handle) => Ok(handle),
            None => {
                let handle = FileHandle::from_path(file_path)?;
                let _ = self.0.borrow_mut().insert(file_id, handle.clone());

                Ok(handle)
            }
        }
    }
}

