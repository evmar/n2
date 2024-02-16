//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::canon_path,
    densemap::Index,
    eval::{EvalPart, EvalString},
    file_pool::FilePool,
    graph::{BuildId, FileId, Graph, RspFile},
    parse::{Build, DefaultStmt, IncludeOrSubninja, Rule, Statement, VariableAssignment},
    scanner,
    scanner::ParseResult,
    smallmap::SmallMap,
    {db, eval, graph, parse, trace},
};
use anyhow::{anyhow, bail};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::{
    borrow::Cow,
    path::Path,
    sync::{atomic::AtomicUsize, mpsc::TryRecvError},
};
use std::{
    cell::UnsafeCell,
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
    thread::available_parallelism,
};
use std::{path::PathBuf, sync::atomic::AtomicU32};

/// A variable lookup environment for magic $in/$out variables.
struct BuildImplicitVars<'a> {
    explicit_ins: &'a [String],
    explicit_outs: &'a [String],
}
impl<'text> eval::Env for BuildImplicitVars<'text> {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>> {
        let string_to_evalstring =
            |s: String| Some(EvalString::new(vec![EvalPart::Literal(Cow::Owned(s))]));
        match var {
            "in" => string_to_evalstring(self.explicit_ins.join(" ")),
            "in_newline" => string_to_evalstring(self.explicit_ins.join("\n")),
            "out" => string_to_evalstring(self.explicit_outs.join(" ")),
            "out_newline" => string_to_evalstring(self.explicit_outs.join("\n")),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ScopePosition(pub usize);

pub struct ParentScopeReference<'text>(pub Arc<Scope<'text>>, pub ScopePosition);

pub struct Scope<'text> {
    parent: Option<ParentScopeReference<'text>>,
    rules: HashMap<&'text str, Rule<'text>>,
    variables: FxHashMap<&'text str, Vec<VariableAssignment<'text>>>,
    next_free_position: ScopePosition,
}

impl<'text> Scope<'text> {
    pub fn new(parent: Option<ParentScopeReference<'text>>) -> Self {
        Self {
            parent,
            rules: HashMap::new(),
            variables: FxHashMap::default(),
            next_free_position: ScopePosition(0),
        }
    }

    pub fn get_and_inc_scope_position(&mut self) -> ScopePosition {
        let result = self.next_free_position;
        self.next_free_position.0 += 1;
        result
    }

    pub fn get_last_scope_position(&self) -> ScopePosition {
        self.next_free_position
    }

    pub fn get_rule(&self, name: &'text str, position: ScopePosition) -> Option<&Rule> {
        match self.rules.get(name) {
            Some(rule) if rule.scope_position.0 < position.0 => Some(rule),
            Some(_) | None => self
                .parent
                .as_ref()
                .map(|p| p.0.get_rule(name, p.1))
                .flatten(),
        }
    }

    pub fn evaluate(&self, result: &mut String, varname: &'text str, position: ScopePosition) {
        if let Some(variables) = self.variables.get(varname) {
            let i = variables
                .binary_search_by(|x| {
                    if x.scope_position.0 < position.0 {
                        Ordering::Less
                    } else if x.scope_position.0 > position.0 {
                        Ordering::Greater
                    } else {
                        // If we're evaluating a variable assignment, we don't want to
                        // get the same assignment, but instead, we want the one just
                        // before it. So return Greater instead of Equal.
                        Ordering::Greater
                    }
                })
                .unwrap_err();
            let i = std::cmp::min(i, variables.len() - 1);
            if variables[i].scope_position.0 < position.0 {
                variables[i].evaluate(result, &self);
                return;
            }
            // We couldn't find a variable assignment before the input
            // position, so check the parent scope if there is one.
        }
        if let Some(parent) = &self.parent {
            parent.0.evaluate(result, varname, position);
        }
    }
}

fn add_build<'text>(
    files: &Files,
    filename: &Arc<PathBuf>,
    scope: &Scope,
    b: parse::Build,
) -> anyhow::Result<graph::Build> {
    let ins: Vec<_> = b
        .ins
        .iter()
        .map(|x| canon_path(x.evaluate(&[&b.vars], scope, b.scope_position)))
        .collect();
    let outs: Vec<_> = b
        .outs
        .iter()
        .map(|x| canon_path(x.evaluate(&[&b.vars], scope, b.scope_position)))
        .collect();

    let rule = match scope.get_rule(b.rule, b.scope_position) {
        Some(r) => r,
        None => bail!("unknown rule {:?}", b.rule),
    };

    let implicit_vars = BuildImplicitVars {
        explicit_ins: &ins[..b.explicit_ins],
        explicit_outs: &outs[..b.explicit_outs],
    };

    // temp variable in order to not move all of b into the closure
    let build_vars = &b.vars;
    let lookup = |key: &str| -> Option<String> {
        // Look up `key = ...` binding in build and rule block.
        Some(match rule.vars.get(key) {
            Some(val) => val.evaluate(&[&implicit_vars, build_vars], scope, b.scope_position),
            None => build_vars.get(key)?.evaluate(&[], scope, b.scope_position),
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

    let build_id = files.create_build_id();

    let ins = graph::BuildIns {
        ids: ins
            .into_iter()
            .map(|x| {
                let f = files.id_from_canonical(x);
                f.dependents.prepend(build_id);
                f
            })
            .collect(),
        explicit: b.explicit_ins,
        implicit: b.implicit_ins,
        order_only: b.order_only_ins,
        // validation is implied by the other counts
    };
    let outs = graph::BuildOuts {
        ids: outs
            .into_iter()
            .map(|x| files.id_from_canonical(x))
            .collect(),
        explicit: b.explicit_outs,
    };
    let mut build = graph::Build::new(
        build_id,
        graph::FileLoc {
            filename: filename.clone(),
            line: b.line,
        },
        ins,
        outs,
    );

    build.cmdline = cmdline;
    build.desc = desc;
    build.depfile = depfile;
    build.parse_showincludes = parse_showincludes;
    build.rspfile = rspfile;
    build.pool = pool;

    graph::Graph::initialize_build(&mut build)?;

    Ok(build)
}

struct Files {
    by_name: dashmap::DashMap<Arc<String>, Arc<graph::File>>,
    next_build_id: AtomicUsize,
}
impl Files {
    pub fn new() -> Self {
        Self {
            by_name: dashmap::DashMap::new(),
            next_build_id: AtomicUsize::new(0),
        }
    }

    pub fn id_from_canonical(&self, file: String) -> Arc<graph::File> {
        match self.by_name.entry(Arc::new(file)) {
            dashmap::mapref::entry::Entry::Occupied(o) => o.get().clone(),
            dashmap::mapref::entry::Entry::Vacant(v) => {
                let mut f = graph::File::default();
                f.name = v.key().clone();
                let f = Arc::new(f);
                v.insert(f.clone());
                f
            }
        }
    }

    pub fn into_maps(self) -> dashmap::DashMap<Arc<String>, Arc<graph::File>> {
        self.by_name
    }

    pub fn create_build_id(&self) -> BuildId {
        let id = self
            .next_build_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        BuildId::from(id)
    }
}

#[derive(Default)]
struct SubninjaResults<'text> {
    pub builds: Vec<graph::Build>,
    defaults: Vec<Arc<graph::File>>,
    builddir: Option<String>,
    pools: SmallMap<&'text str, usize>,
}

fn subninja<'thread, 'text>(
    num_threads: usize,
    files: &'thread Files,
    file_pool: &'text FilePool,
    path: String,
    parent_scope: Option<ParentScopeReference<'text>>,
    executor: &rayon::Scope<'thread>,
) -> anyhow::Result<SubninjaResults<'text>>
where
    'text: 'thread,
{
    let path = PathBuf::from(path);
    let top_level_scope = parent_scope.is_none();
    let mut scope = Scope::new(parent_scope);
    if top_level_scope {
        let position = scope.get_and_inc_scope_position();
        scope.rules.insert(
            "phony",
            Rule {
                name: "phony",
                vars: SmallMap::default(),
                scope_position: position,
            },
        );
    }
    let parse_results = trace::scope("parse", || {
        parse(
            num_threads,
            file_pool,
            file_pool.read_file(&path)?,
            &mut scope,
            executor,
        )
    })?;
    let scope = Arc::new(scope);
    let mut subninja_results = parse_results
        .subninjas
        .into_par_iter()
        .map(|sn| {
            let file = canon_path(sn.file.evaluate(&[], &scope, sn.scope_position));
            subninja(
                num_threads,
                files,
                file_pool,
                file,
                Some(ParentScopeReference(scope.clone(), sn.scope_position)),
                executor,
            )
        })
        .collect::<anyhow::Result<Vec<SubninjaResults>>>()?;

    let filename = Arc::new(path);
    let mut results = SubninjaResults::default();

    let builds = parse_results.builds;
    results.builds = trace::scope("add builds", || {
        builds
            .into_par_iter()
            .map(|build| add_build(files, &filename, &scope, build))
            .collect::<anyhow::Result<Vec<graph::Build>>>()
    })?;
    results.pools = parse_results.pools;
    for default in parse_results.defaults.into_iter() {
        let scope = scope.clone();
        results.defaults.extend(default.files.iter().map(|x| {
            let path = canon_path(x.evaluate(&[], &scope, default.scope_position));
            files.id_from_canonical(path)
        }));
    }

    // Only the builddir in the outermost scope is respected
    if top_level_scope {
        let mut build_dir = String::new();
        scope.evaluate(&mut build_dir, "builddir", scope.get_last_scope_position());
        if !build_dir.is_empty() {
            results.builddir = Some(build_dir);
        }
    }

    results.builds.par_extend(
        subninja_results
            .par_iter_mut()
            .flat_map(|x| std::mem::take(&mut x.builds)),
    );
    results.defaults.par_extend(
        subninja_results
            .par_iter_mut()
            .flat_map(|x| std::mem::take(&mut x.defaults)),
    );
    for new_results in subninja_results {
        for (name, depth) in new_results.pools.into_iter() {
            add_pool(&mut results.pools, name, depth)?;
        }
    }

    Ok(results)
}

fn include<'thread, 'text>(
    num_threads: usize,
    file_pool: &'text FilePool,
    path: String,
    scope: &mut Scope<'text>,
    executor: &rayon::Scope<'thread>,
) -> anyhow::Result<ParseResults<'text>>
where
    'text: 'thread,
{
    let path = PathBuf::from(path);
    parse(
        num_threads,
        file_pool,
        file_pool.read_file(&path)?,
        scope,
        executor,
    )
}

fn add_pool<'text>(
    pools: &mut SmallMap<&'text str, usize>,
    name: &'text str,
    depth: usize,
) -> anyhow::Result<()> {
    if let Some(_) = pools.get(name) {
        bail!("duplicate pool {}", name);
    }
    pools.insert(name, depth);
    Ok(())
}

#[derive(Default)]
struct ParseResults<'text> {
    builds: Vec<Build<'text>>,
    defaults: Vec<DefaultStmt<'text>>,
    subninjas: Vec<IncludeOrSubninja<'text>>,
    pools: SmallMap<&'text str, usize>,
}

impl<'text> ParseResults<'text> {
    pub fn merge(&mut self, other: ParseResults<'text>) -> anyhow::Result<()> {
        self.builds.extend(other.builds);
        self.defaults.extend(other.defaults);
        self.subninjas.extend(other.subninjas);
        for (name, depth) in other.pools.into_iter() {
            add_pool(&mut self.pools, name, depth)?;
        }
        Ok(())
    }
}

fn parse<'thread, 'text>(
    num_threads: usize,
    file_pool: &'text FilePool,
    bytes: &'text [u8],
    scope: &mut Scope<'text>,
    executor: &rayon::Scope<'thread>,
) -> anyhow::Result<ParseResults<'text>>
where
    'text: 'thread,
{
    let chunks = parse::split_manifest_into_chunks(bytes, num_threads);

    let receivers = chunks
        .into_par_iter()
        .map(|chunk| {
            let mut parser = parse::Parser::new(chunk);
            parser.read_all()
        })
        .collect::<ParseResult<Vec<Vec<Statement>>>>();

    let Ok(receivers) = receivers else {
        // TODO: Call format_parse_error
        bail!(receivers.unwrap_err().msg);
    };

    let mut results = ParseResults::default();

    results.builds.reserve(
        receivers
            .par_iter()
            .flatten()
            .map(|x| match x {
                Statement::Build(_) => 1,
                _ => 0,
            })
            .sum(),
    );

    for stmt in receivers.into_iter().flatten() {
        match stmt {
            Statement::VariableAssignment(mut variable_assignment) => {
                variable_assignment.scope_position = scope.get_and_inc_scope_position();
                match scope.variables.entry(variable_assignment.name) {
                    Entry::Occupied(mut e) => e.get_mut().push(variable_assignment),
                    Entry::Vacant(e) => {
                        e.insert(vec![variable_assignment]);
                    }
                }
            }
            Statement::Include(i) => trace::scope("include", || -> anyhow::Result<()> {
                let evaluated = canon_path(i.file.evaluate(&[], &scope, i.scope_position));
                let new_results = include(num_threads, file_pool, evaluated, scope, executor)?;
                results.merge(new_results)?;
                Ok(())
            })?,
            Statement::Subninja(mut subninja) => trace::scope("subninja", || {
                subninja.scope_position = scope.get_and_inc_scope_position();
                results.subninjas.push(subninja);
            }),
            Statement::Default(mut default) => {
                default.scope_position = scope.get_and_inc_scope_position();
                results.defaults.push(default);
            }
            Statement::Rule(mut rule) => {
                rule.scope_position = scope.get_and_inc_scope_position();
                match scope.rules.entry(rule.name) {
                    Entry::Occupied(_) => bail!("duplicate rule '{}'", rule.name),
                    Entry::Vacant(e) => e.insert(rule),
                };
            }
            Statement::Build(mut build) => {
                build.scope_position = scope.get_and_inc_scope_position();
                results.builds.push(build);
            }
            Statement::Pool(pool) => {
                add_pool(&mut results.pools, pool.name, pool.depth)?;
            }
        };
    }

    Ok(results)
}

/// State loaded by read().
pub struct State {
    pub graph: graph::Graph,
    pub db: db::Writer,
    pub hashes: graph::Hashes,
    pub default: Vec<Arc<graph::File>>,
    pub pools: SmallMap<String, usize>,
}

/// Load build.ninja/.n2_db and return the loaded build graph and state.
pub fn read(build_filename: &str) -> anyhow::Result<State> {
    let build_filename = canon_path(build_filename);
    let file_pool = FilePool::new();
    let files = Files::new();
    let num_threads = available_parallelism()?.get();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()?;
    let SubninjaResults {
        builds,
        defaults,
        builddir,
        pools,
    } = trace::scope("loader.read_file", || -> anyhow::Result<SubninjaResults> {
        pool.scope(|executor: &rayon::Scope| {
            let mut results = subninja(
                num_threads,
                &files,
                &file_pool,
                build_filename,
                None,
                executor,
            )?;
            trace::scope("sort builds", || {
                results.builds.par_sort_unstable_by_key(|b| b.id.index())
            });
            Ok(results)
        })
    })?;
    drop(pool);
    let mut graph = trace::scope("loader.from_uninitialized_builds_and_files", || {
        Graph::from_uninitialized_builds_and_files(builds, files.into_maps())
    })?;
    let mut hashes = graph::Hashes::default();
    let db = trace::scope("db::open", || {
        let mut db_path = PathBuf::from(".n2_db");
        if let Some(builddir) = &builddir {
            db_path = Path::new(&builddir).join(db_path);
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
        };
        db::open(&db_path, &mut graph, &mut hashes)
    })
    .map_err(|err| anyhow!("load .n2_db: {}", err))?;

    let mut owned_pools = SmallMap::with_capacity(pools.len());
    for pool in pools.iter() {
        owned_pools.insert(pool.0.to_owned(), pool.1);
    }

    Ok(State {
        graph,
        db,
        hashes,
        default: defaults,
        pools: owned_pools,
    })
}
