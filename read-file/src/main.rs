use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

fn count_newlines(buf: &[u8]) -> usize {
    buf.iter().filter(|&&c| c == b'\n').count()
}

fn main() {
    let mut args = std::env::args();
    args.next().unwrap();
    let mmap = match args.next().expect("provide mode").as_str() {
        "read" => false,
        "mmap" => true,
        arg => panic!("mode {:?} must be read/mmap", arg),
    };
    let path = args.next().expect("provide path");

    let nl = if mmap {
        let file = std::fs::File::open(path).unwrap();
        let size = file.metadata().unwrap().size() as usize;
        let buf = unsafe {
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_NOCACHE,
                file.as_raw_fd(),
                0,
            );
            if ptr.is_null() {
                panic!("mmap failed");
            }
            if libc::madvise(ptr, size, libc::MADV_SEQUENTIAL) != 0 {
                panic!("madvise");
            }
            std::slice::from_raw_parts(ptr as *const u8, size)
        };
        count_newlines(buf)
    } else {
        let buf = std::fs::read(path).unwrap();
        count_newlines(&buf)
    };
    println!("nl {}", nl);
}
