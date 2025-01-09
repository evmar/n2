use divan::Bencher;
use std::{io::Write, path::PathBuf, str::FromStr};

fn generate_build_ninja(statement_count: usize) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    write!(buf, "rule cc\n    command = touch $out",).unwrap();
    for i in 0..statement_count {
        write!(
            buf,
            "build $out/foo/bar{}.o: cc $src/long/file/name{}.cc
  depfile = $out/foo/bar{}.o.d\n",
            i, i, i
        )
        .unwrap();
    }
    buf
}

mod parser {
    use super::*;
    use n2::parse::Parser;

    #[divan::bench]
    fn synthetic(bencher: Bencher) {
        let mut input = generate_build_ninja(1000);
        input.push(0);

        bencher.bench_local(|| {
            let mut parser = Parser::new(&input);
            while let Some(_) = parser.read().unwrap() {}
        });
    }

    // This can take a while to run (~100ms per sample), so reduce total count.
    #[divan::bench(sample_size = 3, max_time = 1)]
    fn file(bencher: Bencher) {
        let input = match n2::scanner::read_file_with_nul("benches/build.ninja".as_ref()) {
            Ok(input) => input,
            Err(err) => {
                eprintln!("failed to read benches/build.ninja: {}", err);
                eprintln!("will skip benchmarking with real data");
                return;
            }
        };
        bencher.bench_local(|| {
            let mut parser = n2::parse::Parser::new(&input);
            while let Some(_) = parser.read().unwrap() {}
        });
    }
}

#[divan::bench]
fn load_synthetic(bencher: Bencher) {
    let mut input = generate_build_ninja(1000);
    input.push(0);
    bencher.bench_local(|| {
        let mut loader = n2::load::Loader::new();
        loader
            .parse(PathBuf::from_str("build.ninja").unwrap(), &input)
            .unwrap();
    });
}

fn main() {
    divan::main();
}
