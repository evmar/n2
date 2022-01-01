//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::graph::FileId;
use crate::parse::Statement;
use crate::scanner::Scanner;
use crate::{db, eval, graph, parse, trace};
use anyhow::{anyhow, bail};
use std::collections::HashMap;

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
impl<'a> eval::Env for BuildImplicitVars<'a> {
    fn get_var(&self, var: &str) -> Option<String> {
        match var {
            "in" => Some(self.file_list(&self.build.explicit_ins())),
            "out" => Some(self.file_list(&self.build.explicit_outs())),
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
    default: Option<FileId>,
    rules: HashMap<String, parse::Rule>,
}

pub struct State {
    pub graph: graph::Graph,
    pub db: db::Writer,
    pub hashes: graph::Hashes,
    pub default: Option<FileId>,
}

impl Loader {
    fn new() -> Self {
        let mut loader = Loader {
            graph: graph::Graph::new(),
            default: None,
            rules: HashMap::new(),
        };

        loader.rules.insert(
            "phony".to_owned(),
            parse::Rule {
                name: "phony".to_owned(),
                vars: eval::LazyVars::new(),
            },
        );

        loader
    }

    fn add_build<'a>(
        &mut self,
        filename: std::rc::Rc<String>,
        env: &eval::Vars<'a>,
        b: parse::Build,
    ) -> anyhow::Result<()> {
        let mut build = graph::Build::new(graph::FileLoc {
            filename: filename,
            line: b.line,
        });
        build.set_ins(
            b.ins.into_iter().map(|f| self.graph.file_id(f)).collect(),
            b.explicit_ins,
            b.implicit_ins,
            b.order_only_ins,
        );
        build.set_outs(
            b.outs.into_iter().map(|f| self.graph.file_id(f)).collect(),
            b.explicit_outs,
        );

        let rule = match self.rules.get(b.rule) {
            Some(r) => r,
            None => bail!("unknown rule {:?}", b.rule),
        };

        let implicit_vars = BuildImplicitVars {
            graph: &self.graph,
            build: &build,
        };
        let envs: [&dyn eval::Env; 4] = [&implicit_vars, &b.vars, &rule.vars, env];

        let cmdline = match b.vars.get("command").or_else(|| rule.vars.get("command")) {
            Some(var) => Some(var.evaluate(&envs)),
            None => None,
        };
        let desc = match b
            .vars
            .get("description")
            .or_else(|| rule.vars.get("description"))
        {
            Some(var) => Some(var.evaluate(&envs)),
            None => None,
        };
        let depfile = match b.vars.get("depfile").or_else(|| rule.vars.get("depfile")) {
            Some(var) => Some(var.evaluate(&envs)),
            None => None,
        };
        build.cmdline = cmdline;
        build.desc = desc;
        build.depfile = depfile;

        self.graph.add_build(build);
        Ok(())
    }

    fn read_file(&mut self, path: &str) -> anyhow::Result<()> {
        let mut bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path, e),
        };
        bytes.push(0);
        let filename = std::rc::Rc::new(String::from(path));

        let mut parser = parse::Parser::new(Scanner::new(unsafe {
            std::str::from_utf8_unchecked(&bytes)
        }));
        loop {
            let stmt = match trace::scope("parser.read", || parser.read())
                .map_err(|err| anyhow!(parser.format_parse_error(path, err)))?
            {
                None => break,
                Some(s) => s,
            };
            match stmt {
                Statement::Include(f) => self.read_file(&f)?,
                Statement::Default(f) => self.default = Some(self.graph.file_id(f)),
                Statement::Rule(r) => {
                    self.rules.insert(r.name.clone(), r);
                }
                Statement::Build(b) => {
                    self.add_build(filename.clone(), &parser.vars, b)?;
                }
            };
        }
        Ok(())
    }
}

pub fn read() -> anyhow::Result<State> {
    let mut loader = Loader::new();
    trace::scope("loader.read_file", || loader.read_file("build.ninja"))?;
    let mut hashes = graph::Hashes::new(&loader.graph);
    let db = trace::scope("db::open", || {
        db::open(".n2_db", &mut loader.graph, &mut hashes)
    })
    .map_err(|err| anyhow!("load .n2_db: {}", err))?;
    Ok(State {
        graph: loader.graph,
        hashes: hashes,
        db: db,
        default: loader.default,
    })
}
