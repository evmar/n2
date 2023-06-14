# n2, an alternative ninja implementation

![CI status](https://github.com/evmar/n2/actions/workflows/ci.yml/badge.svg)
![MSRV](https://img.shields.io/badge/msrv-1.59.0-red)

n2 (pronounced "into") implements enough of [ninja](https://ninja-build.org/) to
successfully build some projects that build with ninja.

I wrote it to
[explore some alternative ideas](http://neugierig.org/software/blog/2022/03/n2.html)
I had around how to structure a build system. In a very real sense the
exploration is more important than the actual software itself, so you can view
the [design notes](doc/design_notes.md) as one of the primary artifacts of this.

[Here's a small demo](https://asciinema.org/a/480446) of n2 building some of
Clang.

## Install

```
$ cargo build --release
$ ./target/release/n2 -C some/build/dir
```

When CMake executes Ninja it expects some particular Ninja behaviors. n2
emulates these behaviors when invoked as `ninja`. To use n2 with CMake you can
create a symlink:

- UNIX: `ln -s path/to/n2 ninja`
- Windows(cmd): `mklink ninja.exe path\to\n2`
- Windows(PowerShell): `New-Item -Type Symlink ninja.exe -Target path\to\n2`

somewhere in your `$PATH`, such that CMake can discover it.

## The console output

While building, n2 displays build progress like this:

```
[=========================---------       ] 2772/4459 done, 8/930 running
2s Building foo/bar
0s Building foo/baz
```

The progress bar always covers all build steps needed for the targets,
regardless of whether they need to be executed or not.

- **Done:** This build has 4459 total build steps, 2272 of which are currently
  up to date. This is what the equals signs show.
- **In progress:** Hyphens show steps that are in-progress (e.g. if you had
  enough CPUs they would all be executing). The `8/930 running` means n2 is
  currently executing 8 of them.
- **Unknown:** The remaining space shows steps whose status is yet to be known,
  as they depend on the in progress steps.

The lines below the progress bar show some build steps that are currrently
running, along with how long they've been running. Their text is controlled by
the input `build.ninja` file.

## More reading

- [Design notes](doc/design_notes.md).
- [Development tips](doc/development.md).

## Differences from Ninja

n2 is [missing many Ninja features](doc/missing.md).

n2 does some things Ninja doesn't:

- Builds start tasks as soon as an out of date one is found, rather than
  gathering all the out of date tasks before executing.
- Fancier status output, modeled after Bazel.
- `-d trace` generates a performance trace as used by Chrome's `about:tracing`
  or alternatives (speedscope, perfetto).
