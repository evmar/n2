//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::{canon_path, canon_path_fast},
    graph::{FileId, RspFile},
    parse::Statement,
    smallmap::SmallMap,
    {db, eval, graph, parse, trace},
};
use anyhow::{anyhow, bail};
use std::collections::HashMap;
use std::path::PathBuf;
use std::{borrow::Cow, path::Path};

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
#[derive(Default)]
struct Loader {
    graph: graph::Graph,
    default: Vec<FileId>,
    /// rule name -> list of (key, val)
    rules: HashMap<String, SmallMap<String, eval::EvalString<String>>>,
    pools: SmallMap<String, usize>,
    builddir: Option<String>,
}

impl parse::Loader for Loader {
    type Path = FileId;
    fn path(&mut self, path: &mut str) -> Self::Path {
        // Perf: this is called while parsing build.ninja files.  We go to
        // some effort to avoid allocating in the common case of a path that
        // refers to a file that is already known.
        let len = canon_path_fast(path);
        self.graph.files.id_from_canonical(&path[..len])
    }
}

impl Loader {
    fn new() -> Self {
        let mut loader = Loader::default();

        loader.rules.insert("phony".to_owned(), SmallMap::default());

        loader
    }

    fn add_build(
        &mut self,
        filename: std::rc::Rc<PathBuf>,
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

        // Expand all build-scoped variable values, as they may be referred to in rules.
        let mut build_vars = SmallMap::default();
        for &(name, ref val) in b.vars.iter() {
            let val = val.evaluate(&[&implicit_vars, &build_vars, env]);
            build_vars.insert(name, val);
        }

        let envs: [&dyn eval::Env; 4] = [&implicit_vars, &build_vars, rule, env];
        let lookup = |key: &str| -> Option<String> {
            // Look up `key = ...` binding in build and rule block.
            let val = match build_vars.get(key) {
                Some(val) => val.clone(),
                None => rule.get(key)?.evaluate(&envs),
            };
            Some(val)
        };

        let cmdline = lookup("command");
        let desc = lookup("description");
        let depfile = lookup("depfile");
        let parse_showincludes = match lookup("deps").as_deref() {
            None => false,
            Some("gcc") => false,
            Some("msvc") => true,
            Some(other) => bail!("invalid deps attribute {:?}", other),
        };
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
        build.parse_showincludes = parse_showincludes;
        build.rspfile = rspfile;
        build.pool = pool;

        self.graph.add_build(build)
    }

    fn read_file(&mut self, id: FileId) -> anyhow::Result<()> {
        let path = self.graph.file(id).path().to_path_buf();
        let bytes = match trace::scope("fs::read", || std::fs::read(&path)) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path.display(), e),
        };
        self.parse(path, bytes)
    }

    fn parse(&mut self, path: PathBuf, mut bytes: Vec<u8>) -> anyhow::Result<()> {
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
                    let mut vars: SmallMap<String, eval::EvalString<String>> = SmallMap::default();
                    for (name, val) in rule.vars.into_iter() {
                        vars.insert(name.to_owned(), val);
                    }
                    self.rules.insert(rule.name.to_owned(), vars);
                }
                Statement::Build(build) => self.add_build(filename.clone(), &parser.vars, build)?,
                Statement::Pool(pool) => {
                    self.pools.insert(pool.name.to_string(), pool.depth);
                }
            };
        }
        self.builddir = parser.vars.get("builddir").cloned();
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
        let id = loader
            .graph
            .files
            .id_from_canonical(canon_path(build_filename));
        loader.read_file(id)
    })?;
    let mut hashes = graph::Hashes::default();
    let db = trace::scope("db::open", || {
        let mut db_path = PathBuf::from(".n2_db");
        if let Some(builddir) = &loader.builddir {
            db_path = Path::new(&builddir).join(db_path);
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
        };
        db::open(&db_path, &mut loader.graph, &mut hashes)
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
    trace::scope("loader.read_file", || {
        loader.parse(PathBuf::from(name), content)
    })?;
    Ok(loader.graph)
}
