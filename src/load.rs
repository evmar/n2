//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::graph::FileId;
use crate::parse::Statement;
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
    rules: HashMap<String, eval::LazyVars>,
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

        loader
            .rules
            .insert("phony".to_owned(), eval::LazyVars::new());

        loader
    }

    fn add_build<'a>(
        &mut self,
        filename: std::rc::Rc<String>,
        env: &eval::Vars<'a>,
        b: parse::Build,
    ) -> anyhow::Result<()> {
        let ins = graph::BuildIns {
            ids: b.ins.into_iter().map(|f| self.graph.file_id(f)).collect(),
            explicit: b.explicit_ins,
            implicit: b.implicit_ins,
            // order_only is unused
        };
        let outs = graph::BuildOuts {
            ids: b.outs.into_iter().map(|f| self.graph.file_id(f)).collect(),
            explicit: b.explicit_outs,
        };
        let mut build = graph::Build::new(
            graph::FileLoc {
                filename,
                line: b.line,
            },
            ins,
            outs,
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
        let envs: [&dyn eval::Env; 4] = [&implicit_vars, build_vars, rule, env];

        let lookup = |key: &str| {
            build_vars
                .get(key)
                .or_else(|| rule.get(key))
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
        let filename = std::rc::Rc::new(String::from(path));

        let mut parser = parse::Parser::new(&mut bytes);
        loop {
            let stmt = match parser
                .read()
                .map_err(|err| anyhow!(parser.format_parse_error(path, err)))?
            {
                None => break,
                Some(s) => s,
            };
            match stmt {
                Statement::Include(path) => trace::scope("include", || self.read_file(&path))?,
                // TODO: implement scoping for subninja
                Statement::Subninja(path) => trace::scope("subninja", || self.read_file(&path))?,
                Statement::Default(defaults) => {
                    let graph = &mut self.graph;
                    self.default
                        .extend(defaults.into_iter().map(|f| graph.file_id(f)));
                }
                Statement::Rule(rule) => {
                    self.rules.insert(rule.name.to_owned(), rule.vars);
                }
                Statement::Build(build) => self.add_build(filename.clone(), &parser.vars, build)?,
                Statement::Pool(pool) => {
                    self.pools.push((pool.name.to_string(), pool.depth));
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
    let mut hashes = graph::Hashes::new();
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
