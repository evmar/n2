//! Graph loading: runs .ninja parsing and constructs the build graph from it.

use crate::{
    canon::canon_path,
    db,
    file_pool::FilePool,
    graph::{self, BuildId, Graph, GraphFiles},
    parse::{self, Clump, ClumpOrInclude, Rule, VariableAssignment},
    scanner::{format_parse_error, ParseResult},
    smallmap::SmallMap,
    trace,
};
use anyhow::{anyhow, bail};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::{collections::hash_map::Entry, sync::Arc, thread::available_parallelism};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct ScopePosition(pub usize);

impl ScopePosition {
    pub fn add(&self, other: ScopePosition) -> ScopePosition {
        ScopePosition(self.0 + other.0)
    }
    pub fn add_usize(&self, other: usize) -> ScopePosition {
        ScopePosition(self.0 + other)
    }
}

#[derive(Debug)]
pub struct ParentScopeReference(pub Arc<Scope>, pub ScopePosition);

#[derive(Debug)]
pub struct Scope {
    parent: Option<ParentScopeReference>,
    rules: FxHashMap<String, Rule>,
    variables: FxHashMap<String, Vec<VariableAssignment>>,
    next_free_position: ScopePosition,
}

impl Scope {
    pub fn new(parent: Option<ParentScopeReference>) -> Self {
        Self {
            parent,
            rules: FxHashMap::default(),
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

    pub fn get_rule(&self, name: &str, position: ScopePosition) -> Option<&Rule> {
        match self.rules.get(name) {
            Some(rule) if rule.scope_position.0 < position.0 => Some(rule),
            Some(_) | None => self
                .parent
                .as_ref()
                .map(|p| p.0.get_rule(name, p.1))
                .flatten(),
        }
    }

    pub fn evaluate(&self, result: &mut String, varname: &str, position: ScopePosition) {
        if let Some(variables) = self.variables.get(varname) {
            let i = variables.binary_search_by_key(&position, |x| x.scope_position);
            let i = match i {
                Ok(i) => std::cmp::max(i, 1) - 1,
                Err(i) => std::cmp::min(i, variables.len() - 1),
            };
            if variables[i].scope_position.0 < position.0 {
                variables[i].evaluate(result, &self);
                return;
            }
            // We couldn't find a variable assignment before the input
            // position, so check the parent scope if there is one.
        }
        if let Some(parent) = &self.parent {
            parent.0.evaluate(result, varname, parent.1);
        }
    }
}

fn evaluate_build_files<'text>(
    files: &GraphFiles,
    scope: Arc<Scope>,
    b: &mut graph::Build,
    base_position: ScopePosition,
) -> anyhow::Result<()> {
    b.scope_position.0 += base_position.0;
    let num_outs = b.outs.num_outs();
    b.outs.ids = b.unevaluated_outs_and_ins[..num_outs]
        .iter()
        .map(|x| {
            files.id_from_canonical(canon_path(x.evaluate(
                &[&b.bindings],
                &scope,
                b.scope_position,
            )))
        })
        .collect();
    b.ins.ids = b.unevaluated_outs_and_ins[num_outs..]
        .iter()
        .map(|x| {
            files.id_from_canonical(canon_path(x.evaluate(
                &[&b.bindings],
                &scope,
                b.scope_position,
            )))
        })
        .collect();
    // The unevaluated values actually have a lifetime of 'text, not 'static,
    // so clear them so they don't accidentally get used later.
    b.unevaluated_outs_and_ins.clear();
    b.unevaluated_outs_and_ins.shrink_to_fit();
    b.scope = Some(scope);

    Ok(())
}

#[derive(Default)]
struct SubninjaResults<'text> {
    clumps: Vec<Clump<'text>>,
    builddir: Option<String>,
}

fn subninja<'thread, 'text>(
    num_threads: usize,
    files: &'thread GraphFiles,
    file_pool: &'text FilePool,
    path: String,
    parent_scope: Option<ParentScopeReference>,
) -> anyhow::Result<SubninjaResults<'text>>
where
    'text: 'thread,
{
    let path = PathBuf::from(path);
    let top_level_scope = parent_scope.is_none();
    let mut scope = Scope::new(parent_scope);
    if top_level_scope {
        scope.rules.insert(
            "phony".to_owned(),
            Rule {
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
            if top_level_scope {
                ScopePosition(1)
            } else {
                ScopePosition(0)
            },
        )
    })?;

    let scope = Arc::new(scope);

    for clump in &mut parse_results {
        let base_position = clump.base_position;
        for default in clump.defaults.iter_mut() {
            let scope = scope.clone();
            default.evaluated = default
                .files
                .iter()
                .map(|x| {
                    let path = canon_path(x.evaluate(
                        &[],
                        &scope,
                        default.scope_position.add(base_position),
                    ));
                    files.id_from_canonical(path)
                })
                .collect();
        }
    }

    trace::scope("evaluate builds' files", || -> anyhow::Result<()> {
        parse_results
            .par_iter_mut()
            .flat_map(|x| {
                let num_builds = x.builds.len();
                x.builds
                    .par_iter_mut()
                    .zip(rayon::iter::repeatn(x.base_position, num_builds))
            })
            .try_for_each(|(mut build, base_position)| -> anyhow::Result<()> {
                evaluate_build_files(files, scope.clone(), &mut build, base_position)
            })
    })?;

    // The unevaluated values of scoped variables have a lifetime of 'static
    // for simplicity in the code, but in actuality their lifetime is 'text.
    // We need to evaluate all the variables before the lifetime of 'text ends.
    scope
        .variables
        .par_iter()
        .flat_map(|x| x.1.par_iter())
        .for_each(|x| {
            x.pre_evaluate(&scope);
        });

    let mut subninja_results = parse_results
        .par_iter()
        .flat_map(|x| {
            x.subninjas
                .par_iter()
                .zip(rayon::iter::repeatn(x.base_position, x.subninjas.len()))
        })
        .map(|(sn, base_position)| -> anyhow::Result<Vec<Clump>> {
            let position = sn.scope_position.add(base_position);
            let file = canon_path(sn.file.evaluate(&[], &scope, position));
            Ok(subninja(
                num_threads,
                files,
                file_pool,
                file,
                Some(ParentScopeReference(scope.clone(), position)),
            )?
            .clumps)
        })
        .collect::<anyhow::Result<Vec<Vec<Clump<'text>>>>>()?;

    for subninja_result in &mut subninja_results {
        parse_results.append(subninja_result);
    }

    // Only the builddir in the outermost scope is respected
    let build_dir = if top_level_scope {
        let mut build_dir = String::new();
        scope.evaluate(&mut build_dir, "builddir", ScopePosition(usize::MAX));
        if !build_dir.is_empty() {
            Some(build_dir)
        } else {
            None
        }
    } else {
        None
    };

    Ok(SubninjaResults {
        clumps: parse_results,
        builddir: build_dir,
    })
}

fn include<'thread, 'text>(
    filename: &Arc<PathBuf>,
    num_threads: usize,
    file_pool: &'text FilePool,
    path: String,
    scope: &mut Scope,
    clump_base_position: ScopePosition,
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
    )
}

fn parse<'thread, 'text>(
    filename: &Arc<PathBuf>,
    num_threads: usize,
    file_pool: &'text FilePool,
    bytes: &'text [u8],
    scope: &mut Scope,
    mut clump_base_position: ScopePosition,
) -> anyhow::Result<Vec<parse::Clump<'text>>>
where
    'text: 'thread,
{
    let chunks = parse::split_manifest_into_chunks(bytes, num_threads);

    let statements: ParseResult<Vec<Vec<ClumpOrInclude>>> = chunks
        .par_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut parser = parse::Parser::new(chunk, filename.clone(), i);
            parser.read_clumps()
        })
        .collect();

    let Ok(statements) = statements else {
        let err = statements.unwrap_err();
        let ofs = chunks[..err.chunk_index].iter().map(|x| x.len()).sum();
        bail!(format_parse_error(
            ofs,
            chunks[err.chunk_index],
            filename,
            err
        ));
    };

    let mut num_rules = 0;
    let mut num_variables = 0;
    let mut num_clumps = 0;
    for clumps in &statements {
        num_clumps += clumps.len();
        for clump_or_include in clumps {
            if let ClumpOrInclude::Clump(clump) = clump_or_include {
                num_rules += clump.rules.len();
                num_variables += clump.assignments.len();
            }
        }
    }

    scope.rules.reserve(num_rules);
    scope.variables.reserve(num_variables);

    let mut results = Vec::with_capacity(num_clumps);

    for stmt in statements.into_iter().flatten() {
        match stmt {
            ClumpOrInclude::Clump(mut clump) => {
                // Variable assignments must be added to the scope now, because
                // they may be referenced by a later include. Also add rules
                // while we're at it, to avoid some copies later on.
                let rules = std::mem::take(&mut clump.rules);
                let assignments = std::mem::take(&mut clump.assignments);
                let scope_rules = &mut scope.rules;
                let scope_variables = &mut scope.variables;
                rayon::join(
                    || {
                        for (name, mut variable_assignment) in assignments.into_iter() {
                            variable_assignment.scope_position.0 += clump_base_position.0;
                            match scope_variables.entry(name) {
                                Entry::Occupied(mut e) => e.get_mut().push(variable_assignment),
                                Entry::Vacant(e) => {
                                    e.insert(vec![variable_assignment]);
                                }
                            }
                        }
                    },
                    || -> anyhow::Result<()> {
                        for (name, mut rule) in rules.into_iter() {
                            rule.scope_position.0 += clump_base_position.0;
                            match scope_rules.entry(name) {
                                Entry::Occupied(e) => bail!("duplicate rule '{}'", e.key()),
                                Entry::Vacant(e) => {
                                    e.insert(rule);
                                }
                            }
                        }
                        Ok(())
                    },
                )
                .1?;
                clump.base_position = clump_base_position;
                clump_base_position.0 += clump.used_scope_positions;
                results.push(clump);
            }
            ClumpOrInclude::Include(i) => {
                trace::scope("include", || -> anyhow::Result<()> {
                    let evaluated = canon_path(i.evaluate(&[], &scope, clump_base_position));
                    let mut new_results = include(
                        filename,
                        num_threads,
                        file_pool,
                        evaluated,
                        scope,
                        clump_base_position,
                    )?;
                    clump_base_position = new_results
                        .last()
                        .map(|c| c.base_position.add_usize(c.used_scope_positions))
                        .unwrap_or(clump_base_position);
                    results.append(&mut new_results);
                    Ok(())
                })?;
            }
        }
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
    let files = GraphFiles::default();
    let num_threads = available_parallelism()?.get();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()?;
    let (defaults, builddir, pools, builds) =
        trace::scope("loader.read_file", || -> anyhow::Result<_> {
            pool.scope(|_| {
                let mut results = subninja(num_threads, &files, &file_pool, build_filename, None)?;

                let mut pools = SmallMap::default();
                let mut defaults = Vec::new();
                let mut num_builds = 0;
                trace::scope("add pools and defaults", || -> anyhow::Result<()> {
                    for clump in &mut results.clumps {
                        for pool in &clump.pools {
                            if !pools.insert_if_absent(pool.name.to_owned(), pool.depth) {
                                bail!("duplicate pool {}", pool.name);
                            }
                        }
                        for default in &mut clump.defaults {
                            defaults.append(&mut default.evaluated);
                        }
                        num_builds += clump.builds.len();
                    }
                    Ok(())
                })?;
                let mut builds = trace::scope("allocate and concat builds", || {
                    let mut builds = Vec::with_capacity(num_builds);
                    for clump in &mut results.clumps {
                        builds.append(&mut clump.builds);
                    }
                    builds
                });
                let builddir = results.builddir.take();
                drop(results);
                // Turns out munmap is rather slow, unmapping the android ninja
                // files takes ~150ms. Do this in parallel with initialize_build.
                rayon::spawn(move || {
                    drop(file_pool);
                });
                trace::scope("initialize builds", move || {
                    builds
                        .par_iter_mut()
                        .enumerate()
                        .try_for_each(|(id, build)| {
                            build.id = BuildId::from(id);
                            graph::Graph::initialize_build(build)
                        })?;
                    Ok((defaults, builddir, pools, builds))
                })
            })
        })?;

    let mut graph = Graph::new(builds, files)?;
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

    Ok(State {
        graph,
        db,
        hashes,
        default: defaults,
        pools,
    })
}
