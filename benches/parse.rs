use criterion::{criterion_group, criterion_main, Criterion};
use std::io::Write;

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

pub fn bench_parse(c: &mut Criterion) {
    let mut input: Vec<u8> = Vec::new();
    for i in 0..50 {
        write!(
            input,
            "build $out/foo/bar{}.o: cc $src/long/file/name{}.cc
        depfile = $out/foo/bar{}.o.d
",
            i, i, i
        )
        .unwrap();
    }
    input.push(0);

    let mut loader = NoOpLoader {};
    c.bench_function("parse", |b| {
        b.iter(|| {
            let mut parser = n2::parse::Parser::new(&input);
            parser.read(&mut loader).unwrap();
        })
    });
}

criterion_group!(benches, bench_canon, bench_parse);
criterion_main!(benches);
