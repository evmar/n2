//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::graph::{FileId, RspFile};
use crate::parse::Statement;
use crate::smallmap::SmallMap;
use crate::{db, eval, graph, parse, trace};
use anyhow::{anyhow, bail};
use std::borrow::Cow;
use std::collections::HashMap;

/// A variable lookup environment for magic $in/$out variables.
struct BuildImplicitVars<'a> {
    graph: &'a graph::Graph,
    build: &'a graph::Build,
}
impl<'a> BuildImplicitVars<'a> {
    fn file_list(&self, ids: &[FileId], sep: char) -> String {
        let mut out = String::new();
        for &id in ids {
            if !out.is_empty() {
                out.push(sep);
            }
            out.push_str(&self.graph.file(id).name);
        }
        out
    }
}
impl<'a> eval::Env for BuildImplicitVars<'a> {
    fn get_var(&self, var: &str) -> Option<Cow<str>> {
        match var {
            "in" => Some(Cow::Owned(self.file_list(self.build.explicit_ins(), ' '))),
            "in_newline" => Some(Cow::Owned(self.file_list(self.build.explicit_ins(), '\n'))),
            "out" => Some(Cow::Owned(self.file_list(self.build.explicit_outs(), ' '))),
            "out_newline" => Some(Cow::Owned(self.file_list(self.build.explicit_outs(), '\n'))),
            _ => None,
        }
    }
}

/// Internal state used while loading.
struct Loader {
    graph: graph::Graph,
    default: Vec<FileId>,
    rules: HashMap<String, eval::LazyVars>,
    pools: SmallMap<String, usize>,
}

impl parse::Loader for Loader {
    type Path = FileId;
    fn path(&mut self, path: &mut String) -> Self::Path {
        self.graph.file_id(path)
    }
}

impl Loader {
    fn new() -> Self {
        let mut loader = Loader {
            graph: graph::Graph::new(),
            default: Vec::new(),
            rules: HashMap::new(),
            pools: SmallMap::new(),
        };

        loader
            .rules
            .insert("phony".to_owned(), eval::LazyVars::new());

        loader
    }

    fn add_build(
        &mut self,
        filename: std::rc::Rc<String>,
        env: &eval::Vars,
        b: parse::Build<FileId>,
    ) -> anyhow::Result<()> {
        let ins = graph::BuildIns {
            ids: b.ins,
            explicit: b.explicit_ins,
            implicit: b.implicit_ins,
            // order_only is unused
        };
        let outs = graph::BuildOuts {
            ids: b.outs,
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

        let rspfile_path = lookup("rspfile");
        let rspfile_content = lookup("rspfile_content");
        let rspfile = match (rspfile_path, rspfile_content) {
            (None, None) => None,
            (Some(path), Some(content)) => Some(RspFile {
                path: std::path::PathBuf::from(path),
                content,
            }),
            _ => bail!("rspfile and rspfile_content need to be both specified"),
        };

        build.cmdline = cmdline;
        build.desc = desc;
        build.depfile = depfile;
        build.rspfile = rspfile;
        build.pool = pool;

        self.graph.add_build(build)
    }

    fn read_file(&mut self, id: FileId) -> anyhow::Result<()> {
        let path = self.graph.file(id).name.clone();
        let bytes = match trace::scope("fs::read", || std::fs::read(&path)) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path, e),
        };
        self.parse(path, bytes)
    }

    fn parse(&mut self, path: String, mut bytes: Vec<u8>) -> anyhow::Result<()> {
        let filename = std::rc::Rc::new(path);

        let mut parser = parse::Parser::new(&mut bytes);
        loop {
            let stmt = match parser
                .read(self)
                .map_err(|err| anyhow!(parser.format_parse_error(&filename, err)))?
            {
                None => break,
                Some(s) => s,
            };
            match stmt {
                Statement::Include(id) => trace::scope("include", || self.read_file(id))?,
                // TODO: implement scoping for subninja
                Statement::Subninja(id) => trace::scope("subninja", || self.read_file(id))?,
                Statement::Default(defaults) => {
                    self.default.extend(defaults);
                }
                Statement::Rule(rule) => {
                    self.rules.insert(rule.name.to_owned(), rule.vars);
                }
                Statement::Build(build) => self.add_build(filename.clone(), &parser.vars, build)?,
                Statement::Pool(pool) => {
                    self.pools.insert(pool.name.to_string(), pool.depth);
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
    pub pools: SmallMap<String, usize>,
}

/// Load build.ninja/.n2_db and return the loaded build graph and state.
pub fn read(build_filename: &str) -> anyhow::Result<State> {
    let mut loader = Loader::new();
    trace::scope("loader.read_file", || {
        let id = loader.graph.file_id(&mut build_filename.to_string());
        loader.read_file(id)
    })?;
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
pub fn parse(name: String, content: Vec<u8>) -> anyhow::Result<graph::Graph> {
    let mut loader = Loader::new();
    trace::scope("loader.read_file", || loader.parse(name, content))?;
    Ok(loader.graph)
}
