use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::{io::Write, path::PathBuf, str::FromStr};

pub fn bench_canon(c: &mut Criterion) {
    let mut group = c.benchmark_group("canon_path");

    // TODO switch to canon_path_fast
    group.bench_with_input(
        "plain",
        "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o",
        |b, path| {
            b.iter(|| {
                n2::canon::canon_path(path);
            })
        },
    );

    group.bench_with_input(
        "with parents",
        "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
                ../../../\
                CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o",
        |b, path| {
            b.iter(|| {
                n2::canon::canon_path(path);
            })
        },
    );
}

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

fn bench_parse_synthetic(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse synthetic");

    for statement_count in [1000, 5000] {
        let mut input = generate_build_ninja(statement_count);
        input.push(0);

        group.throughput(Throughput::Elements(statement_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(statement_count),
            &input,
            |b, input| {
                b.iter(|| {
                    let mut parser = n2::parse::Parser::new(input);
                    loop {
                        if parser.read().unwrap().is_none() {
                            break;
                        }
                    }
                })
            },
        );
    }
}

fn bench_parse_file(c: &mut Criterion) {
    let input = match n2::scanner::read_file_with_nul("benches/build.ninja".as_ref()) {
        Ok(input) => input,
        Err(err) => {
            eprintln!("failed to read benches/build.ninja: {}", err);
            eprintln!("will skip benchmarking with real data");
            return;
        }
    };
    c.bench_with_input(
        BenchmarkId::new("parse build.ninja", format!("{} bytes", input.len())),
        &input,
        |b, input| {
            b.iter(|| {
                let mut parser = n2::parse::Parser::new(input);
                loop {
                    if parser.read().unwrap().is_none() {
                        break;
                    }
                }
            })
        },
    );
}

fn bench_load_synthetic(c: &mut Criterion) {
    let mut input = generate_build_ninja(1000);
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

criterion_group!(
    benches,
    bench_canon,
    bench_parse_synthetic,
    bench_parse_file,
    bench_load_synthetic
);
criterion_main!(benches);
