//! Path canonicalization.

use std::mem::MaybeUninit;

/// An on-stack stack of values.
/// Used for tracking locations of parent components within a path.
struct StackStack<T> {
    n: usize,
    vals: [MaybeUninit<T>; 60],
}

impl<T: Copy> StackStack<T> {
    fn new() -> Self {
        StackStack {
            n: 0,
            // Safety: we only access vals[i] after setting it.
            vals: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    fn push(&mut self, val: T) {
        if self.n >= self.vals.len() {
            panic!("too many path components");
        }
        self.vals[self.n].write(val);
        self.n += 1;
    }

    fn pop(&mut self) -> Option<T> {
        if self.n > 0 {
            self.n -= 1;
            // Safety: we only access vals[i] after setting it.
            Some(unsafe { self.vals[self.n].assume_init() })
        } else {
            None
        }
    }
}

/// Lexically canonicalize a path, removing redundant components.
/// Does not access the disk, but only simplifies things like
/// "foo/./bar" => "foo/bar".
/// These paths can show up due to variable expansion in particular.
/// Returns the new length of the path, guaranteed <= the original length.
#[must_use]
pub fn canon_path_fast(path: &mut str) -> usize {
    assert!(!path.is_empty());
    // Safety: this traverses the path buffer to move data around.
    // We maintain the invariant that *dst always points to a point within
    // the buffer, and that src is always checked against end before reading.
    unsafe {
        let mut components = StackStack::<*mut u8>::new();
        let mut dst = path.as_mut_ptr();
        let mut src = path.as_ptr();
        let start = path.as_mut_ptr();
        let end = src.add(path.len());

        if src == end {
            return 0;
        }
        if *src == b'/' || *src == b'\\' {
            src = src.add(1);
            dst = dst.add(1);
        }

        // Outer loop: one iteration per path component.
        while src < end {
            // Peek ahead for special path components: "/", ".", and "..".
            match *src {
                b'/' | b'\\' => {
                    src = src.add(1);
                    continue;
                }
                b'.' => {
                    let mut peek = src.add(1);
                    if peek == end {
                        break; // Trailing '.', trim.
                    }
                    match *peek {
                        b'/' | b'\\' => {
                            // "./", skip.
                            src = src.add(2);
                            continue;
                        }
                        b'.' => {
                            // ".."
                            peek = peek.add(1);
                            if !(peek == end || *peek == b'/' || *peek == b'\\') {
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
                                    *dst = *peek;
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
                if *src.offset(-1) == b'/' || *src.offset(-1) == b'\\' {
                    break;
                }
            }
        }

        if dst == start {
            *start = b'.';
            1
        } else {
            dst.offset_from(start) as usize
        }
    }
}

pub fn canon_path<T: Into<String>>(path: T) -> String {
    let mut path = path.into();
    let len = canon_path_fast(&mut path);
    path.truncate(len);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    // Assert that canon path equals expected path with different path separators
    fn assert_canon_path_eq(left: &str, right: &str) {
        assert_eq!(canon_path(left), right);
        assert_eq!(
            canon_path(left.replace('/', "\\")),
            right.replace('/', "\\")
        );
    }

    #[test]
    fn noop() {
        assert_canon_path_eq("foo", "foo");

        assert_canon_path_eq("foo/bar", "foo/bar");
    }

    #[test]
    fn dot() {
        assert_canon_path_eq("./foo", "foo");
        assert_canon_path_eq("foo/.", "foo/");
        assert_canon_path_eq("foo/./bar", "foo/bar");
        assert_canon_path_eq("./", ".");
        assert_canon_path_eq("./.", ".");
        assert_canon_path_eq("././", ".");
        assert_canon_path_eq("././.", ".");
        assert_canon_path_eq(".", ".");
    }

    #[test]
    fn slash() {
        assert_canon_path_eq("/foo", "/foo");
        assert_canon_path_eq("foo//bar", "foo/bar");
    }

    #[test]
    fn parent() {
        assert_canon_path_eq("foo/../bar", "bar");

        assert_canon_path_eq("/foo/../bar", "/bar");
        assert_canon_path_eq("../foo", "../foo");
        assert_canon_path_eq("../foo/../bar", "../bar");
        assert_canon_path_eq("../../bar", "../../bar");
        assert_canon_path_eq("./../foo", "../foo");
        assert_canon_path_eq("foo/..", ".");
        assert_canon_path_eq("foo/../", ".");
        assert_canon_path_eq("foo/../../", "../");
        assert_canon_path_eq("foo/../../bar", "../bar");
    }
}
