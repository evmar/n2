extern crate getopts;

use anyhow::anyhow;
use std::path::Path;

use crate::{load, progress::ConsoleProgress, terminal, trace, work};

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
    parallelism: usize,
    regen: bool,
    keep_going: usize,
    target_names: &'a [String],
    build_filename: &'a String,
}

// Build a given set of targets.  If regen is true, build "build.ninja" first if
// possible, and if that build changes build.ninja, then return
// BuildResult::Regen to signal to the caller that we need to start the whole
// build over.
fn build(progress: &mut ConsoleProgress, params: &BuildParams) -> anyhow::Result<BuildResult> {
    let mut state = trace::scope("load::read", || load::read(params.build_filename))?;

    let mut work = work::Work::new(
        &mut state.graph,
        &state.hashes,
        &mut state.db,
        progress,
        params.keep_going,
        state.pools,
        params.parallelism,
    );

    if params.regen {
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

fn run_impl() -> anyhow::Result<i32> {
    let args: Vec<_> = std::env::args().collect();
    let fake_ninja_compat = Path::new(&args[0]).file_name().unwrap()
        == std::ffi::OsStr::new(&format!("ninja{}", std::env::consts::EXE_SUFFIX));

    // Ninja uses available processors + a constant, but I don't think the
    // difference matters too much.
    let mut parallelism = usize::from(std::thread::available_parallelism()?);

    let mut opts = getopts::Options::new();
    opts.optopt("C", "", "chdir before running", "DIR");
    opts.optopt(
        "f",
        "",
        "specify input build file [default=build.ninja]",
        "FILE",
    );
    opts.optopt("d", "debug", "debugging tools", "TOOL");
    opts.optopt("t", "tool", "subcommands", "TOOL");
    opts.optopt(
        "j",
        "",
        &format!("parallelism [default from system={}]", parallelism),
        "NUM",
    );
    opts.optopt(
        "k",
        "",
        "keep going until at least N failures (0 means infinity) [default=1]",
        "N",
    );
    opts.optflag("h", "help", "");
    opts.optflag("v", "verbose", "print executed command lines");
    if fake_ninja_compat {
        opts.optflag("", "version", "print fake ninja version");
    }
    let matches = opts.parse(&args[1..])?;
    if matches.opt_present("h") {
        println!("{}", opts.usage("usage: n2 [target]"));
        return Ok(1);
    }

    if fake_ninja_compat && matches.opt_present("version") {
        println!("1.10.2");
        return Ok(0);
    }

    if let Some(debug) = matches.opt_str("d") {
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

    if let Some(tool) = matches.opt_str("t") {
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

    if let Some(parallelism_flag) = matches.opt_str("j") {
        match parallelism_flag.parse::<usize>() {
            Ok(n) => parallelism = n,
            Err(e) => anyhow::bail!("invalid -j {:?}: {:?}", parallelism, e),
        }
    }

    let keep_going = match matches.opt_str("k") {
        Some(val) => match val.parse::<usize>() {
            Ok(n) => n,
            Err(e) => anyhow::bail!("invalid -k {:?}: {:?}", val, e),
        },
        None => 1,
    };

    if let Some(dir) = matches.opt_str("C") {
        let dir = Path::new(&dir);
        std::env::set_current_dir(dir).map_err(|err| anyhow!("chdir {:?}: {}", dir, err))?;
    }

    let mut build_filename = "build.ninja".to_string();
    if let Some(name) = matches.opt_str("f") {
        build_filename = name;
    }

    let mut progress = ConsoleProgress::new(matches.opt_present("v"), terminal::use_fancy());

    // Build once with regen=true, and if the result says we regenerated the
    // build file, reload and build everything a second time.

    let mut params = BuildParams {
        parallelism,
        regen: true,
        keep_going,
        target_names: &matches.free,
        build_filename: &build_filename,
    };
    let mut result = build(&mut progress, &params)?;
    if let BuildResult::Regen = result {
        params.regen = false;
        result = build(&mut progress, &params)?;
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
            println!("n2: ran {} tasks, now up to date", n);
        }
    }

    Ok(0)
}

pub fn run() -> anyhow::Result<i32> {
    let res = run_impl();
    trace::close();
    res
}
