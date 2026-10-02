//! Alerts already printed, kept for seven days so later Updates and Cancels
//! that reference them pass the filter. One "unix_secs<TAB>sender,identifier"
//! per line; a missing or unwritable file just means no memory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const KEEP_SECS: u64 = 7 * 24 * 3600;

pub struct Seen {
    path: Option<PathBuf>,
    entries: HashMap<String, u64>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Per-user state directory: %LOCALAPPDATA% on Windows, Application Support
/// on macOS, $XDG_STATE_HOME or ~/.local/state elsewhere.
pub fn default_path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let dir = if cfg!(windows) {
        env("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support")
    } else {
        env("XDG_STATE_HOME").or_else(|| env("HOME").map(|h| h.join(".local/state")))?
    };
    Some(dir.join("ipawsClient").join("seen.txt"))
}

/// "sender,identifier" — CAP's own key for a message.
pub fn key(alert: &Value) -> String {
    let s = |k: &str| alert[k].as_str().unwrap_or("").trim().to_string();
    format!("{},{}", s("sender"), s("identifier"))
}

/// Keys named by <references>: space-separated "sender,identifier,sent" triples.
pub fn references(alert: &Value) -> Vec<String> {
    alert["references"]
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|r| {
            let mut f = r.split(',');
            Some(format!("{},{}", f.next()?, f.next()?))
        })
        .collect()
}

impl Seen {
    pub fn load(path: Option<PathBuf>) -> Self {
        let cutoff = now().saturating_sub(KEEP_SECS);
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (t, k) = l.split_once('\t')?;
                let t: u64 = t.parse().ok()?;
                (t >= cutoff).then(|| (k.to_string(), t))
            })
            .collect();
        Seen { path, entries }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert(&mut self, key: String) {
        let cutoff = now().saturating_sub(KEEP_SECS);
        self.entries.retain(|_, t| *t >= cutoff);
        self.entries.insert(key, now());
        let Some(path) = &self.path else { return };
        let body: String = self.entries.iter().map(|(k, t)| format!("{t}\t{k}\n")).collect();
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, body)
        };
        if let Err(e) = write() {
            crate::note(format!("cannot save {}: {e}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_and_references() {
        let a = json!({"sender": "w-nws.webmaster@noaa.gov", "identifier": "urn:oid:2.49.1",
                       "references": "w-nws.webmaster@noaa.gov,urn:oid:2.49.0,2026-09-14T19:07:00-07:00 x@y,id2,2026-09-14T20:00:00-07:00"});
        assert_eq!(key(&a), "w-nws.webmaster@noaa.gov,urn:oid:2.49.1");
        assert_eq!(references(&a), ["w-nws.webmaster@noaa.gov,urn:oid:2.49.0", "x@y,id2"]);
        assert!(references(&json!({})).is_empty());
    }

    #[test]
    fn persists_and_expires() {
        let path = std::env::temp_dir().join(format!("ipawsClient-test-{}/seen.txt", std::process::id()));
        let stale = now() - KEEP_SECS - 10;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{stale}\told,one\nnot a line\n")).unwrap();

        let mut s = Seen::load(Some(path.clone()));
        assert!(!s.contains("old,one"), "older than seven days is dropped");
        s.insert("a@b,1".into());
        assert!(Seen::load(Some(path.clone())).contains("a@b,1"), "survives a restart");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
