extern crate getopts;

use anyhow::anyhow;
use std::path::Path;

use crate::{load, progress::ConsoleProgress, trace, work};

// The result of starting a build.
enum BuildResult {
    /// A build task failed.
    Failed,
    /// Renerated build.ninja rather than the requested build.  The caller must
    /// reload build.ninja to continue with building.
    Regen,
    /// Build succeeded, and the number is the count of executed tasks.
    Success(usize),
}

// Build a given set of targets.  If regen is true, build "build.ninja" first if
// possible, and if that build changes build.ninja, then return
// BuildResult::Regen to signal to the caller that we need to start the whole
// build over.
fn build(
    progress: &mut ConsoleProgress,
    parallelism: usize,
    regen: bool,
    target_names: &[String],
) -> anyhow::Result<BuildResult> {
    let mut state = trace::scope("load::read", load::read)?;

    let mut work = work::Work::new(
        &mut state.graph,
        &state.hashes,
        &mut state.db,
        progress,
        state.pools,
        parallelism,
    );

    if regen {
        if let Some(target) = work.build_ninja_fileid() {
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

    if !target_names.is_empty() {
        for name in target_names {
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

#[cfg(unix)]
fn use_fancy_terminal() -> bool {
    unsafe {
        libc::isatty(/* stdout */ 1) == 1
    }
}

#[cfg(windows)]
fn use_fancy_terminal() -> bool {
    unsafe {
        let handle = winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_OUTPUT_HANDLE);
        let mut out = 0;
        // Note: GetConsoleMode itself fails when not attached to a console.
        winapi::um::consoleapi::GetConsoleMode(handle, &mut out) != 0
    }
}

fn run_impl() -> anyhow::Result<i32> {
    let args: Vec<_> = std::env::args().collect();
    let fake_ninja_compat =
        Path::new(&args[0]).file_name().unwrap() == std::ffi::OsStr::new("ninja");

    // Ninja uses available processors + a constant, but I don't think the
    // difference matters too much.
    let mut parallelism = usize::from(std::thread::available_parallelism()?);

    let mut opts = getopts::Options::new();
    opts.optopt("C", "", "chdir before running", "DIR");
    opts.optopt("d", "debug", "debugging tools", "TOOL");
    opts.optopt("t", "tool", "subcommands", "TOOL");
    opts.optopt(
        "j",
        "",
        &format!("parallelism [default from system={}]", parallelism),
        "NUM",
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

    if fake_ninja_compat {
        if matches.opt_present("version") {
            println!("1.10.2");
            return Ok(0);
        }
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

    if let Some(dir) = matches.opt_str("C") {
        let dir = Path::new(&dir);
        std::env::set_current_dir(dir).map_err(|err| anyhow!("chdir {:?}: {}", dir, err))?;
    }

    let mut progress = ConsoleProgress::new(matches.opt_present("v"), use_fancy_terminal());

    // Build once with regen=true, and if the result says we regenerated the
    // build file, reload and build everything a second time.
    let mut result = build(&mut progress, parallelism, true, &matches.free)?;
    if let BuildResult::Regen = result {
        result = build(&mut progress, parallelism, false, &matches.free)?;
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

    return Ok(0);
}

pub fn run() -> anyhow::Result<i32> {
    let res = run_impl();
    trace::close();
    res
}
