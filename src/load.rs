//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::{canonicalize_path, to_owned_canon_path},
    db,
    eval::{self, EvalPart, EvalString},
    graph::{self, FileId, RspFile},
    parse::{self, Statement},
    scanner,
    smallmap::SmallMap,
    trace,
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
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<'_, str>>> {
        let string_to_evalstring =
            |s: String| Some(EvalString::new(vec![EvalPart::Literal(Cow::Owned(s))]));
        match var {
            "in" => string_to_evalstring(self.file_list(self.build.explicit_ins(), ' ')),
            "in_newline" => string_to_evalstring(self.file_list(self.build.explicit_ins(), '\n')),
            "out" => string_to_evalstring(self.file_list(self.build.explicit_outs(), ' ')),
            "out_newline" => string_to_evalstring(self.file_list(self.build.explicit_outs(), '\n')),
            _ => None,
        }
    }
}

/// Internal state used while loading.
#[derive(Default)]
pub struct Loader {
    pub graph: graph::Graph,
    default: Vec<FileId>,
    /// rule name -> list of (key, val)
    rules: HashMap<String, SmallMap<String, eval::EvalString<String>>>,
    pools: SmallMap<String, usize>,
    builddir: Option<String>,
}

impl Loader {
    pub fn new() -> Self {
        let mut loader = Loader::default();

        loader.rules.insert("phony".to_owned(), SmallMap::default());

        loader
    }

    /// Convert a path string to a FileId.
    fn path(&mut self, mut path: String) -> FileId {
        // Perf: this is called while parsing build.ninja files.  We go to
        // some effort to avoid allocating in the common case of a path that
        // refers to a file that is already known.
        canonicalize_path(&mut path);
        self.graph.files.id_from_canonical(path)
    }

    fn evaluate_path(&mut self, path: EvalString<&str>, envs: &[&dyn eval::Env]) -> FileId {
        self.path(path.evaluate(envs))
    }

    fn evaluate_paths(
        &mut self,
        paths: Vec<EvalString<&str>>,
        envs: &[&dyn eval::Env],
    ) -> Vec<FileId> {
        paths
            .into_iter()
            .map(|path| self.evaluate_path(path, envs))
            .collect()
    }

    fn add_build(
        &mut self,
        filename: std::rc::Rc<PathBuf>,
        env: &eval::Vars,
        b: parse::Build,
    ) -> anyhow::Result<()> {
        let ins = graph::BuildIns {
            ids: self.evaluate_paths(b.ins, &[&b.vars, env]),
            explicit: b.explicit_ins,
            implicit: b.implicit_ins,
            order_only: b.order_only_ins,
            // validation is implied by the other counts
        };
        let outs = graph::BuildOuts {
            ids: self.evaluate_paths(b.outs, &[&b.vars, env]),
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

        // temp variable in order to not move all of b into the closure
        let build_vars = &b.vars;
        let lookup = |key: &str| -> Option<String> {
            // Look up `key = ...` binding in build and rule block.
            // See "Variable scope" in the design notes.
            Some(match build_vars.get(key) {
                Some(val) => val.evaluate(&[env]),
                None => rule.get(key)?.evaluate(&[&implicit_vars, build_vars, env]),
            })
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
        let hide_success = lookup("hide_success").is_some();
        let hide_progress = lookup("hide_progress").is_some();

        build.cmdline = cmdline;
        build.desc = desc;
        build.depfile = depfile;
        build.parse_showincludes = parse_showincludes;
        build.rspfile = rspfile;
        build.pool = pool;
        build.hide_success = hide_success;
        build.hide_progress = hide_progress;

        self.graph.add_build(build)
    }

    fn read_file(&mut self, id: FileId, envs: &[&dyn eval::Env]) -> anyhow::Result<()> {
        let path = self.graph.file(id).path().to_path_buf();
        let bytes = match trace::scope("read file", || scanner::read_file_with_nul(&path)) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path.display(), e),
        };
        self.parse(path, &bytes, envs)
    }

    fn evaluate_and_read_file(
        &mut self,
        file: EvalString<&str>,
        envs: &[&dyn eval::Env],
    ) -> anyhow::Result<()> {
        let evaluated = self.evaluate_path(file, envs);
        self.read_file(evaluated, envs)
    }

    pub fn parse(
        &mut self,
        path: PathBuf,
        bytes: &[u8],
        envs: &[&dyn eval::Env],
    ) -> anyhow::Result<()> {
        let filename = std::rc::Rc::new(path);

        let mut parser = parse::Parser::new(&bytes);

        loop {
            let stmt = match parser
                .read()
                .map_err(|err| anyhow!(parser.format_parse_error(&filename, err)))?
            {
                None => break,
                Some(s) => s,
            };

            let mut combined_envs: Vec<&dyn eval::Env> = vec![&parser.vars];
            combined_envs.extend(envs);

            match stmt {
                Statement::Include(id) => trace::scope("include", || {
                    self.evaluate_and_read_file(id, &[&parser.vars])
                })?,
                // TODO: implement scoping for subninja
                Statement::Subninja(id) => trace::scope("subninja", || {
                    self.evaluate_and_read_file(id, &[&parser.vars])
                })?,
                Statement::Default(defaults) => {
                    let evaluated = self.evaluate_paths(defaults, &[&parser.vars]);
                    self.default.extend(evaluated);
                }
                Statement::Rule(rule) => {
                    let mut vars: SmallMap<String, eval::EvalString<String>> = SmallMap::default();
                    for (name, val) in rule.vars.into_iter() {
                        // TODO: We should not need to call .into_owned() here
                        // if we keep the contents of all included files in
                        // memory.
                        vars.insert(name.to_owned(), val.into_owned());
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
            .id_from_canonical(to_owned_canon_path(build_filename));
        loader.read_file(id, &[])
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
pub fn parse(name: &str, mut content: Vec<u8>) -> anyhow::Result<graph::Graph> {
    content.push(0);
    let mut loader = Loader::new();
    trace::scope("loader.read_file", || {
        loader.parse(PathBuf::from(name), &content, &[])
    })?;
    Ok(loader.graph)
}
