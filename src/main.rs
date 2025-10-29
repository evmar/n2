fn main() {
    let exit_code = match n2::run::run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("n2: error: {}", err);
            1
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
