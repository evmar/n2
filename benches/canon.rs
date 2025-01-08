use std::hint::black_box;

use divan::Bencher;

mod paths {
    pub const NOOP: &str = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
            CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
    pub const PARENTS: &str = "examples/../OrcV2Examples/OrcV2CBindingsVeryLazy/../../../\
            CMakeFiles/OrcV2CBindingsVeryLazy.dir/OrcV2CBindingsVeryLazy.c.o";
    pub const ONE_DOT: &str = "examples/./OrcV2Examples/./OrcV2CBindingsVeryLazy/\
            CMakeFiles/OrcV2CBindingsVeryLazy.dir/././OrcV2CBindingsVeryLazy.c.o";
    pub const TWO_DOTS_IN_COMPONENT: &str = "examples/OrcV2Examples/OrcV2CBindingsVeryLazy/\
            ..CMakeFiles/OrcV2CBindingsVeryLazy.dir/../OrcV2CBindingsVeryLazy.c.o";
}

macro_rules! cases {
    () => {
        #[divan::bench]
        pub fn noop(b: Bencher) {
            run(b, paths::NOOP)
        }

        #[divan::bench]
        pub fn with_parents(b: Bencher) {
            run(b, paths::PARENTS)
        }

        #[divan::bench]
        pub fn with_one_dot(b: Bencher) {
            run(b, paths::ONE_DOT)
        }

        #[divan::bench]
        pub fn with_two_dots_in_component(b: Bencher) {
            run(b, paths::TWO_DOTS_IN_COMPONENT)
        }
    };
}

mod inplace {
    use super::*;

    fn run(b: Bencher, path: &str) {
        b.with_inputs(|| path.to_string()).bench_values(|path| {
            let mut path = black_box(path);
            n2::canon::canon_path_fast(&mut path);
            // Return the String buffer, so that the deallocation is not benchmarked.
            black_box(path)
        })
    }

    cases! {}
}

pub mod allocating {
    use super::*;

    fn run(b: Bencher, path: &str) {
        b.bench(|| {
            // Return the String buffer, so that the deallocation is not benchmarked.
            black_box(n2::canon::canon_path(black_box(path)))
        });
    }

    cases! {}
}

use divan::main;
