//! Path canonicalization.

/*fn canon_path(path: &mut String) {
    let bytes = &mut path.0;
    let mut src = 0;
    let mut dst = 0;
    let mut components = Vec::new();
    while src < bytes.len() {
        match bytes[src] as char {
            '.' => {

            }
            c => {
                bytes[dst] = c as u8;
                dst += 1;
            }
        }
        src += 1;
    }
    bytes.resize(dst, 0);
}*/

pub fn canon_path(pathstr: &str) -> String {
    let path = std::path::Path::new(pathstr);
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(_) => panic!("unhandled"),
            std::path::Component::RootDir => {
                out.clear();
                out.push("/");
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(p) => {
                out.push(p);
            }
        }
    }
    String::from(out.to_str().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canon() {
        assert_eq!(canon_path("foo"), "foo");

        assert_eq!(canon_path("foo/bar"), "foo/bar");

        assert_eq!(canon_path("foo/../bar"), "bar");

        assert_eq!(canon_path("/foo/../bar"), "/bar");
    }
}
