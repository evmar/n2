extern crate getopts;

use n2::load;
use n2::progress;
use n2::trace;
use n2::work;

fn run() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mut opts = getopts::Options::new();
    opts.optopt("C", "", "chdir", "DIR");
    opts.optopt("d", "debug", "debug", "TOOL");
    opts.optflag("h", "help", "help");
    let matches = opts.parse(&args[1..])?;
    if matches.opt_present("h") {
        anyhow::bail!("TODO: help");
    }

    if let Some(debug) = matches.opt_str("d") {
        match debug.as_str() {
            "trace" => trace::open("trace.json")?,
            _ => anyhow::bail!("unknown -d {:?}", debug),
        }
    }

    if let Some(dir) = matches.opt_str("C") {
        std::env::set_current_dir(dir).unwrap();
    }

    let load::State {
        mut graph,
        mut db,
        default,
        hashes: last_hashes,
    } = trace::scope("load::read", load::read)?;

    let mut targets = Vec::new();
    for free in matches.free {
        let id = match graph.get_file_id(&free) {
            None => anyhow::bail!("unknown path requested: {:?}", free),
            Some(id) => id,
        };
        targets.push(id);
    }
    if targets.is_empty() {
        match default {
            // TODO: build all?
            None => anyhow::bail!("no path specified and no default"),
            Some(id) => targets.push(id),
        }
    }

    let mut progress = progress::RcProgress::new(progress::ConsoleProgress::new());

    let mut work = work::Work::new(&mut graph, &last_hashes, &mut db, &mut progress);
    for target in targets {
        work.want_file(target);
    }
    trace::scope("work.run", || work.run())
}

fn main() {
    match run() {
        Ok(_) => {}
        Err(err) => {
            println!("n2: error: {}", err);
        }
    }
    trace::close().unwrap();
}
