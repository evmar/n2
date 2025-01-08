//! Path canonicalization.

use std::mem::MaybeUninit;

/// An on-stack stack of values.
/// Used for tracking locations of parent components within a path.
struct StackStack<T, const CAPACITY: usize> {
    n: usize,
    vals: [MaybeUninit<T>; CAPACITY],
}

impl<T: Copy, const CAPACITY: usize> StackStack<T, CAPACITY> {
    fn new() -> Self {
        StackStack {
            n: 0,
            vals: [MaybeUninit::uninit(); CAPACITY],
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
pub fn canon_path_fast(path: &mut String) {
    assert!(!path.is_empty());
    // Safety: this traverses the path buffer to move data around.
    // We maintain the invariant that *dst always points to a point within
    // the buffer, and that src is always checked against end before reading.
    unsafe {
        let mut components = StackStack::<usize, 60>::new();
        let mut dst = 0;
        let mut src = 0;
        let end = path.len();
        let data = path.as_mut_ptr();

        if src == end {
            return;
        }
        if let b'/' | b'\\' = data.add(src).read() {
            src += 1;
            dst += 1;
        }

        // Outer loop: one iteration per path component.
        while src < end {
            // Peek ahead for special path components: "/", ".", and "..".
            match data.add(src).read() {
                b'/' | b'\\' => {
                    src += 1;
                    continue;
                }
                b'.' => {
                    let mut peek = src + 1;
                    if peek == end {
                        break; // Trailing '.', trim.
                    }
                    match data.add(peek).read() {
                        b'/' | b'\\' => {
                            // "./", skip.
                            src += 2;
                            continue;
                        }
                        b'.' => {
                            // ".."
                            peek = peek + (1);
                            if !(peek == end || matches!(data.add(peek).read(), b'/' | b'\\')) {
                                // Component that happens to start with "..".
                                // Handle as an ordinary component.
                            } else {
                                // ".." component, try to back up.
                                if let Some(ofs) = components.pop() {
                                    dst = ofs;
                                } else {
                                    data.add(dst).write(b'.');
                                    dst += 1;
                                    data.add(dst).write(b'.');
                                    dst += 1;
                                    if peek != end {
                                        data.add(dst).write(data.add(peek).read());
                                        dst += 1;
                                    }
                                }
                                src += 3;
                                continue;
                            }
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
                data.add(dst).write(data.add(src).read());
                src += 1;
                dst += 1;
                if let b'/' | b'\\' = data.add(src - 1).read() {
                    break;
                }
            }
        }

        if dst == 0 {
            path.clear();
            path.push_str(".");
        } else {
            path.as_mut_vec().set_len(dst);
        }
    }
}

#[must_use = "this methods returns the canonicalized version; if possible, prefer `canon_path_fast`"]
pub fn canon_path(path: impl Into<String>) -> String {
    let mut path = path.into();
    canon_path_fast(&mut path);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    // Assert that canon path equals expected path with different path separators
    #[track_caller]
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
    fn not_dot() {
        assert_canon_path_eq("t/.hidden", "t/.hidden");
        assert_canon_path_eq("t/.._lib.c.o", "t/.._lib.c.o");
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
