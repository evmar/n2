use divan::Bencher;

mod loader {
    use super::*;
    use n2::load;

    #[divan::bench(sample_size = 3, sample_count = 3)]
    fn file_via_loader(bencher: Bencher) {
        bencher.bench_local(|| {
            load::testing::read_internal("benches/build.ninja").unwrap();
        });
    }

    #[divan::bench(sample_size = 3, sample_count = 3)]
    fn file_via_loader_slow(bencher: Bencher) {
        bencher.bench_local(|| {
            load::testing::read_internal_slow("benches/build.ninja").unwrap();
        });
    }
}

fn main() {
    divan::main();
}
