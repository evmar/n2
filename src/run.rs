use crate::{
    load,
    progress::{DumbConsoleProgress, FancyConsoleProgress, Progress},
    terminal, trace, work,
};
use anyhow::anyhow;
use std::path::Path;

fn build(
    options: work::Options,
    build_filename: String,
    targets: Vec<String>,
    verbose: bool,
) -> anyhow::Result<Option<usize>> {
    let (mut dumb_console, mut fancy_console);
    let progress: &mut dyn Progress = if terminal::use_fancy() {
        fancy_console = FancyConsoleProgress::new(verbose);
        &mut fancy_console
    } else {
        dumb_console = DumbConsoleProgress::new(verbose);
        &mut dumb_console
    };

    let mut state = trace::scope("load::read", || load::read(&build_filename))?;
    let mut work = work::Work::new(
        state.graph,
        state.hashes,
        state.db,
        &options,
        progress,
        state.pools,
    );

    let mut tasks_finished = 0;

    // Attempt to rebuild build.ninja.
    let build_file_target = work.lookup(&build_filename);
    if let Some(target) = build_file_target {
        work.want_file(target)?;
        match trace::scope("work.run", || work.run())? {
            None => return Ok(None),
            Some(0) => {
                // build.ninja already up to date.
                // TODO: this logic is not right in the case where a build has
                // a step that doesn't touch build.ninja.  We should instead
                // verify the specific FileId was updated.
            }
            Some(n) => {
                // Regenerated build.ninja; start over.
                tasks_finished = n;
                state = trace::scope("load::read", || load::read(&build_filename))?;
                work = work::Work::new(
                    state.graph,
                    state.hashes,
                    state.db,
                    &options,
                    progress,
                    state.pools,
                );
            }
        }
    }

    if !targets.is_empty() {
        for name in &targets {
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
        work.progress
            .log("no path specified and no default target; building everything");
        work.want_every_file(build_file_target)?;
    }

    let tasks = trace::scope("work.run", || work.run())?;
    // Include any tasks from initial build in final count of steps.
    Ok(tasks.map(|n| n + tasks_finished))
}

fn default_parallelism() -> anyhow::Result<usize> {
    // Ninja uses available processors + a constant, but I don't think the
    // difference matters too much.
    let par = std::thread::available_parallelism()?;
    Ok(usize::from(par))
}

#[derive(argh::FromArgs)] // this struct generates the flags and --help output
/// n2, a ninja compatible build system
struct Args {
    /// chdir before running
    #[argh(option, short = 'C')]
    chdir: Option<String>,

    /// input build file [default=build.ninja]
    #[argh(option, short = 'f', default = "(\"build.ninja\".into())")]
    build_file: String,

    /// debugging tools
    #[argh(option, short = 'd')]
    debug: Option<String>,

    /// subcommands
    #[argh(option, short = 't')]
    tool: Option<String>,

    /// parallelism [default uses system thread count]
    #[argh(option, short = 'j')] // tododefault_parallelism()")]
    parallelism: Option<usize>,

    /// keep going until at least N failures (0 means infinity) [default=1]
    #[argh(option, short = 'k', default = "1")]
    keep_going: usize,

    /// print version (required by cmake)
    #[argh(switch, hidden_help)]
    version: bool,

    /// compdb flag (required by meson)
    #[allow(dead_code)]
    #[argh(switch, short = 'x', hidden_help)]
    expand_rspfile: bool,

    /// print executed command lines
    #[argh(switch, short = 'v')]
    verbose: bool,

    /// targets to build
    #[argh(positional)]
    targets: Vec<String>,
}

fn run_impl() -> anyhow::Result<i32> {
    let mut fake_ninja_compat = Path::new(&std::env::args().next().unwrap())
        .file_name()
        .unwrap()
        == std::ffi::OsStr::new(&format!("ninja{}", std::env::consts::EXE_SUFFIX));

    let args: Args = argh::from_env();

    let mut options = work::Options {
        parallelism: match args.parallelism {
            Some(p) => p,
            None => default_parallelism()?,
        },
        failures_left: Some(args.keep_going).filter(|&n| n > 0),
        explain: false,
        adopt: false,
    };

    if let Some(dir) = args.chdir {
        let dir = Path::new(&dir);
        std::env::set_current_dir(dir).map_err(|err| anyhow!("chdir {:?}: {}", dir, err))?;
    }

    if let Some(debug) = args.debug {
        match debug.as_str() {
            "ninja_compat" => fake_ninja_compat = true,
            "explain" => options.explain = true,
            "list" => {
                println!("debug tools:");
                println!("  explain  print why each target is considered out of date");
                println!("  trace    generate json performance trace");
                return Ok(1);
            }
            "trace" => trace::open("trace.json")?,
            _ => anyhow::bail!("unknown -d {:?}, use -d list to list", debug),
        }
    }

    if args.version {
        if fake_ninja_compat {
            // CMake requires a particular Ninja version.
            println!("1.10.2");
            return Ok(0);
        } else {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        return Ok(0);
    }

    if let Some(tool) = args.tool {
        match tool.as_str() {
            "list" => {
                println!("subcommands:");
                println!("  (none yet, but see README if you're looking here trying to get CMake to work)");
                return Ok(1);
            }
            "compdb" if fake_ninja_compat => {
                // meson wants to invoke this tool.
                return Ok(0); // do nothing; TODO
            }
            "recompact" if fake_ninja_compat => {
                // CMake unconditionally invokes this tool, yuck.
                return Ok(0); // do nothing
            }
            "restat" if fake_ninja_compat => {
                // CMake invokes this after generating build files; mark build
                // targets as up to date by running the build with "adopt" flag
                // on.
                options.adopt = true;
            }
            _ => {
                anyhow::bail!("unknown -t {:?}, use -t list to list", tool);
            }
        }
    }

    match build(options, args.build_file, args.targets, args.verbose)? {
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
