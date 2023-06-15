use crate::{load, progress::ConsoleProgress, terminal, trace, work};
use anyhow::anyhow;
use std::path::Path;

// The result of starting a build.
enum BuildResult {
    /// A build task failed.
    Failed,
    /// Regenerated build.ninja rather than the requested build.  The caller
    /// must reload build.ninja to continue with building.
    Regen,
    /// Build succeeded, and the number is the count of executed tasks.
    Success(usize),
}

struct BuildParams<'a> {
    options: work::Options,
    target_names: &'a [String],
    build_filename: &'a String,
}

// Build a given set of targets.  If regen is true, build "build.ninja" first if
// possible, and if that build changes build.ninja, then return
// BuildResult::Regen to signal to the caller that we need to start the whole
// build over.
fn build(
    progress: &mut ConsoleProgress,
    params: &BuildParams,
    regen: bool,
) -> anyhow::Result<BuildResult> {
    let mut state = trace::scope("load::read", || load::read(params.build_filename))?;

    let mut work = work::Work::new(
        &mut state.graph,
        &state.hashes,
        &mut state.db,
        &params.options,
        progress,
        state.pools,
    );

    if regen {
        if let Some(target) = work.build_ninja_fileid(params.build_filename) {
            // Attempt to rebuild build.ninja.
            work.want_fileid(target)?;
            match trace::scope("work.run", || work.run())? {
                None => return Ok(BuildResult::Failed),
                Some(0) => {
                    // build.ninja already up to date.
                }
                Some(_) => {
                    // Regenerated build.ninja; start over.
                    return Ok(BuildResult::Regen);
                }
            }
        }
    }

    if !params.target_names.is_empty() {
        for name in params.target_names {
            work.want_file(name)?;
        }
    } else if !state.default.is_empty() {
        for target in state.default {
            work.want_fileid(target)?;
        }
    } else {
        anyhow::bail!("no path specified and no default");
    }

    Ok(match trace::scope("work.run", || work.run())? {
        None => BuildResult::Failed,
        Some(n) => BuildResult::Success(n),
    })
}

fn default_parallelism() -> anyhow::Result<usize> {
    // Ninja uses available processors + a constant, but I don't think the
    // difference matters too much.
    let par = std::thread::available_parallelism()?;
    Ok(usize::from(par))
}

#[derive(argh::FromArgs)] // this struct generates the flags and --help output
/// n2, a ninja compatible build system
struct Opts {
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

    /// print executed command lines
    #[argh(switch, short = 'v')]
    verbose: bool,

    /// targets to build
    #[argh(positional)]
    targets: Vec<String>,
}

fn run_impl() -> anyhow::Result<i32> {
    let args: Vec<_> = std::env::args().collect();
    let fake_ninja_compat = Path::new(&args[0]).file_name().unwrap()
        == std::ffi::OsStr::new(&format!("ninja{}", std::env::consts::EXE_SUFFIX));

    let opts: Opts = argh::from_env();

    let params = BuildParams {
        options: work::Options {
            parallelism: match opts.parallelism {
                Some(p) => p,
                None => default_parallelism()?,
            },
            keep_going: opts.keep_going,
        },
        target_names: &opts.targets,
        build_filename: &opts.build_file,
    };

    if fake_ninja_compat && opts.version {
        println!("1.10.2");
        return Ok(0);
    }

    if let Some(debug) = opts.debug {
        match debug.as_str() {
            "list" => {
                println!("debug tools:");
                println!("  trace  generate json performance trace");
                return Ok(1);
            }
            "trace" => trace::open("trace.json")?,
            _ => anyhow::bail!("unknown -d {:?}, use -d list to list", debug),
        }
    }

    if let Some(tool) = opts.tool {
        match tool.as_str() {
            "list" => {
                println!("subcommands:");
                println!("  (none yet, but see README if you're looking here trying to get CMake to work)");
                return Ok(1);
            }
            _ => {
                if fake_ninja_compat {
                    return Ok(0);
                }
                anyhow::bail!("unknown -t {:?}, use -t list to list", tool);
            }
        }
    }

    if let Some(dir) = opts.chdir {
        let dir = Path::new(&dir);
        std::env::set_current_dir(dir).map_err(|err| anyhow!("chdir {:?}: {}", dir, err))?;
    }

    let mut progress: ConsoleProgress = ConsoleProgress::new(opts.verbose, terminal::use_fancy());

    // Build once with regen=true, and if the result says we regenerated the
    // build file, reload and build everything a second time.
    let mut result: BuildResult = build(&mut progress, &params, true)?;
    if let BuildResult::Regen = result {
        result = build(&mut progress, &params, false)?;
    }

    match result {
        BuildResult::Regen => panic!("should not happen"),
        BuildResult::Failed => {
            // Don't print any summary, the failing task is enough info.
            return Ok(1);
        }
        BuildResult::Success(0) => {
            // Special case: don't print numbers when no work done.
            println!("n2: no work to do");
        }
        BuildResult::Success(n) => {
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
