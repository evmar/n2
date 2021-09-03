extern crate getopts;

mod parse;
mod graph;

struct LoadState<'a> {
    rules: Vec<parse::Rule<'a>>,
}

fn read() -> Result<(), String> {
    let mut bytes = match std::fs::read("build.ninja") {
        Ok(b) => b,
        Err(e) => return Err(format!("read build.ninja: {}", e)),
    };
    bytes.push(0);
    let mut p = parse::Parser::new(&bytes);
    let mut env = parse::Env::new();
    loop {
        match p.read(&mut env) {
            Err(err) => {
                println!("{}", p.format_parse_error(err));
                break;
            }
            Ok(None) => break,
            Ok(Some(p)) => println!("parsed as {:#?}", p),
        }
    }
    Ok(())
}

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

    if let Err(err) = read() {
        println!("ERROR: {}", err);
    }
}
