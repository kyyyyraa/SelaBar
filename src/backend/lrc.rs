use crate::events::{LrcLine, LrcResult};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const ENDPOINT: &str = "https://lrclib.net";
const UA: &str = "SelaBar/0.2.0 (lyrics)";
const CACHE_CAP: usize = 25;

#[derive(Clone)]
struct CacheState {
    map: HashMap<String, LrcResult>,
    order: Vec<String>,
}

fn cache() -> &'static Mutex<CacheState> {
    static CELL: OnceLock<Mutex<CacheState>> = OnceLock::new();
    CELL.get_or_init(|| {
        Mutex::new(CacheState {
            map: HashMap::new(),
            order: Vec::new(),
        })
    })
}

fn cache_get(key: &str) -> Option<LrcResult> {
    cache().lock().ok()?.map.get(key).cloned()
}

fn cache_put(key: String, val: LrcResult) {
    if let Ok(mut st) = cache().lock() {
        if st.map.contains_key(&key) {
            st.map.insert(key, val);
            return;
        }
        st.map.insert(key.clone(), val);
        st.order.push(key);
        if st.order.len() > CACHE_CAP {
            let old = st.order.remove(0);
            st.map.remove(&old);
        }
    }
}

/// Strip YouTube junk from a raw media title safely (sorts/merges bracket ranges & respects UTF-8 boundaries).
pub fn clean_title(raw: &str) -> String {
    let mut t = raw.trim().to_string();
    let junk_words = [
        "official",
        "video",
        "lyric",
        "audio",
        "mv",
        "hd",
        "4k",
        "visualizer",
        "color coded",
        "sub",
        "temps",
        "m/v",
        "music video",
        "performance",
    ];

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (open, close) in [('{', '}'), ('[', ']'), ('(', ')')] {
        let mut search_from = 0usize;
        while search_from < t.len() {
            let Some(rel_open) = t[search_from..].find(open) else {
                break;
            };
            let start = search_from + rel_open;
            let after_open = start + open.len_utf8();
            if let Some(rel_close) = t[after_open..].find(close) {
                let end_inner = after_open + rel_close;
                let end_outer = end_inner + close.len_utf8();
                let inner = t[after_open..end_inner].to_lowercase();
                if junk_words.iter().any(|w| inner.contains(w)) {
                    ranges.push((start, end_outer));
                }
                search_from = end_outer;
            } else {
                break;
            }
        }
    }

    if !ranges.is_empty() {
        ranges.sort_by_key(|r| (r.0, r.1));
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (a, b) in ranges {
            if let Some(last) = merged.last_mut() {
                if a <= last.1 {
                    last.1 = last.1.max(b);
                    continue;
                }
            }
            merged.push((a, b));
        }

        let mut out = String::new();
        let mut last = 0;
        for (a, b) in merged {
            if a >= last && b <= t.len() && t.is_char_boundary(a) && t.is_char_boundary(b) {
                out.push_str(&t[last..a]);
                last = b;
            }
        }
        out.push_str(&t[last..]);
        t = out;
    }

    if let Some(p) = t.find('|') {
        t.truncate(p);
    }

    loop {
        let trimmed = t.trim().to_lowercase();
        let changed = ["hd", "4k", "official", "video", "lyric", "lyrics", "audio"]
            .iter()
            .any(|w| {
                trimmed
                    .rsplit_once(' ')
                    .map(|(_, last)| last == *w)
                    .unwrap_or(false)
            });
        if changed {
            t = t
                .trim()
                .rsplit_once(' ')
                .map(|(a, _)| a.to_string())
                .unwrap_or(t);
        } else {
            break;
        }
    }

    let t = t.replace(['"', '\u{201c}', '\u{201d}'], "");
    t.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

pub fn split_artist(raw: &str) -> (String, String) {
    let t = raw.trim();
    for sep in [" - ", " – ", " — "] {
        if let Some(p) = t.find(sep) {
            let a = t[..p].trim().to_string();
            let b = t[p + sep.len()..].trim().to_string();
            if !a.is_empty() && !b.is_empty() {
                return (a, b);
            }
        }
    }
    (String::new(), t.to_string())
}

pub fn parse_lrc(lrc: &str) -> Vec<(u64, String)> {
    let mut offset_ms: i64 = 0;
    for cap in lrc.split('\n') {
        let low = cap.trim().to_lowercase();
        if low.starts_with("[offset:") && low.ends_with(']') {
            offset_ms = low[8..low.len() - 1].trim().parse::<i64>().unwrap_or(0);
            break;
        }
    }

    let mut out: Vec<(u64, String)> = Vec::new();
    for line in lrc.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut stamps: Vec<u64> = Vec::new();
        let mut rest = line;
        loop {
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('[') {
                break;
            }
            let Some(close) = trimmed.find(']') else {
                break;
            };
            let tag = &trimmed[1..close];
            let after = &trimmed[close + 1..];
            // UTF-8 safe check: metadata tags start with >=2 ASCII alphabetic chars (ar, ti, al, ...)
            let mut chars = tag.chars();
            let c0 = chars.next();
            let c1 = chars.next();
            let is_meta = matches!((c0, c1), (Some(a), Some(b)) if a.is_ascii_alphabetic() && b.is_ascii_alphabetic());
            if is_meta {
                rest = after;
                continue;
            }

            let mut parts = tag.split([':', '.']);
            let mm = parts.next().unwrap_or("");
            let ss = parts.next().unwrap_or("");
            let frac = parts.next().unwrap_or("");
            if mm.trim().parse::<u64>().is_err() || ss.trim().parse::<u64>().is_err() {
                break;
            }
            let frac_trimmed = frac.trim();
            let frac_ms = match frac_trimmed.len() {
                0 => 0,
                1 => frac_trimmed.parse::<u64>().unwrap_or(0) * 100,
                2 => frac_trimmed.parse::<u64>().unwrap_or(0) * 10,
                _ => {
                    let f: String = frac_trimmed.chars().take(3).collect();
                    f.parse::<u64>().unwrap_or(0)
                }
            };
            stamps.push(
                mm.trim().parse::<u64>().unwrap_or(0) * 60_000
                    + ss.trim().parse::<u64>().unwrap_or(0) * 1000
                    + frac_ms,
            );
            rest = after;
        }
        let text = rest.trim().to_string();
        if stamps.is_empty() || text.is_empty() {
            continue;
        }
        for t in stamps {
            let adj = (t as i64 + offset_ms).max(0) as u64;
            out.push((adj, text.clone()));
        }
    }
    out.sort_by_key(|(t, _)| *t);
    out.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    out
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|e| e.to_string())
}

async fn api_get(cl: &reqwest::Client, artist: &str, track: &str) -> Option<Value> {
    let url = format!(
        "{}/api/get?artist_name={}&track_name={}",
        ENDPOINT,
        urlencode(artist),
        urlencode(track)
    );
    let resp = cl.get(&url).send().await.ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    resp.json::<Value>().await.ok()
}

async fn api_search(cl: &reqwest::Client, q: &str) -> Vec<Value> {
    let url = format!("{}/api/search?q={}", ENDPOINT, urlencode(q));
    let Ok(resp) = cl.get(&url).send().await else {
        return Vec::new();
    };
    if resp.status().as_u16() != 200 {
        return Vec::new();
    }
    resp.json::<Vec<Value>>().await.unwrap_or_default()
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn j_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn j_f64(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn duration_ok(resp: &Value, want: f64) -> bool {
    if want <= 0.0 {
        return true;
    }
    let d = j_f64(resp, "duration");
    d <= 0.0 || (d - want).abs() <= 3.0
}

fn norm_t(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn title_plausible(want: &str, resp: &Value) -> bool {
    let got = j_str(resp, "trackName");
    if got.is_empty() {
        return true;
    }
    let w = norm_t(want);
    let g = norm_t(&got);
    if w.is_empty() || g.is_empty() {
        return true;
    }
    w.contains(&g) || g.contains(&w)
}

fn has_synced(resp: &Value) -> bool {
    !j_str(resp, "syncedLyrics").is_empty()
}

fn pick_from_search(list: &[Value], want: f64, want_title: &str) -> Option<Value> {
    let plausible = |v: &Value| title_plausible(want_title, v);
    if let Some(v) = list
        .iter()
        .find(|v| has_synced(v) && duration_ok(v, want) && plausible(v))
    {
        return Some(v.clone());
    }
    if let Some(v) = list
        .iter()
        .find(|v| !j_str(v, "plainLyrics").is_empty() && duration_ok(v, want) && plausible(v))
    {
        return Some(v.clone());
    }
    list.iter().find(|v| has_synced(v) && plausible(v)).cloned()
}

fn build_result(key: String, resp: &Value) -> LrcResult {
    let synced_raw = j_str(resp, "syncedLyrics");
    let plain = j_str(resp, "plainLyrics");
    let instrumental = resp
        .get("instrumental")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let lines: Vec<LrcLine> = parse_lrc(&synced_raw)
        .into_iter()
        .map(|(t_ms, text)| LrcLine { t_ms, text })
        .collect();
    let status = if instrumental {
        "instrumental"
    } else if !lines.is_empty() {
        "synced"
    } else if !plain.is_empty() {
        "plain"
    } else {
        "not-found"
    }
    .to_string();

    LrcResult {
        key,
        status: status.clone(),
        lines,
        plain: if status == "plain" { Some(plain) } else { None },
        track_name: j_str(resp, "trackName"),
        artist_name: j_str(resp, "artistName"),
    }
}

pub async fn fetch_lyrics(
    key: String,
    raw_title: &str,
    raw_artist: &str,
    want_dur: f64,
) -> LrcResult {
    if raw_title.trim().is_empty() {
        return LrcResult {
            key,
            status: "no-track".to_string(),
            lines: Vec::new(),
            plain: None,
            track_name: String::new(),
            artist_name: String::new(),
        };
    }

    let mut title = raw_title.trim().to_string();
    let mut artist = raw_artist.trim().to_string();

    let cleaned = clean_title(&title);
    if !cleaned.is_empty() && cleaned != title {
        title = cleaned;
    }
    if artist.is_empty() {
        let (a, t) = split_artist(&title);
        if !a.is_empty() {
            artist = a;
            title = t;
        }
    }

    let cache_key = format!(
        "{}|{}|{}",
        artist.to_lowercase(),
        title.to_lowercase(),
        (want_dur * 10.0).round() as i64 / 10
    );
    if let Some(mut v) = cache_get(&cache_key) {
        v.key = key;
        return v;
    }

    let cl = match client() {
        Ok(c) => c,
        Err(_) => {
            return LrcResult {
                key,
                status: "error".to_string(),
                lines: Vec::new(),
                plain: None,
                track_name: title,
                artist_name: artist,
            }
        }
    };

    let mut found: Option<Value> = None;

    if !artist.is_empty() {
        if let Some(j) = api_get(&cl, &artist, &title).await {
            if (has_synced(&j)
                || j.get("instrumental")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false))
                && duration_ok(&j, want_dur)
                && title_plausible(&title, &j)
            {
                found = Some(j);
            }
        }
    }

    if found.is_none() {
        let (a2, t2) = split_artist(&title);
        if !a2.is_empty() && a2 != artist {
            if let Some(j) = api_get(&cl, &a2, &t2).await {
                if (has_synced(&j)
                    || j.get("instrumental")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false))
                    && duration_ok(&j, want_dur)
                    && title_plausible(&t2, &j)
                {
                    found = Some(j);
                }
            }
        }
    }

    if found.is_none() {
        if !artist.is_empty() {
            let list = api_search(&cl, &format!("{} {}", artist, title)).await;
            if let Some(j) = pick_from_search(&list, want_dur, &title) {
                found = Some(j);
            }
        }
        if found.is_none() {
            let list = api_search(&cl, &title).await;
            if let Some(j) = pick_from_search(&list, want_dur, &title) {
                found = Some(j);
            }
        }
    }

    if found.is_none() && !artist.is_empty() {
        if let Some(j) = api_get(&cl, &artist, &title).await {
            if has_synced(&j) && title_plausible(&title, &j) {
                found = Some(j);
            }
        }
    }

    let res = match found {
        Some(j) => build_result(key, &j),
        None => LrcResult {
            key,
            status: "not-found".to_string(),
            lines: Vec::new(),
            plain: None,
            track_name: title,
            artist_name: artist,
        },
    };
    cache_put(cache_key, res.clone());
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_titles_mixed_brackets_no_panic() {
        assert_eq!(
            clean_title("Artist - Lagu (Official Audio) [HD]"),
            "Artist - Lagu"
        );
        assert_eq!(clean_title("Lagu [Official (HD) Video]"), "Lagu");
        assert_eq!(
            clean_title("Tulus - Ruang Sendiri (Official Music Video)"),
            "Tulus - Ruang Sendiri"
        );
        assert_eq!(
            clean_title("Nadin Amizah - Bertaut [Official Lyric Video]"),
            "Nadin Amizah - Bertaut"
        );
        assert_eq!(
            clean_title("Coldplay - Yellow (Official Video) HD"),
            "Coldplay - Yellow"
        );
        assert_eq!(clean_title("Song | Live Session"), "Song");
    }

    #[test]
    fn parses_lrc_including_non_ascii() {
        let lrc = "[ar:X]\n[♪]\n[00:13.24] Baris satu\n[01:02.5] Baris dua\n[00:13.24] Baris satu\n[02:00] Baris tiga";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], (13_240, "Baris satu".into()));
        assert_eq!(lines[1], (62_500, "Baris dua".into()));
        assert_eq!(lines[2], (120_000, "Baris tiga".into()));
    }
}
