//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::{canon_path, canon_path_fast},
    eval::{EvalPart, EvalString, Vars},
    graph::{FileId, RspFile},
    parse::Statement,
    scanner,
    smallmap::SmallMap,
    {db, eval, graph, parse, trace}, thread_pool::{self, ThreadPoolExecutor}, scanner::ParseResult,
};
use anyhow::{anyhow, bail};
use std::{collections::HashMap, sync::Mutex, cell::UnsafeCell, thread::Thread};
use std::path::PathBuf;
use std::{borrow::Cow, path::Path};

/// A variable lookup environment for magic $in/$out variables.
struct BuildImplicitVars<'text> {
    graph: &'text graph::Graph,
    build: &'text graph::Build,
}
impl<'text> BuildImplicitVars<'text> {
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
impl<'text> eval::Env for BuildImplicitVars<'text> {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>> {
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

/// FilePool is a datastucture that is intended to hold onto byte buffers and give out immutable
/// references to them. But it can also accept new byte buffers while old ones are still lent out.
/// This requires interior mutability / unsafe code. Appending to a Vec while references to other
/// elements are held is generally unsafe, because the Vec can reallocate all the prior elements
/// to a new memory location. But if the elements themselves are Vecs that never change, the
/// contents of those inner vecs can be referenced safely. This also requires guarding the outer
/// Vec with a Mutex so that two threads don't append to it at the same time.
struct FilePool {
    files: Mutex<UnsafeCell<Vec<Vec<u8>>>>,
}
impl FilePool {
    fn new() -> FilePool {
        FilePool { files: Mutex::new(UnsafeCell::new(Vec::new())) }
    }
    /// Add the file to the file pool, and then return it back to the caller as a slice.
    /// Returning the Vec instead of a slice would be unsafe, as the Vecs will be reallocated.
    fn add_file(&self, file: Vec<u8>) -> &[u8] {
        let files = self.files.lock().unwrap().get();
        unsafe {
            (*files).push(file);
            (*files).last().unwrap().as_slice()
        }
    }
}

/// Internal state used while loading.
pub struct Loader<'text> {
    file_pool: &'text FilePool,
    vars: Vars<'text>,
    graph: graph::Graph,
    default: Vec<FileId>,
    /// rule name -> list of (key, val)
    rules: HashMap<&'text str, SmallMap<&'text str, eval::EvalString<&'text str>>>,
    pools: SmallMap<String, usize>,
    builddir: Option<String>,
}

impl<'text> Loader<'text> {
    pub fn new(file_pool: &'text FilePool) -> Self {
        let mut loader = Loader {
            file_pool,
            vars: Vars::default(),
            graph: graph::Graph::default(),
            default: Vec::default(),
            rules: HashMap::default(),
            pools: SmallMap::default(),
            builddir: None,
        };

        loader.rules.insert("phony", SmallMap::default());

        loader
    }

    /// Convert a path string to a FileId.  For performance reasons
    /// this requires an owned 'path' param.
    fn path(&mut self, mut path: String) -> FileId {
        // Perf: this is called while parsing build.ninja files.  We go to
        // some effort to avoid allocating in the common case of a path that
        // refers to a file that is already known.
        let len = canon_path_fast(&mut path);
        path.truncate(len);
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
        b: parse::Build,
    ) -> anyhow::Result<()> {
        let ins = graph::BuildIns {
            ids: b.ins.iter().map(|x| {
                self.path(x.evaluate(&[&b.vars, &self.vars]))
            }).collect(),
            explicit: b.explicit_ins,
            implicit: b.implicit_ins,
            order_only: b.order_only_ins,
            // validation is implied by the other counts
        };
        let outs = graph::BuildOuts {
            ids: b.outs.iter().map(|x| {
                self.path(x.evaluate(&[&b.vars, &self.vars]))
            }).collect(),
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
            Some(match rule.get(key) {
                Some(val) => val.evaluate(&[&implicit_vars, build_vars, &self.vars]),
                None => build_vars.get(key)?.evaluate(&[&self.vars]),
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

        build.cmdline = cmdline;
        build.desc = desc;
        build.depfile = depfile;
        build.parse_showincludes = parse_showincludes;
        build.rspfile = rspfile;
        build.pool = pool;

        self.graph.add_build(build)
    }

    fn read_file(&mut self, id: FileId, executor: &ThreadPoolExecutor<'text>) -> anyhow::Result<()> {
        let path = self.graph.file(id).path().to_path_buf();
        let bytes = match trace::scope("read file", || scanner::read_file_with_nul(&path)) {
            Ok(b) => b,
            Err(e) => bail!("read {}: {}", path.display(), e),
        };
        self.parse(path, self.file_pool.add_file(bytes), executor)
    }

    pub fn parse(&mut self, path: PathBuf, bytes: &'text [u8], executor: &ThreadPoolExecutor<'text>) -> anyhow::Result<()> {
        let filename = std::rc::Rc::new(path);

        let chunks = parse::split_manifest_into_chunks(bytes, executor.get_num_threads().get());

        let mut receivers = Vec::with_capacity(chunks.len());

        for chunk in chunks.into_iter() {
            let (sender, receiver) = std::sync::mpsc::channel::<ParseResult<Statement<'text>>>();
            receivers.push(receiver);
            executor.execute(move || {
                let mut parser = parse::Parser::new(chunk);
                parser.read_to_channel(sender);
            })
        }

        for stmt in receivers.into_iter().flatten() {
            match stmt {
                Ok(Statement::VariableAssignment((name, val))) => {
                    self.vars.insert(name, val.evaluate(&[&self.vars]));
                },
                Ok(Statement::Include(id)) => trace::scope("include", || {
                    let evaluated = self.path(id.evaluate(&[&self.vars]));
                    self.read_file(evaluated, executor)
                })?,
                // TODO: implement scoping for subninja
                Ok(Statement::Subninja(id)) => trace::scope("subninja", || {
                    let evaluated = self.path(id.evaluate(&[&self.vars]));
                    self.read_file(evaluated, executor)
                })?,
                Ok(Statement::Default(defaults)) => {
                    let it: Vec<FileId> = defaults.into_iter().map(|x| {
                        self.path(x.evaluate(&[&self.vars]))
                    }).collect();
                    self.default.extend(it);
                }
                Ok(Statement::Rule(rule)) => {
                    self.rules.insert(rule.name, rule.vars);
                }
                Ok(Statement::Build(build)) => self.add_build(filename.clone(), build)?,
                Ok(Statement::Pool(pool)) => {
                    self.pools.insert(pool.name.to_string(), pool.depth);
                }
                // TODO: Call format_parse_error
                Err(e) => bail!(e.msg),
            };
        }
        self.builddir = self.vars.get("builddir").cloned();
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
    let file_pool = FilePool::new();
    let mut loader = trace::scope("loader.read_file", || -> anyhow::Result<Loader> {
        thread_pool::scoped_thread_pool(std::thread::available_parallelism()?, |executor| {
            let mut loader = Loader::new(&file_pool);
            let id = loader
                .graph
                .files
                .id_from_canonical(canon_path(build_filename));
            loader.read_file(id, executor)?;
            Ok(loader)
        })
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
    let file_pool = FilePool::new();
    let mut loader = Loader::new(&file_pool);
    trace::scope("loader.read_file", || {
        thread_pool::scoped_thread_pool(std::num::NonZeroUsize::new(1).unwrap(), |executor| {
            loader.parse(PathBuf::from(name), &content, executor)
        })
    })?;
    Ok(loader.graph)
}
