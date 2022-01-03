# n2, an alternative ninja implementation

n2 (pronounced "into") implements enough of [ninja](https://ninja-build.org/)
to at least successfully build a couple of projects that build with ninja.

I wrote it to explore some alternative ideas I had around how to structure
a build system.

## Missing features

Known missing pieces of real Ninja include:

- Windows support, including MVSC-related features;
- [Pools](https://ninja-build.org/manual.html#ref_pool);
- RSP files;
- Dynamic dependencies;
- Other [rule variables](https://ninja-build.org/manual.html#ref_rule) such as
  `in_newline`;
- Many command-line flags such as the various `-t` tools.

There are likely many more pieces I overlooked.
