use crate::graph::FileId;
use crate::parse::Statement;
use crate::{graph, parse};
use std::collections::{HashMap, HashSet};

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

impl<'a> parse::Env<'a> for graph::Build {
    fn get_var(&self, var: &str) -> Option<String> {
        match var.as_bytes() {
            b"in" => Some(String::from("$in$")),
            b"out" => Some(String::from("$out$")),
            _ => None,
        }
    }
}

struct SavedRule<'a>(parse::Rule<'a>);

impl<'a> PartialEq for SavedRule<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.0.name == other.0.name
    }
}
impl<'a> Eq for SavedRule<'a> {}
impl<'a> std::hash::Hash for SavedRule<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.name.hash(state)
    }
}
impl<'a> std::borrow::Borrow<str> for SavedRule<'a> {
    fn borrow(&self) -> &str {
        &self.0.name
    }
}

pub fn read() -> Result<(graph::Graph, Option<FileId>), String> {
    let mut bytes = match std::fs::read("build.ninja") {
        Ok(b) => b,
        Err(e) => return Err(format!("read build.ninja: {}", e)),
    };
    bytes.push(0);

    let mut p = parse::Parser::new(unsafe { std::str::from_utf8_unchecked(&bytes) });

    let mut graph = graph::Graph::new();
    let mut file_to_id: HashMap<String, FileId> = HashMap::new();
    fn file_id(graph: &mut graph::Graph, hash: &mut HashMap<String, FileId>, f: String) -> FileId {
        // TODO: so many string copies :<
        let canon = canon_path(&f);
        match hash.get(&canon) {
            Some(id) => *id,
            None => {
                let id = graph.add_file(canon.clone());
                hash.insert(canon, id.clone());
                id
            }
        }
    }

    let mut rules: HashSet<SavedRule> = HashSet::new();
    rules.insert(SavedRule(parse::Rule {
        name: "phony",
        vars: parse::DelayEnv::new(),
    }));
    let mut default: Option<FileId> = None;
    loop {
        let stmt = match p.read().map_err(|err| p.format_parse_error(err))? {
            None => break,
            Some(s) => s,
        };
        match stmt {
            Statement::Default(f) => match file_to_id.get(f) {
                Some(id) => default = Some(*id),
                None => return Err(format!("unknown default {:?}", f)),
            },
            Statement::Rule(r) => {
                rules.insert(SavedRule(r));
            }
            Statement::Build(b) => {
                let rule = match rules.get(b.rule) {
                    Some(r) => r,
                    None => return Err(format!("unknown rule {:?}", b.rule)),
                };
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
                let mut build = graph::Build {
                    cmdline: String::from(""),
                    ins: ins,
                    outs: outs,
                };

                let key = "command";
                if let Some(var) = b.vars.get(&key).or_else(|| rule.0.vars.get(&key)) {
                    let envs: [&dyn parse::Env; 4] = [&build, &b.vars, &rule.0.vars, &p.vars];
                    build.cmdline = var.evaluate(&envs);
                }

                graph.add_build(build);
            }
        };
    }
    Ok((graph, default))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canon() {
        assert_eq!(canon_path("foo"), String::from("foo"));

        assert_eq!(canon_path("foo/bar"), String::from("foo/bar"));

        assert_eq!(canon_path("foo/../bar"), String::from("bar"));
    }
}
