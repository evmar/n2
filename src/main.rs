extern crate getopts;

use n2::load;
use n2::trace;
use n2::work;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mut opts = getopts::Options::new();
    opts.optopt("C", "", "chdir", "DIR");
    opts.optflag("h", "help", "help");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            println!("{}", f);
            return;
        }
    };
    if matches.opt_present("h") {
        println!("TODO: help");
        return;
    }

    trace::open("trace.json").unwrap();

    if let Some(dir) = matches.opt_str("C") {
        std::env::set_current_dir(dir).unwrap();
    }

    let state = trace::scope("load::read", || load::read());
    let load::State {
        mut graph,
        mut db,
        default,
        hashes: last_hashes,
    } = match state {
        Err(err) => {
            println!("ERROR: {}", err);
            return;
        }
        Ok(ok) => ok,
    };

    let target = match matches.free.len() {
        0 => default.expect("TODO"),
        1 => graph.file_id(&matches.free[0]),
        _ => panic!("unimpl: multiple args"),
    };
    println!("target {:?}", graph.file(target).name);
    let mut work = work::Work::new(&mut graph, &last_hashes, &mut db);
    trace::scope("want_file", || work.want_file(target)).unwrap();
    match trace::scope("work.run", || work.run()) {
        Ok(_) => {}
        Err(err) => {
            println!("error: {}", err);
        }
    }

    trace::close().unwrap();
}
