use core::slice;
use std::{os::fd::{AsFd, AsRawFd}, path::Path, ptr::null_mut, sync::Mutex};
use anyhow::bail;
use libc::{c_void, mmap, munmap, strerror, sysconf, MAP_ANONYMOUS, MAP_FAILED, MAP_FIXED, MAP_PRIVATE, PROT_READ, PROT_WRITE, _SC_PAGESIZE};

/// FilePool is a datastucture that is intended to hold onto byte buffers and give out immutable
/// references to them. But it can also accept new byte buffers while old ones are still lent out.
/// This requires interior mutability / unsafe code. Appending to a Vec while references to other
/// elements are held is generally unsafe, because the Vec can reallocate all the prior elements
/// to a new memory location. But if the elements themselves are pointers to stable memory, the
/// contents of those pointers can be referenced safely. This also requires guarding the outer
/// Vec with a Mutex so that two threads don't append to it at the same time.
pub struct FilePool {
    files: Mutex<Vec<(*mut c_void, usize)>>,
}
impl FilePool {
    pub fn new() -> FilePool {
        FilePool {
            files: Mutex::new(Vec::new()),
        }
    }

    pub fn read_file(&self, path: &Path) -> anyhow::Result<&[u8]> {
        let page_size = unsafe {sysconf(_SC_PAGESIZE)} as usize;
        let file = std::fs::File::open(path)?;
        let fd = file.as_fd().as_raw_fd();
        let file_size = file.metadata()?.len() as usize;
        let mapping_size = (file_size + page_size).next_multiple_of(page_size);
        unsafe {
            // size + 1 to add a null terminator.
            let addr = mmap(null_mut(), mapping_size, PROT_READ, MAP_PRIVATE, fd, 0);
            if addr == MAP_FAILED {
                bail!("mmap failed");
            }

            let addr2 = mmap(
                addr.add(mapping_size).sub(page_size),
                page_size,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1, 0);
            if addr2 == MAP_FAILED {
                bail!("mmap failed");
            }
            *(addr.add(mapping_size).sub(page_size) as *mut u8) = 0;
            // The manpages say the extra bytes past the end of the file are
            // zero-filled, but just to make sure:
            assert!(*(addr.add(file_size) as *mut u8) == 0);
            
            let files = &mut self.files.lock().unwrap();
            files.push((addr, mapping_size));

            Ok(slice::from_raw_parts(addr as *mut u8, file_size + 1))
        }
    }
}

// SAFETY: Sync isn't implemented automatically because we have a *mut pointer,
// but that pointer isn't used at all aside from the drop implementation, so
// we won't have data races.
unsafe impl Sync for FilePool{}

impl Drop for FilePool {
    fn drop(&mut self) {
        let files = self.files.lock().unwrap();
        for &(addr, len) in files.iter() {
            unsafe {
                munmap(addr, len);
            }
        }
    }
}
