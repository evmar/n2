//! Path canonicalization.

/// Lexically canonicalize a path, removing redundant components.
/// Does not access the disk, but only simplifies things like
/// "foo/./bar" => "foo/bar".
/// These paths can show up due to variable expansion in particular.
pub fn canon_path<T: Into<String>>(inpath: T) -> String {
    let mut path = inpath.into();

    // Safety: this traverses the path buffer to move data around.
    // We maintain the invariant that *dst always points to a point within
    // the buffer, and that src is always checked against end before reading.
    unsafe {
        let mut components: Vec<*mut u8> = Vec::new();
        let mut dst = path.as_mut_ptr();
        let mut src = path.as_ptr();
        let end = src.add(path.len());

        if src == end {
            return path;
        }
        if *src == b'/' {
            src = src.add(1);
            dst = dst.add(1);
        }

        // Outer loop: one iteration per path component.
        while src < end {
            // Peek ahead for special path components: "/", ".", and "..".
            match *src {
                b'/' => {
                    src = src.add(1);
                    continue;
                }
                b'.' => {
                    let mut peek = src.add(1);
                    if peek == end {
                        break; // Trailing '.', trim.
                    }
                    match *peek {
                        b'/' => {
                            // "./", skip.
                            src = src.add(2);
                            continue;
                        }
                        b'.' => {
                            // ".."
                            peek = peek.add(1);
                            if !(peek == end || *peek == b'/') {
                                // Componet that happens to start with "..".
                                // Handle as an ordinary component.
                                break;
                            }
                            // ".." component, try to back up.
                            if let Some(ofs) = components.pop() {
                                dst = ofs;
                            } else {
                                *dst = b'.';
                                dst = dst.add(1);
                                *dst = b'.';
                                dst = dst.add(1);
                                if peek != end {
                                    *dst = b'/';
                                    dst = dst.add(1);
                                }
                            }
                            src = src.add(3);
                            continue;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }

            // Mark this point as a possible target to pop to.
            components.push(dst);

            // Inner loop: copy one path component, including trailing '/'.
            while src < end {
                *dst = *src;
                src = src.add(1);
                dst = dst.add(1);
                if *src.offset(-1) == b'/' {
                    break;
                }
            }
        }

        path.truncate(dst.offset_from(path.as_ptr()) as usize);
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop() {
        assert_eq!(canon_path("foo"), "foo");

        assert_eq!(canon_path("foo/bar"), "foo/bar");
    }

    #[test]
    fn dot() {
        assert_eq!(canon_path("./foo"), "foo");
        assert_eq!(canon_path("foo/."), "foo/");
        assert_eq!(canon_path("foo/./bar"), "foo/bar");
    }

    #[test]
    fn slash() {
        assert_eq!(canon_path("/foo"), "/foo");
        assert_eq!(canon_path("foo//bar"), "foo/bar");
    }

    #[test]
    fn parent() {
        assert_eq!(canon_path("foo/../bar"), "bar");

        assert_eq!(canon_path("/foo/../bar"), "/bar");
        assert_eq!(canon_path("../foo"), "../foo");
        assert_eq!(canon_path("../foo/../bar"), "../bar");
        assert_eq!(canon_path("../../bar"), "../../bar");
    }
}
