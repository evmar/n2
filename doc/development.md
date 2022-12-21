## Git hook

On a new checkout, run this to install the formatting check hook:

```
$ ln -s ../../git-pre-commit .git/hooks/pre-commit
```

## Path handling and Unicode safety

Ninja
[was intentionally "encoding agnostic"](https://ninja-build.org/manual.html#ref_lexer),
which is to say it treated input build files as any byte stream that is ASCII
compatible. In other words, any string of bytes found in a `build.ninja` is
passed verbatim through printing to stdout and to the OS for path operations,
which meant Ninja was compatible with both UTF-8 and other encodings. The intent
is that those other encodings occur on Linuxes, especially in East Asia, and
also it means Ninja doesn't need any specific knowledge of Unicode.

It looks like since my time,
[Ninja on Windows changed its input files to require UTF-8](https://github.com/ninja-build/ninja/pull/1915).
As mentioned there, this was actually a breaking change, and it looks like there
was a decent amount of fallout from the change.  This area is unfortunately
pretty subtle.

Windows is particularly fiddly in this area because Ninja needs to parse the
`/showIncludes` output from the MSVC compiler, which is localized. See the
`msvc_deps_prefix` variable in the Ninja docs; there have been lots of bug
reports over the years from people with Chinese output that is failing to parse
right due to Windows code page mess.

In any case, n2 doesn't support any of this for now, and instead just follows
Ninja in treating paths as bytes. (n2 doesn't support `/showIncludes` or MSVC at
all yet.)

Further, we currently use Rust `String` for all paths and file contents, but
internally interpret them as as bytes (not UTF-8) including using `unsafe`
sometimes to convert.

Based on my superficial understanding of how safety relates to UTF8 in Rust
strings, it's probably harmless given that we never treat strings as Unicode,
but it's also possible some code outside of our control relies on this. But it
does mean there's a bunch of kind of needless `unsafe`s in the code, and some of
them are possibly actually doing something bad.

Some better fixes are:

- Maybe switch to using a bag of bytes type, like https://crates.io/crates/bstr
  ? But it is pretty invasive. We would need to use that not only for paths but
  also console output, error messages, etc.
- Another possible fix is to require input files to be UTF-8, though I think I'd
  want to better understand the `/showIncludes` situation above. Possibly we
  could mandate "input files are UTF-8, and if you need something other than
  UTF-8 in your `msvc_deps_prefix` it's on you to escape the exact byte sequence
  you desire".

Handling Windows properly is kind of exhausting; see also the bugs about long
files names linked from the above.

## Profiling

### gperftools

I played with a few profilers, but I think the gperftools profiler turned out to
be significantly better than the others. To install:

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

The web server it brings up shows an interactive graph, top functions, annotated
code, disassembly...

### Other options

It appears `perf` profiling of Rust under WSL2 is not a thing(?).

Some other options on Mac that seemed ok are `cargo flamegraph` and
`cargo instruments`.

## Benchmarking

This benchmarks load time, by asking to build a nonexistent target:

1. `cargo install hyperfine`
2. `$ hyperfine -i -- './target/release/n2 -C ~/llvm-project/llvm/utils/gn/out/ xxx'`
