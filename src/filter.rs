//! Does an alert (as CAP-shaped JSON, jcap conventions) concern our square?
//!
//! Per area, in order:
//! 1. a SAME geocode naming one of the square's counties, its whole state
//!    (xx000), the whole US (000000), or one of the NWS partial-county
//!    partitions the square touches (first digit 1-9);
//! 2. a polygon or circle containing the square's centroid or any corner;
//! 3. only if the area has neither SAME codes nor geometry: a UGC geocode in
//!    the square's UGC list.

use std::collections::HashSet;

use serde_json::Value;

/// (west, south, east, north) degrees for a 4- or 6-character Maidenhead square.
pub fn decode(code: &str) -> Result<(f64, f64, f64, f64), String> {
    let c: Vec<char> = code.trim().chars().collect();
    if c.len() != 4 && c.len() != 6 {
        return Err(format!("expected 4 or 6 characters, got {}", c.len()));
    }
    let letter = |ch: char, first: char, last: char| -> Result<f64, String> {
        let ch = if first.is_ascii_uppercase() { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() };
        if (first..=last).contains(&ch) {
            Ok((ch as u8 - first as u8) as f64)
        } else {
            Err(format!("{ch:?} not in {first}-{last}"))
        }
    };
    let digit = |ch: char| ch.to_digit(10).map(f64::from).ok_or(format!("{ch:?} not a digit"));
    let lon0 = letter(c[0], 'A', 'R')? * 20.0 - 180.0 + digit(c[2])? * 2.0;
    let lat0 = letter(c[1], 'A', 'R')? * 10.0 - 90.0 + digit(c[3])?;
    if c.len() == 4 {
        return Ok((lon0, lat0, lon0 + 2.0, lat0 + 1.0));
    }
    let (dlon, dlat) = (2.0 / 24.0, 1.0 / 24.0);
    let x0 = lon0 + letter(c[4], 'a', 'x')? * dlon;
    let y0 = lat0 + letter(c[5], 'a', 'x')? * dlat;
    Ok((x0, y0, x0 + dlon, y0 + dlat))
}

pub struct Square {
    fips: HashSet<String>,
    states: HashSet<String>,
    ugc: HashSet<String>,
    partials: HashSet<String>,
    points: [(f64, f64); 5], // (lat, lon): centroid, then the four corners
}

impl Square {
    pub fn new(bounds: (f64, f64, f64, f64), fips: &[String], ugc: &[String], partials: &[String]) -> Self {
        let (w, s, e, n) = bounds;
        Square {
            fips: fips.iter().cloned().collect(),
            states: fips.iter().filter_map(|f| f.get(..2).map(String::from)).collect(),
            ugc: ugc.iter().cloned().collect(),
            partials: partials.iter().cloned().collect(),
            points: [((s + n) / 2.0, (w + e) / 2.0), (s, w), (s, e), (n, e), (n, w)],
        }
    }

    pub fn matches(&self, alert: &Value) -> bool {
        arr(&alert["info"]).iter().flat_map(|i| arr(&i["area"])).any(|a| self.area_matches(a))
    }

    fn area_matches(&self, area: &Value) -> bool {
        let codes = |name: &str| -> Vec<&str> {
            arr(&area["geocode"])
                .iter()
                .filter(|g| g["valueName"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(name)))
                .filter_map(|g| g["value"].as_str().map(str::trim))
                .collect()
        };
        let same = codes("SAME");
        let polygons = strings(&area["polygon"]);
        let circles = strings(&area["circle"]);

        if same.iter().any(|v| self.same_matches(v)) {
            return true;
        }
        if self.points.iter().any(|&(lat, lon)| {
            polygons.iter().any(|p| polygon_contains(p, lat, lon))
                || circles.iter().any(|c| circle_contains(c, lat, lon))
        }) {
            return true;
        }
        same.is_empty() && polygons.is_empty() && circles.is_empty()
            && codes("UGC").iter().any(|u| self.ugc.contains(*u))
    }

    fn same_matches(&self, v: &str) -> bool {
        if v.len() != 6 || !v.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if !v.starts_with('0') {
            return self.partials.contains(v); // a partial-county partition
        }
        match (&v[1..3], &v[3..]) {
            ("00", "000") => true, // the whole US
            (state, "000") => self.states.contains(state),
            _ => self.fips.contains(&v[1..]),
        }
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn strings(v: &Value) -> Vec<&str> {
    arr(v).iter().filter_map(Value::as_str).collect()
}

fn latlon(pair: &str) -> Option<(f64, f64)> {
    let (lat, lon) = pair.split_once(',')?;
    Some((lat.trim().parse().ok()?, lon.trim().parse().ok()?))
}

/// CAP polygon: "lat,lon lat,lon ..." (closed). Malformed input never matches.
fn polygon_contains(polygon: &str, lat: f64, lon: f64) -> bool {
    let Some(ring) = polygon.split_whitespace().map(latlon).collect::<Option<Vec<_>>>() else {
        return false;
    };
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let (mut yj, mut xj) = ring[ring.len() - 1];
    for &(yi, xi) in &ring {
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        (yj, xj) = (yi, xi);
    }
    inside
}

/// CAP circle: "lat,lon radius_km".
fn circle_contains(circle: &str, lat: f64, lon: f64) -> bool {
    let mut parts = circle.split_whitespace();
    let (Some(Some((clat, clon))), Some(Ok(radius))) =
        (parts.next().map(latlon), parts.next().map(str::parse::<f64>))
    else {
        return false;
    };
    let (p1, p2) = (clat.to_radians(), lat.to_radians());
    let (dp, dl) = ((lat - clat).to_radians(), (lon - clon).to_radians());
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6371.0088 * h.sqrt().asin() <= radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn square(code: &str, fips: &[&str], ugc: &[&str]) -> Square {
        square_p(code, fips, ugc, &[])
    }

    fn square_p(code: &str, fips: &[&str], ugc: &[&str], partials: &[&str]) -> Square {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        Square::new(decode(code).unwrap(), &s(fips), &s(ugc), &s(partials))
    }

    fn alert(area: Value) -> Value {
        json!({"identifier": "x", "info": [{"area": [area]}]})
    }

    fn geo(name: &str, value: &str) -> Value {
        json!({"valueName": name, "value": value})
    }

    #[test]
    fn decodes_and_rejects() {
        assert_eq!(decode("CM87").unwrap(), (-124.0, 37.0, -122.0, 38.0));
        for bad in ["CM9", "CM95hsx", "ZM95", "CM9A", "CM95z5", ""] {
            assert!(decode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn same_codes() {
        let sq = square("CM87vh", &["06081", "06085"], &[]);
        assert!(sq.matches(&alert(json!({"geocode": [geo("SAME", "006085")]}))), "county");
        assert!(sq.matches(&alert(json!({"geocode": [geo("SAME", "006000")]}))), "statewide");
        assert!(sq.matches(&alert(json!({"geocode": [geo("SAME", "000000")]}))), "nationwide");
        assert!(!sq.matches(&alert(json!({"geocode": [geo("SAME", "506085")]}))), "partition not in the list");
        assert!(!sq.matches(&alert(json!({"geocode": [geo("SAME", "006001")]}))), "other county");
        assert!(!sq.matches(&alert(json!({"geocode": [geo("SAME", "004000")]}))), "other state");
    }

    #[test]
    fn partial_county_codes() {
        // DM79 touches Western and Central Arapahoe (408005, 508005), not Eastern (608005).
        let sq = square_p("DM79", &["08005"], &[], &["408005", "508005"]);
        assert!(sq.matches(&alert(json!({"geocode": [geo("SAME", "508005")]}))), "partition in the square");
        assert!(!sq.matches(&alert(json!({"geocode": [geo("SAME", "608005")]}))), "other partition, same county");
        assert!(sq.matches(&alert(json!({"geocode": [geo("SAME", "008005")]}))), "whole county still matches");
    }

    #[test]
    fn geometry_uses_centroid_and_corners() {
        // CM87vh spans lon -122.25..-122.1667, lat 37.2917..37.3333.
        let sq = square("CM87vh", &[], &[]);
        let sw_corner_only = "37.28,-122.26 37.28,-122.24 37.295,-122.24 37.295,-122.26 37.28,-122.26";
        assert!(sq.matches(&alert(json!({"polygon": [sw_corner_only]}))), "polygon around one corner");
        let elsewhere = "37.0,-121.0 37.0,-120.9 37.1,-120.9 37.0,-121.0";
        assert!(!sq.matches(&alert(json!({"polygon": [elsewhere]}))));
        assert!(sq.matches(&alert(json!({"circle": ["37.3125,-122.2083 1.0"]}))), "circle at centroid");
        assert!(!sq.matches(&alert(json!({"circle": ["37.5,-122.2083 1.0"]}))), "circle 20 km north");
        assert!(!sq.matches(&alert(json!({"polygon": ["garbage"], "circle": ["1,2"]}))), "malformed");
    }

    #[test]
    fn ugc_only_without_same_or_geometry() {
        let sq = square("CM87vh", &["06085"], &["CAZ512", "CAC085"]);
        assert!(sq.matches(&alert(json!({"geocode": [geo("UGC", "CAZ512")]}))), "UGC alone");
        let with_same = json!({"geocode": [geo("SAME", "006001"), geo("UGC", "CAZ512")]});
        assert!(!sq.matches(&alert(with_same)), "UGC ignored when SAME is present");
        let with_poly = json!({"polygon": ["37.0,-121.0 37.0,-120.9 37.1,-120.9 37.0,-121.0"], "geocode": [geo("UGC", "CAZ512")]});
        assert!(!sq.matches(&alert(with_poly)), "UGC ignored when geometry is present");
    }

    #[test]
    fn real_alert_json() {
        // NWS Tucson Flash Flood Warning 2026-09-14, as jcap transcodes it.
        let a = alert(json!({
            "areaDesc": "Pima, AZ",
            "polygon": ["32.22,-111.2 32.18,-111.31 32.32,-111.36 32.32,-111.38 32.08,-111.39 32.15,-111.23 32.1,-111.17 32.18,-111.13 32.22,-111.2"],
            "geocode": [geo("SAME", "004019"), geo("UGC", "AZC019")]
        }));
        assert!(square("DM42", &["04019", "04023"], &[]).matches(&a), "Tucson's 4-char square");
        assert!(!square("CM87", &["06081", "06085"], &[]).matches(&a));
    }
}
