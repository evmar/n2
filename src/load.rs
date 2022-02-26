//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::graph::FileId;
use crate::parse::Statement;
use crate::scanner::Scanner;
use crate::{db, eval, graph, parse, trace};
use anyhow::{anyhow, bail};
use std::collections::HashMap;

/// A variable lookup environment for magic $in/$out variables.
struct BuildImplicitVars<'a> {
    graph: &'a graph::Graph,
    build: &'a graph::Build,
}
impl<'a> BuildImplicitVars<'a> {
    fn file_list(&self, ids: &[FileId]) -> String {
        let mut out = String::new();
        for &id in ids {
            if !out.is_empty() {
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
            "in" => Some(self.file_list(self.build.explicit_ins())),
            "out" => Some(self.file_list(self.build.explicit_outs())),
            _ => None,
        }
    }
}

/// Internal state used while loading.
struct Loader {
    graph: graph::Graph,
    default: Vec<FileId>,
    rules: HashMap<String, parse::Rule>,
    pools: Vec<(String, usize)>,
}

impl Loader {
    fn new() -> Self {
        let mut loader = Loader {
            graph: graph::Graph::new(),
            default: Vec::new(),
            rules: HashMap::new(),
            pools: Vec::new(),
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
            filename,
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
        let build_vars = &b.vars;
        let envs: [&dyn eval::Env; 4] = [&implicit_vars, build_vars, &rule.vars, env];

        let lookup = |key: &str| {
            build_vars
                .get(key)
                .or_else(|| rule.vars.get(key))
                .map(|var| var.evaluate(&envs))
        };

        let cmdline = lookup("command");
        let desc = lookup("description");
        let depfile = lookup("depfile");
        let pool = lookup("pool");
        build.cmdline = cmdline;
        build.desc = desc;
        build.depfile = depfile;
        build.pool = pool;

        self.graph.add_build(build);
        Ok(())
    }

    fn read_file(&mut self, path: &str) -> anyhow::Result<()> {
        let bytes = match trace::scope("fs::read", || std::fs::read(path)) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path, e),
        };
        self.parse(path, bytes)
    }

    fn parse(&mut self, path: &str, mut bytes: Vec<u8>) -> anyhow::Result<()> {
        bytes.push(0);
        let filename = std::rc::Rc::new(String::from(path));

        let mut parser = parse::Parser::new(Scanner::new(unsafe {
            std::str::from_utf8_unchecked(&bytes)
        }));
        loop {
            let stmt = match parser
                .read()
                .map_err(|err| anyhow!(parser.format_parse_error(path, err)))?
            {
                None => break,
                Some(s) => s,
            };
            match stmt {
                Statement::Include(f) => trace::scope("include", || self.read_file(&f))?,
                // TODO: implement scoping for subninja
                Statement::Subninja(f) => trace::scope("subninja", || self.read_file(&f))?,
                Statement::Default(f) => self.default.push(self.graph.file_id(f)),
                Statement::Rule(r) => {
                    self.rules.insert(r.name.clone(), r);
                }
                Statement::Build(b) => self.add_build(filename.clone(), &parser.vars, b)?,
                Statement::Pool(p) => {
                    self.pools.push((p.name.to_string(), p.depth));
                }
            };
        }
        Ok(())
    }
}

/// State loaded by read().
pub struct State {
    pub graph: graph::Graph,
    pub db: db::Writer,
    pub hashes: graph::Hashes,
    pub default: Vec<FileId>,
    pub pools: Vec<(String, usize)>,
}

/// Load build.ninja/.n2_db and return the loaded build graph and state.
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
        db,
        hashes,
        default: loader.default,
        pools: loader.pools,
    })
}

/// Parse a single file's content.
#[cfg(test)]
pub fn parse(name: &str, content: Vec<u8>) -> anyhow::Result<graph::Graph> {
    let mut loader = Loader::new();
    trace::scope("loader.read_file", || loader.parse(name, content))?;
    Ok(loader.graph)
}
