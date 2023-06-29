# n2, an alternative ninja implementation

![CI status](https://github.com/evmar/n2/actions/workflows/ci.yml/badge.svg)
![MSRV](https://img.shields.io/badge/msrv-1.59.0-red)

n2 (pronounced "into") implements enough of [Ninja](https://ninja-build.org/) to
successfully build some projects that build with Ninja. Compared to Ninja, n2
missing some features but is faster to build and has a better UI; see
[a more detailed comparison](doc/comparison.md).

> [Here's a small demo](https://asciinema.org/a/F2E7a6nX4feoSSWVI4oFAm21T) of n2
> building some of Clang.

## Install

```
$ cargo install --git https://github.com/evmar/n2
# (installs into ~/.cargo/bin/)

$ n2 -C some/build/dir some-target
```

### Replacing Ninja when using CMake

When CMake generates Ninja files it attempts run a program named `ninja` with
some particular Ninja behaviors. If you have Ninja installed already then things
will continue to work as before.

If you don't have Ninja installed at all, n2 can emulate the expected CMake
behavior when invoked as `ninja`. To do this you create a symlink named `ninja`
somewhere in your `$PATH`, such that CMake can discover it.

- UNIX: `ln -s path/to/n2 ninja`
- Windows(cmd): `mklink ninja.exe path\to\n2`
- Windows(PowerShell): `New-Item -Type Symlink ninja.exe -Target path\to\n2`

## The console output

While building, n2 displays build progress like this:

```
[=========================---------       ] 2772/4459 done, 8/930 running
2s Building foo/bar
0s Building foo/baz
```

The progress bar always covers all build steps needed for the targets,
regardless of whether they need to be executed or not.

The bar shows three categories of state:

- **Done:** The `=` signs show the build steps that are already up to date.
- **In progress:** The `-` signs show steps that are in-progress; if you had
  enough CPUs they would all be executing. The `8/930 running` after shows that
  n2 is currently executing 8 of the 930 available steps.
- **Unknown:** The remaining empty space indicates steps whose status is yet to
  be known, as they depend on the in progress steps. For example, if an
  intermediate step doesn't write its outputs n2 may not need to execute the
  dependent steps.

The lines below the progress bar show some build steps that are currrently
running, along with how long they've been running. Their text is controlled by
the input `build.ninja` file.

## More reading

I wrote n2 to
[explore some alternative ideas](http://neugierig.org/software/blog/2022/03/n2.html)
I had around how to structure a build system. In a very real sense the
exploration is more important than the actual software itself, so you can view
the design notes as one of the primary artifacts of this.

- [Design notes](doc/design_notes.md).
- [Development tips](doc/development.md).
- [Comparison with Ninja / missing features](doc/comparison.md).
