# Multiple Grid Squares in the Startup Filter Set — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `ipawsClient` accept several Maidenhead grid squares at startup and print IPAWS alerts concerning any of them.

**Architecture:** Collect every bare positional as a grid into `Args.squares: Vec<String>`. At startup, decode + look up each square into one `filter::Square`, giving a `Vec<Square>` and one shared `seen::Seen`. In the MQTT loop an alert passes when any square matches. `filter.rs` is unchanged; the union is `.any()` over `Square::matches` plus the shared `Seen`.

**Tech Stack:** Rust 2021, `rumqttc`, `serde_json`. Spec: `docs/2026-10-08-multi-grid-filter-design.md`.

## Global Constraints

- `filter.rs` is not modified — `Square`, `decode`, `matches` keep their current shape.
- No new dependencies.
- Zero grids keeps the existing pass-all behavior.
- Commit messages: plain identifiers, no backticks; no `Co-Authored-By` trailer.

---

### Task 1: Parser collects multiple grids into `Args.squares`

**Files:**
- Modify: `src/main.rs` — `struct Args` (28–32), `parse_args` (34–47), `USAGE` (19–26), and the `tests::arguments` test (241–255).

**Interfaces:**
- Produces: `struct Args { squares: Vec<String>, raw: bool, server: String }` and `fn parse_args(args: &[String]) -> Result<Args, String>`. `squares` holds every bare positional in first-seen order, exact-string de-duplicated; `raw`/`server` unchanged.

- [ ] **Step 1: Update the failing test**

In `src/main.rs`, replace the body of the `arguments` test (currently at 241–255) with:

```rust
    #[test]
    fn arguments() {
        let one = Args { squares: vec!["CM87vh".into()], raw: false, server: DEFAULT_SERVER.into() };
        assert_eq!(p(&["CM87vh"]), Ok(one));
        for flag in ["xml", "raw", "--xml", "--raw", "XML", "--RAW"] {
            assert!(p(&["CM87vh", flag]).unwrap().raw, "{flag}");
            assert!(p(&[flag, "CM87vh"]).unwrap().raw, "{flag} first");
        }
        assert_eq!(p(&["CM87", "--server", "127.0.0.1"]).unwrap().server, "127.0.0.1");
        let none = Args { squares: vec![], raw: false, server: DEFAULT_SERVER.into() };
        assert_eq!(p(&[]), Ok(none), "no arguments: every alert, as JSON");
        assert!(p(&["raw"]).unwrap().squares.is_empty());
        // Multiple grids now accepted, in order.
        assert_eq!(p(&["CM87", "CM88"]).unwrap().squares, vec!["CM87", "CM88"]);
        // Interleaved with flags and server.
        assert_eq!(p(&["CM97", "xml", "CM98", "--server", "h"]).unwrap().squares, vec!["CM97", "CM98"]);
        // Exact-string de-duplication, first-seen order.
        assert_eq!(p(&["CM88", "CM87", "CM88"]).unwrap().squares, vec!["CM88", "CM87"]);
        assert!(p(&["CM87", "--bogus"]).is_err());
        assert!(p(&["CM87", "--server"]).is_err());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --quiet arguments`
Expected: FAIL — compile errors (`Args` has no field `squares`; `.square` vs `.squares`).

- [ ] **Step 3: Change `Args` and `parse_args`**

Replace `struct Args` (28–32) with:

```rust
#[derive(Debug, PartialEq)]
struct Args {
    squares: Vec<String>,
    raw: bool,
    server: String,
}
```

(Keep whatever derives are already on `Args`; it must stay `Debug, PartialEq` for the tests.)

Replace the body of `parse_args` (34–47) with:

```rust
fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut squares, mut raw, mut server): (Vec<String>, bool, String) =
        (Vec::new(), false, DEFAULT_SERVER.to_string());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_ascii_lowercase().as_str() {
            "xml" | "raw" | "--xml" | "--raw" => raw = true,
            "--server" => server = it.next().ok_or("--server needs a host")?.clone(),
            s if s.starts_with('-') => return Err(format!("unknown option {a}")),
            _ => {
                if !squares.iter().any(|s| s == a) {
                    squares.push(a.clone());
                }
            }
        }
    }
    Ok(Args { squares, raw, server })
}
```

- [ ] **Step 4: Update `USAGE`**

In the `USAGE` constant (19–26), change the first line's `[grid square]` to `[grid square ...]` and the sentence `that concern a 4- or 6-character Maidenhead grid square,` to `that concern one or more 4- or 6-character Maidenhead grid squares,`:

```rust
const USAGE: &str = "usage: ipawsClient [grid square ...] [xml|raw|--xml|--raw] [--server [http[s]://]HOST[:PORT]]

Prints IPAWS alerts that concern one or more 4- or 6-character Maidenhead grid
squares, or every alert if no square is given, as pretty-printed JSON by
default, or as the original CAP XML with xml/raw. Alerts are separated by a line
of hyphens. --server defaults to ipaws.kd6o.ampr.org. The lookup goes to
https://HOST[:PORT] unless http:// is given; MQTT is always HOST:1883.";
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --quiet arguments`
Expected: PASS (1 test).

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat: parse multiple grid squares into Args.squares"
```

---

### Task 2: Watch all squares at startup and match any in the loop

**Files:**
- Modify: `src/main.rs` — the `use` block (add `HashSet`), the `filtering` setup (123–147), the match check in the MQTT loop (199–207).

**Interfaces:**
- Consumes: `Args.squares: Vec<String>` from Task 1; `filter::decode`, `filter::Square::new`, `filter::Square::matches`, `seen::Seen`, `string_list`, `http_get`, `fail`, `note` (all already in `main.rs`).
- Produces: `filtering: Option<(Vec<filter::Square>, seen::Seen)>` in `main`. No new public names.

- [ ] **Step 1: Add the `HashSet` import**

At the top of `src/main.rs` with the other `use` lines, add (if not already present):

```rust
use std::collections::HashSet;
```

- [ ] **Step 2: Replace the `filtering` setup**

Replace the `let mut filtering = args.square.as_deref().map(|code| { ... });` block and the following `if filtering.is_none() { ... }` (123 through the pass-all note) with:

```rust
    // No squares: pass everything, so no lookup and no memory of printed alerts.
    let mut filtering = if args.squares.is_empty() {
        note(format!("v{} passing all alerts on {host}:1883 {topic}", env!("CARGO_PKG_VERSION")));
        None
    } else {
        let seen_path = seen::default_path();
        if seen_path.is_none() {
            note("no state directory; updates to earlier alerts won't be recognized");
        }
        let mut squares = Vec::new();
        let mut counties: HashSet<String> = HashSet::new();
        for code in &args.squares {
            let bounds = filter::decode(code)
                .unwrap_or_else(|e| fail(2, format!("{code:?} is not a Maidenhead square: {e}")));
            let (status, body) = http_get(&format!("{base}/v1/squares/{}", code.trim()))
                .unwrap_or_else(|e| fail(1, format!("lookup of {code} at {base} failed: {e}")));
            let info: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                fail(1, format!("lookup of {code} at {base} returned HTTP {status}, unreadable: {e}"))
            });
            if status != 200 {
                fail(1, format!("lookup of {code} at {base} returned HTTP {status}: {info}"));
            }
            let (fips, ugc) = (string_list(&info["fips"]), string_list(&info["ugc"]));
            let partials = string_list(&info["same_partial"]); // absent from older servers: empty
            counties.extend(fips.iter().cloned());
            squares.push(filter::Square::new(bounds, &fips, &ugc, &partials));
        }
        note(format!(
            "v{} watching {} ({} counties across {} square{}) on {host}:1883 {topic}",
            env!("CARGO_PKG_VERSION"),
            args.squares.join(" "),
            counties.len(),
            args.squares.len(),
            if args.squares.len() == 1 { "" } else { "s" },
        ));
        Some((squares, seen::Seen::load(seen_path)))
    };
```

- [ ] **Step 3: Replace the match check in the MQTT loop**

In the `Packet::Publish` arm, replace (199–207):

```rust
                if let Some((square, seen)) = &filtering {
                    if seen.contains(&key) {
                        continue; // already printed (e.g. redelivered)
                    }
                    if !square.matches(&alert) && !seen::references(&alert).iter().any(|r| seen.contains(r)) {
                        continue;
                    }
                }
```

with:

```rust
                if let Some((squares, seen)) = &filtering {
                    if seen.contains(&key) {
                        continue; // already printed (e.g. redelivered)
                    }
                    if !squares.iter().any(|sq| sq.matches(&alert))
                        && !seen::references(&alert).iter().any(|r| seen.contains(r))
                    {
                        continue;
                    }
                }
```

(The `if let Some((_, seen)) = &mut filtering { seen.insert(key); }` after printing is unchanged — its binding already ignores the first tuple element.)

- [ ] **Step 4: Build and run the full test suite**

Run: `cargo test --quiet`
Expected: PASS — all tests in `main.rs` and `filter.rs`, no warnings about `args.square`.

- [ ] **Step 5: Manual smoke of the startup note**

Run: `cargo run --quiet -- CM98 CM97 --server 127.0.0.1`
Expected: a stderr `note` line of the form `vX.Y.Z watching CM98 CM97 (N counties across 2 squares) on 127.0.0.1:1883 ipaws/cap/json`, then a lookup error (no server at 127.0.0.1) — confirming both grids were parsed and looked up. A bad grid, e.g. `cargo run --quiet -- CM98 ZZ99`, exits 2 naming `ZZ99`.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat: watch multiple grid squares, matching any at the MQTT loop"
```

---

## Self-Review

- **Spec coverage:** CLI multiple-positionals + dedup + USAGE (Task 1); per-square decode/lookup, `Vec<Square>`, shared `Seen`, combined-county startup note, per-grid failure, pass-all when empty (Task 2); match-any in the loop (Task 2, Step 3); `filter.rs` untouched (both tasks); parser unit test (Task 1). No gaps.
- **Placeholder scan:** none — every step shows the full code or exact command.
- **Type consistency:** `Args.squares: Vec<String>` (Task 1) is consumed by the Task 2 setup; `filtering: Option<(Vec<filter::Square>, seen::Seen)>` is used consistently in setup, the match check, and the unchanged `seen.insert` arm.
