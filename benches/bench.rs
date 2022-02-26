use n2::canon::canon_path;
use n2::parse::Parser;
use n2::scanner::Scanner;
use std::fmt::Write;

// This code used Criterion, but Criterion had a massive set of dependencies,
// was slow to compile, and clunky to actually use, so I disabled it for now.

pub struct Criterion {}
impl Criterion {
    fn bench_function(&mut self, _name: &str, _f: impl Fn(&mut Criterion) -> ()) {}
    fn iter(&mut self, _f: impl Fn() -> ()) {}
}

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
    let mut input = String::new();
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
    input.push(0 as char);

    c.bench_function("parse", |b| {
        b.iter(|| {
            let scanner = Scanner::new(&input);
            let mut parser = Parser::new(scanner);
            parser.read().unwrap();
        })
    });
}

// criterion_group!(benches, bench_canon, bench_parse);
// criterion_main!(benches);
