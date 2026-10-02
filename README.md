# ipawsClient

Command-line client for the **ipaws_on_44** alert relay on 44net (AMPRNet).
Give it a Maidenhead grid square and it prints each public IPAWS emergency
alert that concerns that square, as it arrives.

## What is IPAWS?

IPAWS (the U.S. Integrated Public Alert and Warning System, operated by the
Federal Emergency Management Agency) is the national master feed of public
warning messages from local, state, and federal agencies. It includes weather
warnings, earthquake notices, missing persons alerts, and local emergency
alerts of all kinds. IPAWS is the primary source for wireless cellphone alerts
and the Emergency Alert System on radio and television. This feed delivers
alerts authenticated by FEMA within seconds of their issuance.

```
ipawsClient [grid square] [xml|raw|--xml|--raw] [--server HOST[:PORT]]
```

```bash
ipawsClient CM87vh
```

```bash
ipawsClient CM87vh raw
```

```bash
ipawsClient
```

- `[grid square]`: 4 or 6 characters, e.g. `CM87` or `CM87vh`. Without one,
  every alert received is printed, unfiltered (as JSON, unless `raw`/`xml` is
  given).
- `xml`, `raw`, `--xml`, `--raw` (any one, any position): print each alert as
  the original CAP XML, byte for byte, digital signature intact. Without it,
  alerts print as pretty-printed JSON with the signature removed.
- `--server`: defaults to `44.27.128.55`. `PORT` is the lookup service's HTTP
  port (default 80); the MQTT broker is always `HOST:1883`.

The alert feed (MQTT) and the grid-square lookup answer 44net (AMPRNet)
addresses only, so the machine running the client needs a 44net connection;
from any other address the lookup returns HTTP 403 and the broker does not
answer. The service's page, <http://44.27.128.55/>, is public; it describes
the alert feed and the grid-square lookup that the client uses, for anyone
who wants to subscribe or query them directly.

## Output

Each matching alert is written to standard output followed by a line of 72
hyphens. Status and errors go to standard error, so standard output carries
only alerts:

```bash
ipawsClient CM87vh > alerts.txt
```

Each status or error line on standard error starts with a UTC timestamp. The
first line names the client's version and what it is watching; after that
the client reports each connection, and how long it was down before a
reconnection:

```
2026-10-02T06:20:02Z ipawsClient: v0.2.2 watching CM87vh (2 counties: 06081 06085; 3 UGC codes) on 44.27.128.55:1883 ipaws/cap/json
2026-10-02T06:20:02Z ipawsClient: connected to 44.27.128.55:1883; subscribing to ipaws/cap/json
2026-10-02T06:23:45Z ipawsClient: connection to 44.27.128.55:1883: Network timeout; retrying
2026-10-02T06:24:20Z ipawsClient: reconnected to 44.27.128.55:1883 after 35 s; subscribing to ipaws/cap/json
```

Exit status: `2` for a usage error or malformed square, `1` if the startup
lookup fails. Once running, the client reconnects to the broker on its own.

## Which alerts match

With no grid square, all of them. Otherwise, at startup the client asks the
lookup service for the square's counties (FIPS codes), NWS UGC codes, and NWS
partial-county partitions. Each arriving alert is kept if any of its areas
matches:

1. **SAME geocode** naming one of the square's counties, one of their whole
   states (`xx000`), the whole US (`000000`), or one of the partial-county
   partitions the square touches (first digit `1`-`9`). A partition code for
   another part of the county does not match. Only about 20 large or oddly
   shaped counties are partitioned.
2. **Polygon or circle** containing the square's center or any of its four
   corners.
3. **UGC geocode** in the square's UGC list (forecast zones and county-form
   codes), consulted only for an area that has no SAME codes and no geometry.

An alert whose `references` name an alert already printed within the last
seven days is printed too, so Updates and Cancels follow the alerts they
change even when they carry no area. The client remembers printed alerts in
a small file:

| OS | File |
|---|---|
| Windows | `%LOCALAPPDATA%\ipawsClient\seen.txt` |
| macOS | `~/Library/Application Support/ipawsClient/seen.txt` |
| Linux and others | `$XDG_STATE_HOME/ipawsClient/seen.txt`, or `~/.local/state/ipawsClient/seen.txt` |

With a grid square, the same alert delivered twice is printed once.

## Download

The current release is
[v0.2.2](https://github.com/artbotterell/ipaws44client/releases/tag/v0.2.2).
Prebuilt programs are on the
[Releases](https://github.com/artbotterell/ipaws44client/releases) page:

| File ends in | For |
|---|---|
| `x86_64-unknown-linux-musl.tar.gz` | Linux, 64-bit Intel/AMD (static; any distribution) |
| `aarch64-unknown-linux-musl.tar.gz` | Linux, 64-bit ARM, e.g. Raspberry Pi OS 64-bit (static) |
| `aarch64-apple-darwin.tar.gz` | macOS, Apple Silicon |
| `x86_64-apple-darwin.tar.gz` | macOS, Intel |
| `x86_64-pc-windows-msvc.zip` | Windows, 64-bit |

`SHA256SUMS` lists each file's checksum. The macOS builds are not signed:
the first time, remove the quarantine flag or allow the program in System
Settings > Privacy & Security.

```bash
xattr -d com.apple.quarantine ./ipawsClient
```

## Building

Needs Rust ([rustup.rs](https://rustup.rs)).

```bash
cargo install --git https://github.com/artbotterell/ipaws44client
```

The JSON conversion comes from [jcap](https://github.com/artbotterell/jcap).

## Releasing

Releases are built by GitHub Actions
([.github/workflows/release.yml](.github/workflows/release.yml)).

1. Set the new version in `Cargo.toml`, run `cargo test` (which updates
   `Cargo.lock`), commit, and push.
2. Tag that commit with the same version and push the tag:

   ```bash
   git tag -a v0.3.0 -m "ipawsClient 0.3.0"
   ```

   ```bash
   git push origin v0.3.0
   ```

3. The workflow tests and builds the five targets listed under Download. If
   all succeed, it publishes a GitHub Release named after the tag, with the
   five archives, `SHA256SUMS`, and notes generated from the commits. If any
   build fails, nothing is published; fix it, delete the tag
   (`git push --delete origin v0.3.0` and `git tag -d v0.3.0`), and tag again.

Running the workflow by hand (Actions > release > Run workflow, or
`gh workflow run release.yml`) only builds; the archives are attached to the
run as artifacts and no release is created. The macOS builds are not signed.

## License

MIT; see [LICENSE](LICENSE).
