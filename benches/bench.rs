use criterion::{Criterion, criterion_group, criterion_main};
use n2::canon::canon_path;
use n2::parse::Parser;
use n2::scanner::Scanner;

pub fn bench_canon(c: &mut Criterion) {
    c.bench_function("canon plain", |b| {
        b.iter(|| {
            let path = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
            canon_path(path);
        })
    });

    c.bench_function("canon with parents", |b| {
        b.iter(|| {
            let path = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                ../../../\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
            canon_path(path);
        })
    });
}

pub fn bench_parse(c: &mut Criterion) {
    let input = "build $out/foo/bar.o: cc $src/long/file/name.cc
depfile = $out/foo/bar.o.d
\0";

    c.bench_function("parse", |b| {
        b.iter(|| {
            let scanner = Scanner::new(input);
            let mut parser = Parser::new(scanner);
            parser.read().unwrap();
        })
    });
}

criterion_group!(benches, bench_canon, bench_parse);
criterion_main!(benches);
