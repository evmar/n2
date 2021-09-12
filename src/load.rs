use crate::graph::FileId;
use crate::parse::Statement;
use crate::{graph, parse};
use std::collections::HashMap;

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

fn canon_path(pathstr: &str) -> String {
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

struct BuildImplicitVars<'a> {
    graph: &'a graph::Graph,
    build: &'a graph::Build,
}
impl<'a> BuildImplicitVars<'a> {
    fn file_list(&self, ids: &[FileId]) -> String {
        let mut out = String::new();
        for &id in ids {
            if out.len() > 0 {
                out.push(' ');
            }
            out.push_str(&self.graph.file(id).name);
        }
        out
    }
}
impl<'a> parse::Env for BuildImplicitVars<'a> {
    fn get_var(&self, var: &str) -> Option<String> {
        match var {
            "in" => Some(self.file_list(&self.build.ins)),
            "out" => Some(self.file_list(&self.build.outs)),
            _ => None,
        }
    }
}

struct SavedRule(parse::Rule);

impl PartialEq for SavedRule {
    fn eq(&self, other: &Self) -> bool {
        self.0.name == other.0.name
    }
}
impl Eq for SavedRule {}
impl std::hash::Hash for SavedRule {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.name.hash(state)
    }
}
impl std::borrow::Borrow<str> for SavedRule {
    fn borrow(&self) -> &str {
        &self.0.name
    }
}

struct Loader {
    graph: graph::Graph,
    file_to_id: HashMap<String, FileId>,
    default: Option<FileId>,
    rules: HashMap<String, parse::Rule>,
}

impl Loader {
    fn new() -> Self {
        let mut loader = Loader {
            graph: graph::Graph::new(),
            file_to_id: HashMap::new(),
            default: None,
            rules: HashMap::new(),
        };

        loader.rules.insert(
            "phony".to_owned(),
            parse::Rule {
                name: "phony".to_owned(),
                vars: parse::LazyVars::new(),
            },
        );

        loader
    }

    fn file_id(&mut self, f: String) -> FileId {
        // TODO: so many string copies :<
        let canon = canon_path(&f);
        match self.file_to_id.get(&canon) {
            Some(id) => *id,
            None => {
                let id = self.graph.add_file(canon.clone());
                self.file_to_id.insert(canon, id.clone());
                id
            }
        }
    }

    fn read_file(&mut self, path: &str) -> Result<(), String> {
        let mut bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return Err(format!("read {}: {}", path, e)),
        };
        bytes.push(0);

        let mut parser = parse::Parser::new(unsafe { std::str::from_utf8_unchecked(&bytes) });
        loop {
            let stmt = match parser
                .read()
                .map_err(|err| parser.format_parse_error(err))?
            {
                None => break,
                Some(s) => s,
            };
            match stmt {
                Statement::Include(f) => self.read_file(&f)?,
                Statement::Default(f) => match self.file_to_id.get(f) {
                    Some(id) => self.default = Some(*id),
                    None => return Err(format!("unknown default {:?}", f)),
                },
                Statement::Rule(r) => {
                    self.rules.insert(r.name.clone(), r);
                }
                Statement::Build(b) => {
                    let ins: Vec<FileId> = b.ins.into_iter().map(|f| self.file_id(f)).collect();
                    let outs: Vec<FileId> = b.outs.into_iter().map(|f| self.file_id(f)).collect();
                    let mut build = graph::Build {
                        cmdline: None,
                        ins: ins,
                        outs: outs,
                    };

                    let rule = match self.rules.get(b.rule) {
                        Some(r) => r,
                        None => return Err(format!("unknown rule {:?}", b.rule)),
                    };
                    let key = "command";
                    let implicit_vars = BuildImplicitVars {
                        graph: &self.graph,
                        build: &build,
                    };
                    let envs: [&dyn parse::Env; 4] =
                        [&implicit_vars, &b.vars, &rule.vars, &parser.vars];
                    if let Some(var) = b.vars.get(key).or_else(|| rule.vars.get(key)) {
                        build.cmdline = Some(var.evaluate(&envs));
                    }

                    self.graph.add_build(build);
                }
            };
        }
        Ok(())
    }
}

pub fn read() -> Result<(graph::Graph, Option<FileId>), String> {
    let mut loader = Loader::new();
    loader.read_file("build.ninja")?;
    Ok((loader.graph, loader.default))
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
