# Missing features from Ninja

- Windows is only partially implemented.
  - `deps = msvc` (parsing of `/showIncludes` output) isn't implemented at all,
    which means n2 currently gets header dependencies wrong when you use the
    MSVC compiler.
- Dynamic dependencies.
- `console` pool. n2 currently just treats `console` as an ordinary pool of
  depth 1, and only shows console output after the task completes.
- `subninja` is only partially implemented.

## Missing flags

- `-l`, load average throttling
- `-n`, dry run

### Missing subcommands

Most of `-d` (debugging), `-t` (tools).

No `-w` (warnings).
