//! ipawsClient [grid square] [xml|raw|--xml|--raw] [--server [http[s]://]HOST[:PORT]]
//!
//! Looks the square up on the ipaws_on_44 gridsquare service, subscribes to
//! its MQTT alert feed, and prints each alert that concerns the square; with
//! no square, prints every alert.

mod filter;
mod seen;

use std::collections::HashSet;
use std::io::Write;
use std::process::exit;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde_json::Value;

const DEFAULT_SERVER: &str = "ipaws.kd6o.ampr.org";
const SEPARATOR: &str = "------------------------------------------------------------------------";
const USAGE: &str = "usage: ipawsClient [grid square ...] [xml|raw|--xml|--raw] [--server [http[s]://]HOST[:PORT]]

Prints IPAWS alerts that concern one or more 4- or 6-character Maidenhead grid
squares, or every alert if no square is given, as pretty-printed JSON by
default, or as the original CAP XML with xml/raw. Alerts are separated by a line
of hyphens. --server defaults to ipaws.kd6o.ampr.org. The lookup goes to
https://HOST[:PORT] unless http:// is given; MQTT is always HOST:1883.";

#[derive(Debug, PartialEq)]
struct Args {
    squares: Vec<String>,
    raw: bool,
    server: String,
}

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

/// "[http[s]://]host[:port]" -> (lookup base URL, MQTT host). HTTPS unless
/// http:// is given; MQTT always uses host:1883.
fn split_server(server: &str) -> Result<(String, &str), String> {
    let (scheme, rest) = server.split_once("://").unwrap_or(("https", server));
    if scheme != "https" && scheme != "http" {
        return Err(format!("unsupported scheme {scheme}:// in --server"));
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => authority,
    };
    if host.is_empty() {
        return Err(format!("no host in --server {server}"));
    }
    Ok((format!("{scheme}://{authority}"), host))
}

/// GET a URL: status and body, whatever the status.
fn http_get(url: &str) -> Result<(u16, String), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(format!("ipawsClient/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    Ok((status, body))
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// "2026-10-02T06:23:45Z" for seconds since the Unix epoch.
fn utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil date from day count (H. Hinnant's days-to-civil algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A status or error line on stderr, timestamped in UTC.
pub fn note(msg: impl std::fmt::Display) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    eprintln!("{} ipawsClient: {msg}", utc(now));
}

fn fail(code: i32, msg: impl std::fmt::Display) -> ! {
    note(msg);
    exit(code)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let args = parse_args(&argv).unwrap_or_else(|e| fail(2, format!("{e}\n{USAGE}")));
    let (base, host) = split_server(&args.server).unwrap_or_else(|e| fail(2, format!("{e}\n{USAGE}")));
    let topic = if args.raw { "ipaws/cap/raw" } else { "ipaws/cap/json" };

    // No square: pass everything, so no lookup and no memory of printed alerts.
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

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let mut opts = MqttOptions::new(format!("ipawsClient-{}-{nanos}", std::process::id()), host, 1883);
    opts.set_keep_alive(Duration::from_secs(60));
    opts.set_max_packet_size(1 << 20, 1 << 20);
    let (client, mut connection) = Client::new(opts, 16);

    let stdout = std::io::stdout();
    let mut connected_once = false;
    let mut down_since: Option<std::time::Instant> = None;
    for event in connection.iter() {
        match event {
            // Subscribe on every (re)connect: the session is clean.
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                let how = match (connected_once, down_since.take()) {
                    (false, _) => format!("connected to {host}:1883"),
                    (true, Some(t)) => format!("reconnected to {host}:1883 after {} s", t.elapsed().as_secs()),
                    (true, None) => format!("reconnected to {host}:1883"),
                };
                connected_once = true;
                note(format!("{how}; subscribing to {topic}"));
                if let Err(e) = client.try_subscribe(topic, QoS::AtLeastOnce) {
                    note(format!("subscribe: {e}"));
                }
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let payload = &p.payload[..];
                // Parse unless passing raw XML through unfiltered.
                let alert = if args.raw && filtering.is_none() {
                    Value::Null
                } else {
                    let parsed = if args.raw {
                        std::str::from_utf8(payload)
                            .map_err(|e| e.to_string())
                            .and_then(jcap::parse_alert)
                            .and_then(|a| serde_json::to_value(a).map_err(|e| e.to_string()))
                    } else {
                        serde_json::from_slice::<Value>(payload).map_err(|e| e.to_string())
                    };
                    match parsed {
                        Ok(v) => v,
                        Err(e) => {
                            note(format!("skipping unreadable message: {e}"));
                            continue;
                        }
                    }
                };
                let key = seen::key(&alert);
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
                let mut out = stdout.lock();
                let written = if args.raw {
                    out.write_all(payload).and_then(|_| {
                        if payload.ends_with(b"\n") { Ok(()) } else { out.write_all(b"\n") }
                    })
                } else {
                    writeln!(out, "{}", serde_json::to_string_pretty(&alert).expect("Value serializes"))
                };
                if written.and_then(|_| writeln!(out, "{SEPARATOR}")).and_then(|_| out.flush()).is_err() {
                    exit(0); // stdout closed (e.g. piped into head)
                }
                if let Some((_, seen)) = &mut filtering {
                    seen.insert(key);
                }
            }
            Ok(_) => {}
            Err(e) => {
                down_since.get_or_insert_with(std::time::Instant::now);
                note(format!("connection to {host}:1883: {e}; retrying"));
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Args, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

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

    #[test]
    fn utc_timestamps() {
        // Expected values from macOS `date -u -r <secs>`.
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_790_878_748), "2026-10-01T18:19:08Z");
        assert_eq!(utc(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn server_forms() {
        let ok = |s: &str| split_server(s).map(|(b, h)| (b, h.to_string())).unwrap();
        assert_eq!(ok("ipaws.kd6o.ampr.org"), ("https://ipaws.kd6o.ampr.org".into(), "ipaws.kd6o.ampr.org".into()));
        assert_eq!(ok("example.org:8443"), ("https://example.org:8443".into(), "example.org".into()));
        assert_eq!(ok("http://127.0.0.1:8098"), ("http://127.0.0.1:8098".into(), "127.0.0.1".into()));
        assert_eq!(ok("https://example.org/"), ("https://example.org".into(), "example.org".into()));
        assert!(split_server("ftp://example.org").is_err());
        assert!(split_server("https://").is_err());
    }
}
