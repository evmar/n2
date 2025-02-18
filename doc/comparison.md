# Feature comparison with Ninja

n2 is intended to be able to build any project that Ninja can load. Here is a
comparison of things n2 does worse and better than Ninja.

## Improvements

Here are some things n2 improves over Ninja:

- Builds are more incremental: n2 starts running tasks as soon as an out of date
  one is found, rather than gathering all the out of date tasks before executing
  as Ninja does.
- Fancier status output, modeled after Bazel.
  [Here's a small demo](https://asciinema.org/a/F2E7a6nX4feoSSWVI4oFAm21T).
- `-d trace` generates a performance trace that can be visualized by Chrome's
  `about:tracing` or alternatives (speedscope, perfetto).

### Extensions

Some extra build variables are available only in n2:

- `hide_progress`: build edges with this flag will not show the last line of
  output in the fancy progress output.
- `hide_success`: build edges with this flag will not show the complete command
  output when the command completes successfully.

## Missing

- Windows is incomplete.
  - Ninja has special handling of backslashed paths that
    [n2 doesn't yet follow](https://github.com/evmar/n2/issues/42).
- Dynamic dependencies.
- `console` pool. n2 currently just treats `console` as an ordinary pool of
  depth 1, and only shows console output after the task completes. In practice
  this means commands that print progress when run currently show nothing until
  they're complete.
- `subninja` is only partially implemented.

### Missing flags

- `-l`, load average throttling
- `-n`, dry run

#### Missing subcommands

Most of `-d` (debugging), `-t` (tools).

No `-w` (warnings).
