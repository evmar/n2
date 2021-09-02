mod parse;
mod graph;

struct LoadState<'a> {
    rules: Vec<parse::Rule<'a>>,
}

fn read() -> std::io::Result<()> {
    let mut bytes = std::fs::read("build.ninja")?;
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
    read().unwrap();
}
