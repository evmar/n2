extern crate getopts;

mod canon;
mod db;
mod depfile;
mod eval;
mod graph;
mod load;
mod parse;
mod scanner;
mod work;

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

    if let Some(dir) = matches.opt_str("C") {
        std::env::set_current_dir(dir).unwrap();
    }

    let load::State {
        mut graph,
        mut db,
        default,
        hashes: last_hashes,
    } = match load::read() {
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
    work.want_file(target).unwrap();
    match work.run() {
        Ok(_) => {}
        Err(err) => {
            println!("error: {}", err);
        }
    }
}
