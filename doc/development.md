## Git hook

On a new checkout, run this to install the formatting check hook:

```
$ ln -s ../../git-pre-commit .git/hooks/pre-commit
```

## Path handling and Unicode safety

Currently we use Rust `String` for all paths and file contents, but
internally interpret them as as bytes (not UTF8) including using "unsafe"
sometimes to convert.

Based on my superficial understanding of how safety relates to UTF8 in Rust
strings, it's probably harmless given that we never treat strings as Unicode,
but it's also possible some code outside of our control relies on this.

The proper fix is to switch to a bag of bytes type, like
https://crates.io/crates/bstr.  I attempted this initially with my own custom
string type but ran into trouble making it compatible with hash tables.

## Profiling

### gperftools

I played with a few profilers, but I think the gperftools profiler turned out
to be significantly better than the others.  To install:

```
$ apt install libgoogle-perftools-dev
$ go install github.com/google/pprof@latest
```

To use:

```
[possibly modify main.rs to make the app do more work than normal]
$ LD_PRELOAD=/usr/lib/x86_64-linux-gnu/libprofiler.so CPUPROFILE=p ./target/release/n2 ...
$ pprof -http=:8080 ./target/release/n2 p
```

The web server it brings up shows an interactive graph, top functions,
annotated code, disassembly...

### Other options

It appears `perf` profiling of Rust under WSL2 is not a thing(?).

Some other options on Mac that seemed ok are `cargo flamegraph` and
`cargo instruments`.

## Benchmarking

This benchmarks load time, by asking to build a nonexistent target:

1. `cargo install hyperfine`
2. `$ hyperfine -i -- './target/release/n2 -C ~/llvm-project/llvm/utils/gn/out/ xxx'`
