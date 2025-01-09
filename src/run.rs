//! Command line argument parsing and initial build invocation.

use crate::{
    load, progress::Progress, progress_dumb::DumbConsoleProgress,
    progress_fancy::FancyConsoleProgress, terminal, trace, work,
};
use anyhow::anyhow;

/// Arguments to start a build, after parsing all the command line etc.
#[derive(Default)]
struct BuildArgs {
    fake_ninja_compat: bool,
    options: work::Options,
    build_filename: Option<String>,
    targets: Vec<String>,
    verbose: bool,
}

/// Returns the number of completed tasks on a successful build.
fn build(args: BuildArgs) -> anyhow::Result<Option<usize>> {
    let (dumb_console, fancy_console);
    let progress: &dyn Progress = if terminal::use_fancy() {
        fancy_console = FancyConsoleProgress::new(args.verbose);
        &fancy_console
    } else {
        dumb_console = DumbConsoleProgress::new(args.verbose);
        &dumb_console
    };

    let build_filename = args.build_filename.as_deref().unwrap_or("build.ninja");
    let mut state = trace::scope("load::read", || load::read(build_filename))?;
    let mut work = work::Work::new(
        state.graph,
        state.hashes,
        state.db,
        &args.options,
        progress,
        state.pools,
    );

    let mut tasks_run = 0;

    // Attempt to rebuild build.ninja.
    let build_file_target = work.lookup(&build_filename);
    if let Some(target) = build_file_target {
        work.want_file(target)?;
        if !trace::scope("work.run", || work.run())? {
            return Ok(None);
        }
        if work.tasks_run == 0 {
            // build.ninja already up to date.
            // TODO: this logic is not right in the case where a build has
            // a step that doesn't touch build.ninja.  We should instead
            // verify the specific FileId was updated.
        } else {
            // Regenerated build.ninja; start over.
            tasks_run = work.tasks_run;
            state = trace::scope("load::read", || load::read(&build_filename))?;
            work = work::Work::new(
                state.graph,
                state.hashes,
                state.db,
                &args.options,
                progress,
                state.pools,
            );
        }
    }

    if !args.targets.is_empty() {
        for name in &args.targets {
            let target = work
                .lookup(name)
                .ok_or_else(|| anyhow::anyhow!("unknown path requested: {:?}", name))?;
            if Some(target) == build_file_target {
                // Already built above.
                continue;
            }
            work.want_file(target)?;
        }
    } else if !state.default.is_empty() {
        for target in state.default {
            work.want_file(target)?;
        }
    } else {
        work.want_every_file(build_file_target)?;
    }

    if !trace::scope("work.run", || work.run())? {
        return Ok(None);
    }
    // Include any tasks from initial build in final count of steps.
    Ok(Some(tasks_run + work.tasks_run))
}

fn default_parallelism() -> anyhow::Result<usize> {
    // Ninja uses available processors + a constant, but I don't think the
    // difference matters too much.
    let par = std::thread::available_parallelism()?;
    Ok(usize::from(par))
}

/// Run a tool as specified by the `-t` flag`.
fn subtool(args: &mut BuildArgs, tool: &str) -> anyhow::Result<Option<i32>> {
    match tool {
        "list" => {
            println!("subcommands:");
            println!(
                "  (none yet, but see README if you're looking here trying to get CMake to work)"
            );
            return Ok(Some(1));
        }
        "recompact" if args.fake_ninja_compat => {
            // CMake unconditionally invokes this tool, yuck.
            return Ok(Some(0)); // do nothing
        }
        "restat" if args.fake_ninja_compat => {
            // CMake invokes this after generating build files; mark build
            // targets as up to date by running the build with "adopt" flag
            // on.
            args.options.adopt = true;
        }
        _ => {
            anyhow::bail!("unknown -t {:?}, use -t list to list", tool);
        }
    }
    Ok(None)
}

/// Run a debug tool as specified by the `-d` flag.
fn debugtool(args: &mut BuildArgs, tool: &str) -> anyhow::Result<Option<i32>> {
    match tool {
        "list" => {
            println!("debug tools:");
            println!("  ninja_compat  enable ninja quirks compatibility mode");
            println!("  explain       print why each target is considered out of date");
            println!("  trace         generate json performance trace");
            return Ok(Some(1));
        }

        "ninja_compat" => args.fake_ninja_compat = true,
        "explain" => args.options.explain = true,
        "trace" => trace::open("trace.json")?,

        _ => anyhow::bail!("unknown -d {:?}, use -d list to list", tool),
    }
    Ok(None)
}

fn parse_args() -> anyhow::Result<Result<BuildArgs, i32>> {
    let mut args = BuildArgs::default();
    args.fake_ninja_compat = std::path::Path::new(&std::env::args().next().unwrap())
        .file_name()
        .unwrap()
        == std::ffi::OsStr::new(&format!("ninja{}", std::env::consts::EXE_SUFFIX));

    use lexopt::prelude::*;
    let mut parser = lexopt::Parser::from_env();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                println!(
                    "n2: a ninja-compatible build tool
usage: n2 [options] [targets...]

options:
-C dir   chdir before running
-f file  input build file [default: build.ninja]
-j N     parallelism [default: use system thread count]
-k N     keep going until at least N failures [default: 1]
-v       print executed command lines

-t tool  tools (`-t list` to list)
-d tool  debugging tools (use `-d list` to list)
"
                );
                return Ok(Err(0));
            }

            Short('C') => {
                let dir = parser.value()?;
                std::env::set_current_dir(&dir)
                    .map_err(|err| anyhow!("chdir {:?}: {}", dir, err))?;
            }

            Short('f') => args.build_filename = Some(parser.value()?.to_string_lossy().into()),
            Short('t') => {
                if let Some(exit) = subtool(&mut args, &*parser.value()?.to_string_lossy())? {
                    return Ok(Err(exit));
                }
            }
            Short('d') => {
                if let Some(exit) = debugtool(&mut args, &*parser.value()?.to_string_lossy())? {
                    return Ok(Err(exit));
                }
            }
            Short('j') => args.options.parallelism = parser.value()?.parse()?,
            Short('k') => args.options.failures_left = Some(parser.value()?.parse()?),
            Short('v') => args.verbose = true,

            Long("version") => {
                if args.fake_ninja_compat {
                    // CMake requires a particular Ninja version.
                    println!("1.10.2");
                } else {
                    println!("{}", env!("CARGO_PKG_VERSION"));
                }
                return Ok(Err(0));
            }

            Value(arg) => args.targets.push(arg.to_string_lossy().into()),

            _ => anyhow::bail!("{}", arg.unexpected()),
        }
    }

    if args.options.parallelism == 0 {
        args.options.parallelism = default_parallelism()?;
    }

    Ok(Ok(args))
}

fn run_impl() -> anyhow::Result<i32> {
    let args = match parse_args()? {
        Ok(args) => args,
        Err(exit) => return Ok(exit),
    };

    match build(args)? {
        None => {
            // Don't print any summary, the failing task is enough info.
            return Ok(1);
        }
        Some(0) => {
            // Special case: don't print numbers when no work done.
            println!("n2: no work to do");
        }
        Some(n) => {
            println!(
                "n2: ran {} task{}, now up to date",
                n,
                if n == 1 { "" } else { "s" }
            );
        }
    }

    Ok(0)
}

pub fn run() -> anyhow::Result<i32> {
    let res = run_impl();
    trace::close();
    res
}
