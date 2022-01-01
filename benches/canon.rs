use criterion::{criterion_group, criterion_main, Criterion};
use n2::canon::canon_path;

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

criterion_group!(benches, bench_canon);
criterion_main!(benches);
