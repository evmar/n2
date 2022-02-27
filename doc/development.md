## Path handling and Unicode safety

Currently we use Rust `String` for all paths and file contents, but
internally interpret them as as bytes (not UTF8) including using "unsafe"
sometimes to convert.

Based on my superficial understanding of how safety relates to UTF8 in Rust
strings, it's probably harmless given that we never treat strings as Unicode,
but it's also possible some code outside of our control relies on this.

The proper fix is to switch to a bag of bytes type.  I attempted this initially
but ran into trouble making my custom string type compatible with hash tables.

## Profiling

It appears profiling Rust under WSL2 is not a thing(?).

On Mac, the best options seemed to be `cargo flamegraph` and
`cargo instruments`.

## Benchmarking

This benchmarks load time, by asking to build a nonexistent target:

1. `cargo install hyperfine`
2. `$ hyperfine -i -- './target/release/n2 -C ~/llvm-project/llvm/utils/gn/out/ xxx'`
