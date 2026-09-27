//! ipawsClient [grid square] [xml|raw|--xml|--raw] [--server HOST[:PORT]]
//!
//! Looks the square up on the ipaws_on_44 gridsquare service, subscribes to
//! its MQTT alert feed, and prints each alert that concerns the square; with
//! no square, prints every alert.

mod filter;
mod seen;

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::exit;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde_json::Value;

const DEFAULT_SERVER: &str = "44.27.128.55";
const SEPARATOR: &str = "------------------------------------------------------------------------";
const USAGE: &str = "usage: ipawsClient [grid square] [xml|raw|--xml|--raw] [--server HOST[:PORT]]

Prints IPAWS alerts that concern a 4- or 6-character Maidenhead grid square,
or every alert if no square is given, as pretty-printed JSON by default, or
as the original CAP XML with xml/raw. Alerts are separated by a line of
hyphens. --server defaults to 44.27.128.55; PORT is the lookup service's
HTTP port (default 80); MQTT is always HOST:1883.";

#[derive(Debug, PartialEq)]
struct Args {
    square: Option<String>,
    raw: bool,
    server: String,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut square, mut raw, mut server) = (None, false, DEFAULT_SERVER.to_string());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_ascii_lowercase().as_str() {
            "xml" | "raw" | "--xml" | "--raw" => raw = true,
            "--server" => server = it.next().ok_or("--server needs a host")?.clone(),
            s if s.starts_with('-') => return Err(format!("unknown option {a}")),
            _ if square.is_none() => square = Some(a.clone()),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    Ok(Args { square, raw, server })
}

/// "host" or "host:port" -> (host, HTTP port); MQTT always uses host:1883.
fn split_server(server: &str) -> (&str, u16) {
    match server.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => p.parse().map_or((server, 80), |p| (h, p)),
        _ => (server, 80),
    }
}

/// GET http://host:port/path, HTTP/1.0: status and body.
fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = (host, port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: ipawsClient/{}\r\n\r\n", env!("CARGO_PKG_VERSION"))
        .map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let (head, body) = resp.split_once("\r\n\r\n").ok_or("malformed HTTP response")?;
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or("malformed status line")?;
    Ok((status, body.to_string()))
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

fn fail(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("ipawsClient: {msg}");
    exit(code)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let args = parse_args(&argv).unwrap_or_else(|e| fail(2, format!("{e}\n{USAGE}")));
    let (host, http_port) = split_server(&args.server);
    let topic = if args.raw { "ipaws/cap/raw" } else { "ipaws/cap/json" };

    // No square: pass everything, so no lookup and no memory of printed alerts.
    let mut filtering = args.square.as_deref().map(|code| {
        let bounds = filter::decode(code)
            .unwrap_or_else(|e| fail(2, format!("{code:?} is not a Maidenhead square: {e}")));
        let (status, body) = http_get(host, http_port, &format!("/v1/squares/{}", code.trim()))
            .unwrap_or_else(|e| fail(1, format!("lookup at {} failed: {e}", args.server)));
        let info: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| fail(1, format!("lookup at {} returned HTTP {status}, unreadable: {e}", args.server)));
        if status != 200 {
            fail(1, format!("lookup at {} returned HTTP {status}: {info}", args.server));
        }
        let (fips, ugc) = (string_list(&info["fips"]), string_list(&info["ugc"]));
        eprintln!(
            "ipawsClient: watching {code} ({} counties: {}; {} UGC codes) on {host}:1883 {topic}",
            fips.len(),
            fips.join(" "),
            ugc.len()
        );
        let seen_path = seen::default_path();
        if seen_path.is_none() {
            eprintln!("ipawsClient: no state directory; updates to earlier alerts won't be recognized");
        }
        let partials = string_list(&info["same_partial"]); // absent from older servers: empty
        (filter::Square::new(bounds, &fips, &ugc, &partials), seen::Seen::load(seen_path))
    });
    if filtering.is_none() {
        eprintln!("ipawsClient: passing all alerts on {host}:1883 {topic}");
    }

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let mut opts = MqttOptions::new(format!("ipawsClient-{}-{nanos}", std::process::id()), host, 1883);
    opts.set_keep_alive(Duration::from_secs(60));
    opts.set_max_packet_size(1 << 20, 1 << 20);
    let (client, mut connection) = Client::new(opts, 16);

    let stdout = std::io::stdout();
    for event in connection.iter() {
        match event {
            // Subscribe on every (re)connect: the session is clean.
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                if let Err(e) = client.try_subscribe(topic, QoS::AtLeastOnce) {
                    eprintln!("ipawsClient: subscribe: {e}");
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
                            eprintln!("ipawsClient: skipping unreadable message: {e}");
                            continue;
                        }
                    }
                };
                let key = seen::key(&alert);
                if let Some((square, seen)) = &filtering {
                    if seen.contains(&key) {
                        continue; // already printed (e.g. redelivered)
                    }
                    if !square.matches(&alert) && !seen::references(&alert).iter().any(|r| seen.contains(r)) {
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
                eprintln!("ipawsClient: connection to {host}:1883: {e}; retrying");
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
        let json = Args { square: Some("CM87vh".into()), raw: false, server: DEFAULT_SERVER.into() };
        assert_eq!(p(&["CM87vh"]), Ok(json));
        for flag in ["xml", "raw", "--xml", "--raw", "XML", "--RAW"] {
            assert!(p(&["CM87vh", flag]).unwrap().raw, "{flag}");
            assert!(p(&[flag, "CM87vh"]).unwrap().raw, "{flag} first");
        }
        assert_eq!(p(&["CM87", "--server", "127.0.0.1"]).unwrap().server, "127.0.0.1");
        let all = Args { square: None, raw: false, server: DEFAULT_SERVER.into() };
        assert_eq!(p(&[]), Ok(all), "no arguments: every alert, as JSON");
        assert_eq!(p(&["raw"]).unwrap().square, None);
        assert!(p(&["CM87", "CM88"]).is_err());
        assert!(p(&["CM87", "--bogus"]).is_err());
        assert!(p(&["CM87", "--server"]).is_err());
    }

    #[test]
    fn server_ports() {
        assert_eq!(split_server("44.27.128.55"), ("44.27.128.55", 80));
        assert_eq!(split_server("127.0.0.1:8098"), ("127.0.0.1", 8098));
        assert_eq!(split_server("host.example:x"), ("host.example:x", 80));
    }
}
