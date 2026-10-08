# Multiple grid squares in the startup filter set

Date: 2026-10-08. Status: design, approved for spec review.

## Goal

Let `ipawsClient` watch more than one Maidenhead grid square at once. Today it
takes a single square and prints IPAWS alerts concerning it; the operator wants
to pass several squares at startup and see alerts concerning any of them.

## CLI

Grids are **bare positionals**, extending the current interface:

    ipawsClient [grid square ...] [xml|raw|--xml|--raw] [--server [http[s]://]HOST[:PORT]]

Example: `ipawsClient CM98 CM97 CM88 xml --server ipaws.kd6o.ampr.org`

The parser keeps its current token handling — `xml`/`raw`/`--xml`/`--raw` set
raw mode, `--server HOST` sets the server, a leading-`-` token that is neither is
an "unknown option" error — and collects **every other positional** as a grid
rather than accepting only the first. No regex classification: anything that is
not a format or server token is treated as a grid, and `filter::decode`
validates it at startup. The list is de-duplicated preserving first-seen order,
so a repeated grid is looked up once. Zero grids keeps the existing pass-all
behavior.

`Args.square: Option<String>` becomes `Args.squares: Vec<String>`. `USAGE` is
updated to the line above.

## Startup

For each square, unchanged per-square work: `filter::decode(code)` for the
bounds, then `GET {base}/v1/squares/{code}` for its `fips`/`ugc`/`same_partial`,
building one `filter::Square`. The result is a `Vec<Square>` plus one shared
`seen::Seen` (loaded once from `seen::default_path()`).

`filtering` changes from `Option<(Square, Seen)>` to `Option<(Vec<Square>,
Seen)>`. The startup `note` lists every watched square and the combined
distinct-county count, e.g. `watching CM98 CM97 CM88 (N counties across 3
squares) on HOST:1883 TOPIC`.

A square whose `decode` fails, or whose lookup returns non-200 or is
unreachable, fails startup exactly as today (`fail(2)` for a bad square,
`fail(1)` for a lookup error), naming the offending square. One bad grid aborts
the run rather than silently dropping that grid.

## Match loop

An alert passes the filter when it concerns **any** watched square:

    squares.iter().any(|sq| sq.matches(&alert))

The existing seen-references check is unchanged (an update that references an
already-printed alert still passes). The single shared `Seen` dedups across all
grids, so an alert concerning two of the watched squares prints once, and the
`seen.insert(key)` after printing is unchanged.

`filter.rs` is untouched: `Square`, `decode`, and `matches` keep their current
shape. The union is purely the `.any()` in the loop and the shared `Seen`.

## Testing

- A `parse_args` unit test in `main.rs`: multiple positionals collect into
  `squares` in order; duplicates are removed; interleaved `xml` and `--server
  HOST` still parse; a lone `-x` still errors. (`main.rs` has no test module
  yet; add a `#[cfg(test)]` one for this.)
- The per-`Square` behavior is already covered by `filter.rs` tests; the new
  union behavior is just `.any()` over `Square::matches`, so it needs no new
  matching test.

## Out of scope

- Per-square `seen` files (one shared set is intended).
- Reading grids from a file or env var.
- Any change to the lookup protocol or `filter.rs`.
