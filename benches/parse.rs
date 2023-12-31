use criterion::{criterion_group, criterion_main, Criterion};
use n2::parse::Loader;
use std::{io::Write, path::PathBuf, str::FromStr};

pub fn bench_canon(c: &mut Criterion) {
    // TODO switch to canon_path_fast
    c.bench_function("canon plain", |b| {
        b.iter(|| {
            let path = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
            n2::canon::canon_path(path);
        })
    });

    c.bench_function("canon with parents", |b| {
        b.iter(|| {
            let path = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                ../../../\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
            n2::canon::canon_path(path);
        })
    });
}

struct NoOpLoader {}
impl n2::parse::Loader for NoOpLoader {
    type Path = ();
    fn path(&mut self, _path: &mut str) -> Self::Path {
        ()
    }
}

fn generate_build_ninja() -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    write!(buf, "rule cc\n    command = touch $out",).unwrap();
    for i in 0..1000 {
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

fn bench_parse_synthetic(c: &mut Criterion) {
    let mut loader = NoOpLoader {};
    let mut input = generate_build_ninja();
    input.push(0);
    c.bench_function("parse synthetic build.ninja", |b| {
        b.iter(|| {
            let mut parser = n2::parse::Parser::new(&input);
            loop {
                if parser.read(&mut loader).unwrap().is_none() {
                    break;
                }
            }
        })
    });
}

fn bench_parse_file(c: &mut Criterion, mut input: Vec<u8>) {
    let mut loader = NoOpLoader {};
    input.push(0);
    c.bench_function("parse benches/build.ninja", |b| {
        b.iter(|| {
            let mut parser = n2::parse::Parser::new(&input);
            loop {
                if parser.read(&mut loader).unwrap().is_none() {
                    break;
                }
            }
        })
    });
}

pub fn bench_parse(c: &mut Criterion) {
    match std::fs::read("benches/build.ninja") {
        Ok(input) => bench_parse_file(c, input),
        Err(err) => {
            eprintln!("failed to read benches/build.ninja: {}", err);
            eprintln!("using synthetic build.ninja");
            bench_parse_synthetic(c)
        }
    };
}

fn bench_load_synthetic(c: &mut Criterion) {
    let mut input = generate_build_ninja();
    input.push(0);
    c.bench_function("load synthetic build.ninja", |b| {
        b.iter(|| {
            let mut loader = n2::load::Loader::new();
            loader
                .parse(PathBuf::from_str("build.ninja").unwrap(), &input)
                .unwrap();
        })
    });
}

criterion_group!(benches, bench_canon, bench_parse, bench_load_synthetic);
criterion_main!(benches);
