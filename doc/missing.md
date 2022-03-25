# Missing features from Ninja

- Windows support, including MVSC-related features.
- RSP files.
- Dynamic dependencies.
- `console` pool.  n2 currently just treats `console` as an ordinary pool of
  depth 1, and only shows console output after the task completes.
- `subninja` is only partially implemented.
- Timestamps with higher-than-seconds granularity.

## Missing flags

- `-f`, specify build file
- `-k`, keep going until N jobs fail
- `-l`, load average throttling
- `-n`, dry run

### Missing subcommands

Most of `-d` (debugging), `-t` (tools).

No `-w` (warnings).

## Missing rule variables

- `$in_newline`, `$out_newline`
