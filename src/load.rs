use crate::graph::FileId;
use crate::parse::{NStr, NString, Statement};
use crate::{graph, parse};
use std::collections::HashMap;

use std::os::unix::ffi::OsStrExt;

/*fn canon_path(path: &mut NString) {
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

fn canon_path(pathstr: NStr) -> NString {
    let path = pathstr.as_path();
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(_) => panic!("unhandled"),
            std::path::Component::RootDir => {
                out.clear();
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
    NString(out.into_os_string().as_bytes().to_vec())
}

pub fn read() -> Result<(graph::Graph, Option<FileId>), String> {
    let mut bytes = match std::fs::read("build.ninja") {
        Ok(b) => b,
        Err(e) => return Err(format!("read build.ninja: {}", e)),
    };
    bytes.push(0);

    let mut p = parse::Parser::new(&bytes);

    let mut graph = graph::Graph::new();
    let mut file_to_id: HashMap<NString, FileId> = HashMap::new();
    fn file_id(
        graph: &mut graph::Graph,
        hash: &mut HashMap<NString, FileId>,
        f: NString,
    ) -> FileId {
        // TODO: so many string copies :<
        let canon = canon_path(f.as_nstr());
        match hash.get(&canon) {
            Some(id) => *id,
            None => {
                let id = graph.add_file(canon.clone());
                hash.insert(canon, id.clone());
                id
            }
        }
    }

    let mut default: Option<FileId> = None;
    loop {
        let stmt = match p.read().map_err(|err| p.format_parse_error(err))? {
            None => break,
            Some(s) => s,
        };
        match stmt {
            Statement::Default(f) => match file_to_id.get(&f.to_nstring()) {
                Some(id) => default = Some(*id),
                None => return Err(format!("unknown default {:?}", f)),
            },
            Statement::Rule(_) => {} // println!("TODO {:?}", r),
            Statement::Build(b) => {
                let ins: Vec<FileId> = b
                    .ins
                    .into_iter()
                    .map(|f| file_id(&mut graph, &mut file_to_id, f))
                    .collect();
                let outs: Vec<FileId> = b
                    .outs
                    .into_iter()
                    .map(|f| file_id(&mut graph, &mut file_to_id, f))
                    .collect();
                graph.add_build(graph::Build {
                    ins: ins,
                    outs: outs,
                });
            }
        };
    }
    println!("file count {}", file_to_id.len());
    Ok((graph, default))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canon() {
        assert_eq!(
            canon_path(NString::from_str("foo").as_nstr()),
            NString::from_str("foo")
        );

        assert_eq!(
            canon_path(NString::from_str("foo/bar").as_nstr()),
            NString::from_str("foo/bar")
        );

        assert_eq!(
            canon_path(NString::from_str("foo/../bar").as_nstr()),
            NString::from_str("bar")
        );
    }
}
