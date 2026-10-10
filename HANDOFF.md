# HANDOFF

Running notes for a fresh session on `ipawsClient` (repo `ipaws44client`).

## 2026-10-10 — shorter MQTT keepalive (v0.4.1, released)

**Symptom:** the client occasionally loses and regains the broker connection.

**Investigation (systematic-debugging, Phase 1):** the client runs on the Mac in
a terminal (stdin/out/err all a tty, not a pipe) as `ipawsClient CM98 CM97 CM88
CM87`, reaching the broker through the Pi's 44net WireGuard gateway
(`44.27.139.108`) → AMPRNet → newec2. From the broker log, most "disconnects"
were the operator re-running the client (clean close + a new process id, once a
47-min gap) — not a fault. The genuine auto-recovering drops were two broker
keepalive timeouts (Oct 6 20:10, Oct 9 11:47): the *same* process went silent
>90s and reconnected. Ruled out with evidence: broker restarts (all
early-morning, none on those days), Mac sleep (no power events; held awake 122h),
stdout backpressure (tty, not a pipe), Pi wg0 flap (handshake healthy, no
wg/network/reboot logs). Conclusion: transient stalls on the long tunneled path
that leave no log and the client already recovers from in 5s.

**Change:** `opts.set_keep_alive(60s)` → **15s** (`src/main.rs:168`). The broker
drops a silent client at 1.5x keepalive, so detection is ~22s instead of 90s.
This speeds detection/reconnect; it does not reduce how often the stalls happen.
Verified live: the restarted client reconnected as `k15` in the broker log.

**Released as v0.4.1** (`a7ea721` version bump on top of `60e0ed7` the keepalive
change; tag `v0.4.1` pushed; release workflow success, 6 assets). `cargo test`
11/11. The operator installed the locally built binary to `/Applications/ipawsClient`
and restarted it.

**Aside, not changed:** the client uses a clean session (`c1` in the broker log),
so alerts published during a reconnect gap are not redelivered. Gap-proof
delivery would be a separate change (persistent session or a catch-up query).

## 2026-10-08 — multiple grid squares in the filter set (v0.4.0, released & deployed)

`ipawsClient` now accepts more than one Maidenhead grid square at startup and
prints alerts concerning any of them. Shipped as **v0.4.0** and the service page
is redeployed.

**What changed (all in `src/main.rs`; `filter.rs` untouched):**
- `Args.square: Option<String>` → `Args.squares: Vec<String>`. The parser
  collects every bare positional as a grid, exact-string de-duplicated in
  first-seen order; `xml`/`raw`/`--server` handling unchanged. `USAGE` →
  `[grid square ...]`.
- Startup builds one `filter::Square` per grid (`decode` + `GET
  /v1/squares/{code}`) into a `Vec<Square>`, with one shared `seen::Seen`.
  `filtering: Option<(Vec<Square>, Seen)>`. Startup note lists the squares and a
  combined distinct-county count: `watching CM87 CM98 (N counties across 2
  squares)`.
- Match loop: an alert passes when `squares.iter().any(|sq| sq.matches(&alert))`
  (existing seen-references check unchanged). The shared `Seen` prints an alert
  concerning several of the squares once.
- A bad grid (`decode`) or failed lookup still aborts startup per grid, naming
  the offending square (`fail(2)` / `fail(1)`). No grids = pass-all, unchanged.

**Design/plan:** `docs/2026-10-08-multi-grid-filter-design.md`,
`docs/2026-10-08-multi-grid-filter-plan.md`.

**Commits (master, pushed):** `a163bfd` spec, `f570574` plan, `ab98b35` parser,
`321dc6a` wiring, `a2ef5a7` README, `2ac9052` release 0.4.0. Tag **`v0.4.0`**
pushed → GitHub Actions release built the five targets and published the
release (run success, 6 assets incl. SHA256SUMS). `cargo test` 11/11, clippy
clean.

**Service page (separate repo `ipaws_on_44`):**
`gridsquare/index.html` updated for multiple grids + "latest release (v0.4.0)"
(commit `acf70c7`, pushed). Deployed to newec2 with `deploy/push.sh` (guard
passed, `acf70c7` over `06b63ba`, all services active, 44net lookup HTTP 200).
Verified live at <https://ipaws.kd6o.ampr.org/>.

**Not done / out of scope (v0.4.0):** per-square `seen` files (one shared set is
intended), reading grids from a file or env var, any change to the lookup
protocol or `filter.rs`.
