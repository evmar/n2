//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::canon_path, db, densemap::Index, eval::{self, EvalPart, EvalString}, file_pool::FilePool, graph::{self, stat, BuildId, FileId, Graph, RspFile}, parse::{self, ClumpOrInclude, DefaultStmt, IncludeOrSubninja, Rule, Statement, VariableAssignment}, scanner::{self, ParseResult}, smallmap::SmallMap, trace
};
use anyhow::{anyhow, bail};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::{
    borrow::Cow, default, path::Path, sync::{atomic::AtomicUsize, mpsc::TryRecvError}, time::Instant
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

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct ScopePosition(pub usize);

impl ScopePosition {
    pub fn add(&self, other: ScopePosition) -> ScopePosition {
        ScopePosition(self.0 + other.0)
    }
}

#[derive(Debug)]
pub struct ParentScopeReference<'text>(pub Arc<Scope<'text>>, pub ScopePosition);

#[derive(Debug)]
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
    scope: &Scope,
    b: &mut graph::Build,
    base_position: ScopePosition,
) -> anyhow::Result<()> {
    b.scope_position.0 += base_position.0;
    let ins: Vec<_> = b.ins.unevaluated
        .iter()
        .map(|x| canon_path(x.evaluate(&[&b.bindings], scope, b.scope_position)))
        .collect();
    let outs: Vec<_> = b.outs.unevaluated
        .iter()
        .map(|x| canon_path(x.evaluate(&[&b.bindings], scope, b.scope_position)))
        .collect();

    let rule = match scope.get_rule(&b.rule, b.scope_position) {
        Some(r) => r,
        None => bail!("unknown rule {:?}", b.rule),
    };

    let implicit_vars = BuildImplicitVars {
        explicit_ins: &ins[..b.ins.explicit],
        explicit_outs: &outs[..b.outs.explicit],
    };

    // temp variable in order to not move all of b into the closure
    let build_vars = &b.bindings;
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

    b.ins.ids = ins.into_iter()
        .map(|x| files.id_from_canonical(x))
        .collect();
    b.outs.ids = outs.into_iter()
        .map(|x| files.id_from_canonical(x))
        .collect();

    b.cmdline = cmdline;
    b.desc = desc;
    b.depfile = depfile;
    b.parse_showincludes = parse_showincludes;
    b.rspfile = rspfile;
    b.pool = pool;

    Ok(())
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
    pub builds: Vec<Vec<graph::Build>>,
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
        scope.rules.insert(
            "phony",
            Rule {
                name: "phony",
                vars: SmallMap::default(),
                scope_position: ScopePosition(0),
            },
        );
    }
    let filename = Arc::new(path);
    let mut parse_results = trace::scope("parse", || {
        parse(
            &filename,
            num_threads,
            file_pool,
            file_pool.read_file(&filename)?,
            &mut scope,
            // to account for the phony rule
            if top_level_scope { ScopePosition(1) } else { ScopePosition(0) },
            executor,
        )
    })?;

    for clump in &mut parse_results {
        for mut rule in std::mem::take(&mut clump.rules).into_iter() {
            rule.scope_position = rule.scope_position.add(clump.base_position);
            match scope.rules.entry(rule.name) {
                Entry::Occupied(_) => bail!("duplicate rule '{}'", rule.name),
                Entry::Vacant(e) => e.insert(rule),
            };
        }
    }

    let scope = Arc::new(scope);
    let mut subninja_results = parse_results.par_iter()
        .flat_map(|x| x.subninjas.par_iter().zip(rayon::iter::repeatn(x.base_position, x.subninjas.len())))
        .map(|(sn, base_position)| {
            let file = canon_path(sn.file.evaluate(&[], &scope, sn.scope_position.add(base_position)));
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

    let mut results = SubninjaResults::default();

    for clump in &parse_results {
        for pool in &clump.pools {
            add_pool(&mut results.pools, pool.name, pool.depth)?;
        }
        for default in clump.defaults.iter() {
            let scope = scope.clone();
            results.defaults.extend(default.files.iter().map(|x| {
                let path = canon_path(x.evaluate(&[], &scope, default.scope_position.add(clump.base_position)));
                files.id_from_canonical(path)
            }));
        }
    }

    results.builds = Vec::new();
    results.builds.push(trace::scope("add builds", || {
        parse_results
            .par_iter_mut()
            .flat_map(|x| {
                let num_builds = x.builds.len();
                std::mem::take(&mut x.builds).into_par_iter().zip(rayon::iter::repeatn(x.base_position, num_builds))
            })
            .map(|(mut build, base_position)| -> anyhow::Result<graph::Build> {
                add_build(files, &scope, &mut build, base_position)?;
                Ok(build)
            })
            .collect::<anyhow::Result<Vec<graph::Build>>>()
    })?);
    trace::write_instant("Right after add_builds");
    
    // Only the builddir in the outermost scope is respected
    if top_level_scope {
        let mut build_dir = String::new();
        scope.evaluate(&mut build_dir, "builddir", ScopePosition(parse_results.iter().map(|x| x.used_scope_positions).sum::<usize>()));
        if !build_dir.is_empty() {
            results.builddir = Some(build_dir);
        }
    }

    trace::scope("Extend subninja results", || -> anyhow::Result<()> {
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
        Ok(())
    })?;

    trace::write_instant("End of subninja");
    Ok(results)
}

fn include<'thread, 'text>(
    filename: &Arc<PathBuf>,
    num_threads: usize,
    file_pool: &'text FilePool,
    path: String,
    scope: &mut Scope<'text>,
    clump_base_position: ScopePosition,
    executor: &rayon::Scope<'thread>,
) -> anyhow::Result<Vec<parse::Clump<'text>>>
where
    'text: 'thread,
{
    let path = PathBuf::from(path);
    parse(
        filename,
        num_threads,
        file_pool,
        file_pool.read_file(&path)?,
        scope,
        clump_base_position,
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

fn parse<'thread, 'text>(
    filename: &Arc<PathBuf>,
    num_threads: usize,
    file_pool: &'text FilePool,
    bytes: &'text [u8],
    scope: &mut Scope<'text>,
    mut clump_base_position: ScopePosition,
    executor: &rayon::Scope<'thread>,
) -> anyhow::Result<Vec<parse::Clump<'text>>>
where
    'text: 'thread,
{
    let chunks = parse::split_manifest_into_chunks(bytes, num_threads);

    let statements: ParseResult<Vec<Vec<ClumpOrInclude>>> = chunks
        .into_par_iter()
        .map(|chunk| {
            let mut parser = parse::Parser::new(chunk, filename.clone());
            parser.read_clumps()
        }).collect();

    let Ok(mut statements) = statements else {
        // TODO: Call format_parse_error
        bail!(statements.unwrap_err().msg);
    };

    let mut results = Vec::new();

    let start = Instant::now();
    for stmt in statements.into_iter().flatten() {
        match stmt {
            ClumpOrInclude::Clump(mut clump) => {
                // Variable assignemnts must be added to the scope now, because
                // they may be referenced by a later include. Everything else
                // can be handled after all parsing is done.
                for mut variable_assignment in std::mem::take(&mut clump.assignments).into_iter() {
                    variable_assignment.scope_position.0 += clump_base_position.0;
                    match scope.variables.entry(variable_assignment.name) {
                        Entry::Occupied(mut e) => e.get_mut().push(variable_assignment),
                        Entry::Vacant(e) => {
                            e.insert(vec![variable_assignment]);
                        }
                    }
                }
                clump.base_position = clump_base_position;
                clump_base_position.0 += clump.used_scope_positions;
                results.push(clump);
            },
            ClumpOrInclude::Include(i) => {
                trace::scope("include", || -> anyhow::Result<()> {
                    let evaluated = canon_path(i.evaluate(&[], &scope, clump_base_position));
                    let mut new_results = include(filename, num_threads, file_pool, evaluated, scope, clump_base_position, executor)?;
                    clump_base_position.0 += new_results.iter().map(|c| c.used_scope_positions).sum::<usize>();
                    // Things will be out of order here, but we don't care about
                    // order for builds, defaults, subninjas, or pools, as long
                    // as their scope_position is correct.
                    results.append(&mut new_results);
                    clump_base_position.0 += 1;
                    Ok(())
                })?;
            },
        }
    }
    trace::write_complete("parse loop", start, Instant::now());

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
    let (SubninjaResults {
        defaults,
        builddir,
        pools,
        ..
    }, builds) = trace::scope("loader.read_file", || -> anyhow::Result<(SubninjaResults, Vec<graph::Build>)> {
        pool.scope(|executor: &rayon::Scope| {
            let mut results = subninja(
                num_threads,
                &files,
                &file_pool,
                build_filename,
                None,
                executor,
            )?;
            trace::scope("initialize builds", || {
                let mut builds = Vec::with_capacity(results.builds.iter().map(|x| x.len()).sum());
                for build_vec in &mut results.builds {
                    builds.append(build_vec);
                }
                builds.par_iter_mut().enumerate().try_for_each(|(id, build)| {
                    build.id = BuildId::from(id);
                    graph::Graph::initialize_build(build)
                })?;
                Ok((results, builds))
            })
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
