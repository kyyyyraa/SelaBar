use crate::backend::{lrc, notif};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 SelaBar/0.2.0";
const MAX_FETCH_BYTES: usize = 512 * 1024;
const CACHE_TTL_SECS: u64 = 120;
const CACHE_MAX_ENTRIES: usize = 64;

fn shared_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(WEB_UA)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .pool_max_idle_per_host(8)
            .build()
            .map_err(|e| format!("http client gagal: {}", e))
    }) {
        Ok(c) => Ok(c),
        Err(e) => Err(e.clone()),
    }
}

struct WebTtlCache {
    map: HashMap<String, (Instant, String)>,
}

impl WebTtlCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn get(&mut self, key: &str, now: Instant) -> Option<String> {
        let ttl = Duration::from_secs(CACHE_TTL_SECS);
        if let Some((ts, val)) = self.map.get(key) {
            if now.saturating_duration_since(*ts) < ttl {
                return Some(val.clone());
            }
        }
        self.map.remove(key);
        None
    }

    fn put(&mut self, key: String, val: String, now: Instant) {
        let ttl = Duration::from_secs(CACHE_TTL_SECS);
        self.map.retain(|_, (ts, _)| now.saturating_duration_since(*ts) < ttl);
        if self.map.len() >= CACHE_MAX_ENTRIES {
            if let Some(oldest_k) = self
                .map
                .iter()
                .min_by_key(|(_, (ts, _))| *ts)
                .map(|(k, _)| k.clone())
            {
                self.map.remove(&oldest_k);
            }
        }
        self.map.insert(key, (now, val));
    }
}

fn web_cache() -> &'static Mutex<WebTtlCache> {
    static CACHE: OnceLock<Mutex<WebTtlCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(WebTtlCache::new()))
}

fn is_forbidden_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let is_cgnat = o[0] == 100 && (o[1] & 0b1100_0000) == 64;
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || o[0] == 0
        || is_cgnat
}

fn is_forbidden_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_forbidden_v4(v4);
    }
    let seg0 = ip.segments()[0];
    let is_ula = (seg0 & 0xfe00) == 0xfc00;
    let is_link_local = (seg0 & 0xffc0) == 0xfe80;
    ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || is_ula || is_link_local
}

pub fn is_public_http_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let low = u.to_lowercase();
    let after = if low.starts_with("https://") {
        &u[8..]
    } else if low.starts_with("http://") {
        &u[7..]
    } else {
        return Err("URL harus diawali http:// atau https://".to_string());
    };

    let authority = after.split(['/', '?', '#']).next().unwrap_or("").trim();
    if authority.is_empty() {
        return Err("Host URL kosong".to_string());
    }
    let host_port = authority.rsplit('@').next().unwrap_or(authority).trim();
    let host = if host_port.starts_with('[') {
        host_port
            .find(']')
            .map(|end| &host_port[1..end])
            .unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    }
    .trim()
    .trim_end_matches('.')
    .to_lowercase();

    if host.is_empty() {
        return Err("Host URL tidak valid".to_string());
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err("URL jaringan lokal/localhost tidak diizinkan".to_string());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(v4) => is_forbidden_v4(v4),
            IpAddr::V6(v6) => is_forbidden_v6(v6),
        };
        if blocked {
            return Err("Alamat IP lokal/privat tidak diizinkan".to_string());
        }
        return Ok(());
    }
    let numeric_like = !host.is_empty()
        && host.split('.').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || matches!(c, 'x' | 'X'))
        });
    if numeric_like {
        return Err("Format alamat IP non-standar tidak diizinkan".to_string());
    }
    if !host.contains('.') {
        return Err("Nama host lokal/tidak lengkap tidak diizinkan".to_string());
    }
    Ok(())
}

fn clean_entities_and_ws(text: &str) -> String {
    let normalized = text
        .replace("&nbsp;", " ")
        .replace("&ndash;", "-")
        .replace("&mdash;", "—")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&ldquo;", "\"")
        .replace("&rdquo;", "\"");
    let decoded = notif::decode_entities(&normalized);
    let mut lines: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for raw in decoded.lines() {
        let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            if !prev_blank && !lines.is_empty() {
                lines.push(String::new());
            }
            prev_blank = true;
        } else {
            lines.push(collapsed);
            prev_blank = false;
        }
    }
    lines.join("\n").trim().to_string()
}

pub fn strip_html_to_text(html: &str) -> String {
    static STRIP_BLOCKS_RE: OnceLock<Regex> = OnceLock::new();
    static BLOCK_BREAK_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();

    let blocks_re = STRIP_BLOCKS_RE.get_or_init(|| {
        Regex::new(r"(?is)<script\b[^>]*>[\s\S]*?</script>|<style\b[^>]*>[\s\S]*?</style>|<noscript\b[^>]*>[\s\S]*?</noscript>|<svg\b[^>]*>[\s\S]*?</svg>|<head\b[^>]*>[\s\S]*?</head>").unwrap()
    });
    let break_re = BLOCK_BREAK_RE.get_or_init(|| {
        Regex::new(r"(?is)</?(?:p|div|section|article|br|li|tr|h[1-6]|blockquote|pre)\b[^>]*>")
            .unwrap()
    });
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").unwrap());

    let no_scripts = blocks_re.replace_all(html, " ");
    let with_breaks = break_re.replace_all(&no_scripts, "\n");
    let no_tags = tag_re.replace_all(&with_breaks, " ");
    clean_entities_and_ws(&no_tags)
}

fn strip_cdata_and_tags(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw);
    strip_html_to_text(trimmed)
        .lines()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_xml_tag<'a>(block: &'a str, tag: &str) -> &'a str {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let Some(start) = block.find(&open) else {
        return "";
    };
    let after = &block[start + open.len()..];
    let Some(gt) = after.find('>') else {
        return "";
    };
    let content = &after[gt + 1..];
    let Some(end) = content.find(&close) else {
        return "";
    };
    &content[..end]
}

pub fn parse_bing_rss_results(xml: &str, limit: usize) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while out.len() < limit {
        let Some(rel_s) = xml[pos..].find("<item>") else {
            break;
        };
        let item_start = pos + rel_s + 6;
        let Some(rel_e) = xml[item_start..].find("</item>") else {
            break;
        };
        let block = &xml[item_start..item_start + rel_e];
        pos = item_start + rel_e + 7;

        let title = strip_cdata_and_tags(extract_xml_tag(block, "title"));
        let link = strip_cdata_and_tags(extract_xml_tag(block, "link"));
        let desc = strip_cdata_and_tags(extract_xml_tag(block, "description"));
        if !title.is_empty() && !link.is_empty() {
            out.push((title, link, desc));
        }
    }
    out
}

fn extract_attr_value(attrs: &str, key: &str) -> String {
    let pat_dq = format!("{}=\"", key);
    if let Some(i) = attrs.find(&pat_dq) {
        let rest = &attrs[i + pat_dq.len()..];
        if let Some(end) = rest.find('"') {
            return notif::decode_entities(&rest[..end]);
        }
    }
    let pat_sq = format!("{}='", key);
    if let Some(i) = attrs.find(&pat_sq) {
        let rest = &attrs[i + pat_sq.len()..];
        if let Some(end) = rest.find('\'') {
            return notif::decode_entities(&rest[..end]);
        }
    }
    String::new()
}

fn unwrap_ddg_redirect(raw_href: &str) -> String {
    let href = raw_href.trim();
    let normalized = if href.starts_with("//") {
        format!("https:{}", href)
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{}", href)
    } else {
        href.to_string()
    };
    if let Some(idx) = normalized.find("uddg=") {
        let enc = normalized[idx + 5..].split('&').next().unwrap_or("");
        let decoded = notif::percent_decode_utf8(&enc.replace('+', " "));
        if decoded.starts_with("http://") || decoded.starts_with("https://") {
            return decoded;
        }
    }
    normalized
}

pub fn parse_ddg_html_results(html: &str, limit: usize) -> Vec<(String, String, String)> {
    static LINK_RE: OnceLock<Regex> = OnceLock::new();
    let link_re =
        LINK_RE.get_or_init(|| Regex::new(r"(?is)<a\b([^>]*)>([\s\S]*?)</a>").unwrap());

    let mut titles: Vec<(usize, String, String)> = Vec::new();
    let mut snippets: Vec<(usize, String)> = Vec::new();

    for cap in link_re.captures_iter(html) {
        let full = cap.get(0).map(|m| m.start()).unwrap_or(0);
        let attrs = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let inner = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let class_attr = extract_attr_value(attrs, "class");
        if class_attr.contains("result__a") {
            let href = unwrap_ddg_redirect(&extract_attr_value(attrs, "href"));
            let title = strip_html_to_text(inner);
            if !title.is_empty() && !href.is_empty() {
                titles.push((full, title, href));
            }
        } else if class_attr.contains("result__snippet") {
            let snip = strip_html_to_text(inner);
            if !snip.is_empty() {
                snippets.push((full, snip));
            }
        }
    }

    let mut out = Vec::new();
    for (i, (pos, title, link)) in titles.iter().enumerate() {
        if out.len() >= limit {
            break;
        }
        let next_pos = titles.get(i + 1).map(|t| t.0).unwrap_or(usize::MAX);
        let snippet = snippets
            .iter()
            .find(|(sp, _)| *sp > *pos && *sp < next_pos)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        out.push((title.clone(), link.clone(), snippet));
    }
    out
}

pub async fn web_fetch(url: &str, max_chars: usize) -> Result<String, String> {
    let trimmed = url.trim();
    is_public_http_url(trimmed)?;
    let cap = max_chars.clamp(400, 6000);
    let cache_key = format!("fetch:{}", trimmed);
    if let Ok(mut g) = web_cache().lock() {
        if let Some(cached) = g.get(&cache_key, Instant::now()) {
            return Ok(cached.chars().take(cap).collect());
        }
    }

    let cl = shared_client()?;
    let resp = cl
        .get(trimmed)
        .header("Accept", "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "id-ID,id;q=0.9,en-US;q=0.8,en;q=0.7")
        .send()
        .await
        .map_err(|e| format!("gagal mengambil halaman web: {}", e))?;

    is_public_http_url(resp.url().as_str())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("halaman mengembalikan HTTP {}", status.as_u16()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("gagal membaca respons web: {}", e))?;
    let slice = if bytes.len() > MAX_FETCH_BYTES {
        &bytes[..MAX_FETCH_BYTES]
    } else {
        &bytes[..]
    };
    let raw = String::from_utf8_lossy(slice);
    let cleaned = if content_type.contains("html") || raw.trim_start().starts_with('<') {
        strip_html_to_text(&raw)
    } else {
        raw.trim().to_string()
    };
    if cleaned.is_empty() {
        return Err("isi halaman kosong atau tidak terbaca".to_string());
    }
    if let Ok(mut g) = web_cache().lock() {
        g.put(cache_key, cleaned.chars().take(6000).collect(), Instant::now());
    }
    Ok(cleaned.chars().take(cap).collect())
}

async fn wikipedia_lang_search(
    cl: &reqwest::Client,
    lang: &str,
    query: &str,
    limit: usize,
) -> Option<String> {
    let api_url = format!(
        "https://{}.wikipedia.org/w/api.php?action=opensearch&search={}&limit={}&namespace=0&format=json",
        lang,
        lrc::urlencode(query),
        limit.clamp(1, 5)
    );
    let resp = cl.get(&api_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v = resp.json::<Value>().await.ok()?;
    let titles = v.get(1).and_then(|x| x.as_array())?;
    let links = v.get(3).and_then(|x| x.as_array())?;
    let first_title = titles.first().and_then(|t| t.as_str())?;
    let first_link = links.first().and_then(|l| l.as_str()).unwrap_or("");
    let sum_url = format!(
        "https://{}.wikipedia.org/api/rest_v1/page/summary/{}",
        lang,
        lrc::urlencode(first_title)
    );
    let sr = cl.get(&sum_url).send().await.ok()?;
    if !sr.status().is_success() {
        return None;
    }
    let sv = sr.json::<Value>().await.ok()?;
    let extract = sv.get("extract").and_then(|x| x.as_str()).unwrap_or("").trim();
    if extract.is_empty() {
        return None;
    }
    Some(format!("1. {}\n   {}\n   URL: {}", first_title, extract, first_link))
}

async fn wikipedia_fallback_search(cl: &reqwest::Client, query: &str, limit: usize) -> Result<String, String> {
    let id_fut = wikipedia_lang_search(cl, "id", query, limit);
    let en_fut = wikipedia_lang_search(cl, "en", query, limit);
    let (id_res, en_res) = tokio::join!(id_fut, en_fut);
    if let Some(s) = id_res.or(en_res) {
        return Ok(s);
    }
    Err("tidak ada hasil pencarian yang ditemukan".to_string())
}

fn format_search_results(items: &[(String, String, String)]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, (title, link, snippet))| {
            if snippet.is_empty() {
                format!("{}. {}\n   URL: {}", i + 1, title, link)
            } else {
                format!("{}. {}\n   {}\n   URL: {}", i + 1, title, snippet, link)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn search_bing_rss(cl: &reqwest::Client, q: &str, lim: usize) -> Option<String> {
    let rss_url = format!(
        "https://www.bing.com/search?q={}&format=rss&setlang=id",
        lrc::urlencode(q)
    );
    let resp = tokio::time::timeout(Duration::from_secs(5), cl.get(&rss_url).send())
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let xml = tokio::time::timeout(Duration::from_secs(4), resp.text())
        .await
        .ok()?
        .ok()?;
    let items = parse_bing_rss_results(&xml, lim);
    if items.is_empty() {
        None
    } else {
        Some(format_search_results(&items))
    }
}

async fn search_ddg_html(cl: &reqwest::Client, q: &str, lim: usize) -> Option<String> {
    let ddg_url = format!("https://html.duckduckgo.com/html/?q={}", lrc::urlencode(q));
    let req = cl
        .get(&ddg_url)
        .header("Accept-Language", "id-ID,id;q=0.9,en;q=0.8")
        .send();
    let resp = tokio::time::timeout(Duration::from_secs(5), req)
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let html = tokio::time::timeout(Duration::from_secs(4), resp.text())
        .await
        .ok()?
        .ok()?;
    let items = parse_ddg_html_results(&html, lim);
    if items.is_empty() {
        None
    } else {
        Some(format_search_results(&items))
    }
}

pub async fn web_search(query: &str, limit: usize) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("query pencarian kosong".to_string());
    }
    let lim = limit.clamp(1, 8);
    let cache_key = format!("search:{}:{}", lim, q.to_lowercase());
    if let Ok(mut g) = web_cache().lock() {
        if let Some(cached) = g.get(&cache_key, Instant::now()) {
            return Ok(cached);
        }
    }

    let cl = shared_client()?;
    let bing_fut = search_bing_rss(cl, q, lim);
    let ddg_fut = search_ddg_html(cl, q, lim);
    tokio::pin!(bing_fut);
    tokio::pin!(ddg_fut);

    let raced = tokio::select! {
        res = &mut bing_fut => {
            match res {
                Some(s) => Some(s),
                None => ddg_fut.await,
            }
        }
        res = &mut ddg_fut => {
            match res {
                Some(s) => Some(s),
                None => bing_fut.await,
            }
        }
    };

    let out = match raced {
        Some(s) => s,
        None => wikipedia_fallback_search(cl, q, lim).await?,
    };
    if let Ok(mut g) = web_cache().lock() {
        g.put(cache_key, out.clone(), Instant::now());
    }
    Ok(out)
}

pub fn wmo_weather_desc_id(code: i64) -> &'static str {
    match code {
        0 => "Cerah",
        1 => "Cerah berawan",
        2 => "Berawan sebagian",
        3 => "Mendung / berawan",
        45 | 48 => "Berkabut",
        51 | 53 | 55 => "Gerimis",
        56 | 57 => "Gerimis dingin",
        61 => "Hujan ringan",
        63 => "Hujan sedang",
        65 => "Hujan lebat",
        66 | 67 => "Hujan dingin",
        71 | 73 | 75 | 77 => "Salju",
        80 => "Hujan lokal ringan",
        81 => "Hujan lokal sedang",
        82 => "Hujan lokal deras",
        85 | 86 => "Hujan salju",
        95 => "Badai petir",
        96 | 99 => "Badai petir disertai hujan es",
        _ => "Berawan",
    }
}

async fn resolve_location_coords(
    cl: &reqwest::Client,
    location: &str,
) -> Result<(f64, f64, String), String> {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        if let Ok(resp) = cl.get("https://ipwho.is/").send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>().await {
                    if v.get("success").and_then(|x| x.as_bool()) == Some(true) {
                        if let (Some(lat), Some(lon)) = (
                            v.get("latitude").and_then(|x| x.as_f64()),
                            v.get("longitude").and_then(|x| x.as_f64()),
                        ) {
                            let city = v.get("city").and_then(|x| x.as_str()).unwrap_or("");
                            let region = v.get("region").and_then(|x| x.as_str()).unwrap_or("");
                            let label = if !city.is_empty() && !region.is_empty() {
                                format!("{}, {}", city, region)
                            } else if !city.is_empty() {
                                city.to_string()
                            } else {
                                "Lokasi IP saat ini".to_string()
                            };
                            return Ok((lat, lon, label));
                        }
                    }
                }
            }
        }
        return Ok((-6.2088, 106.8456, "Jakarta (default)".to_string()));
    }

    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=id&format=json",
        lrc::urlencode(trimmed)
    );
    let resp = cl
        .get(&geo_url)
        .send()
        .await
        .map_err(|e| format!("gagal mencari koordinat kota: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("geocoding mengembalikan HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("gagal membaca data lokasi: {}", e))?;
    let Some(first) = v
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
    else {
        return Err(format!("lokasi '{}' tidak ditemukan", trimmed));
    };
    let lat = first
        .get("latitude")
        .and_then(|x| x.as_f64())
        .ok_or_else(|| "latitude lokasi tidak tersedia".to_string())?;
    let lon = first
        .get("longitude")
        .and_then(|x| x.as_f64())
        .ok_or_else(|| "longitude lokasi tidak tersedia".to_string())?;
    let name = first.get("name").and_then(|x| x.as_str()).unwrap_or(trimmed);
    let country = first.get("country").and_then(|x| x.as_str()).unwrap_or("");
    let label = if country.is_empty() {
        name.to_string()
    } else {
        format!("{}, {}", name, country)
    };
    Ok((lat, lon, label))
}

pub async fn weather(location: &str) -> Result<String, String> {
    let cl = shared_client()?;
    let (lat, lon, label) = resolve_location_coords(cl, location).await?;
    let forecast_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}&current=temperature_2m,relative_humidity_2m,apparent_temperature,precipitation,weather_code,wind_speed_10m&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max&timezone=auto&forecast_days=2",
        lat, lon
    );
    let resp = cl
        .get(&forecast_url)
        .send()
        .await
        .map_err(|e| format!("gagal mengambil prakiraan cuaca: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("layanan cuaca HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("gagal membaca prakiraan cuaca: {}", e))?;
    let cur = v.get("current").ok_or_else(|| "data cuaca saat ini kosong".to_string())?;
    let temp = cur.get("temperature_2m").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let feels = cur.get("apparent_temperature").and_then(|x| x.as_f64()).unwrap_or(temp);
    let hum = cur.get("relative_humidity_2m").and_then(|x| x.as_i64()).unwrap_or(0);
    let wind = cur.get("wind_speed_10m").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let rain = cur.get("precipitation").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let code = cur.get("weather_code").and_then(|x| x.as_i64()).unwrap_or(3);
    let desc = wmo_weather_desc_id(code);

    let mut out = format!(
        "Cuaca di {}: {}, suhu {:.1}°C (terasa {:.1}°C), kelembapan {}%, angin {:.1} km/jam, curah hujan {:.1} mm.",
        label, desc, temp, feels, hum, wind, rain
    );
    if let Some(daily) = v.get("daily") {
        let min0 = daily.get("temperature_2m_min").and_then(|a| a.get(0)).and_then(|x| x.as_f64());
        let max0 = daily.get("temperature_2m_max").and_then(|a| a.get(0)).and_then(|x| x.as_f64());
        let pop0 = daily
            .get("precipitation_probability_max")
            .and_then(|a| a.get(0))
            .and_then(|x| x.as_i64());
        if let (Some(lo), Some(hi)) = (min0, max0) {
            out.push_str(&format!(" Hari ini {:.0}–{:.0}°C", lo, hi));
            if let Some(p) = pop0 {
                out.push_str(&format!(" (peluang hujan {}%)", p));
            }
            out.push('.');
        }
        let min1 = daily.get("temperature_2m_min").and_then(|a| a.get(1)).and_then(|x| x.as_f64());
        let max1 = daily.get("temperature_2m_max").and_then(|a| a.get(1)).and_then(|x| x.as_f64());
        let pop1 = daily
            .get("precipitation_probability_max")
            .and_then(|a| a.get(1))
            .and_then(|x| x.as_i64());
        if let (Some(lo), Some(hi)) = (min1, max1) {
            out.push_str(&format!(" Besok {:.0}–{:.0}°C", lo, hi));
            if let Some(p) = pop1 {
                out.push_str(&format!(" (peluang hujan {}%)", p));
            }
            out.push('.');
        }
    }
    Ok(out)
}

pub fn normalize_currency_code(raw: &str) -> String {
    let t = raw.trim().to_lowercase();
    match t.as_str() {
        "" => "USD".to_string(),
        "dolar" | "dollar" | "usd" | "us$" | "dolar as" | "amerika" => "USD".to_string(),
        "rupiah" | "rp" | "idr" => "IDR".to_string(),
        "yen" | "jpy" | "jepang" => "JPY".to_string(),
        "euro" | "eur" => "EUR".to_string(),
        "ringgit" | "myr" | "malaysia" => "MYR".to_string(),
        "sgd" | "dolar singapura" | "singapura" => "SGD".to_string(),
        "gbp" | "pound" | "poundsterling" | "inggris" => "GBP".to_string(),
        "aud" | "dolar australia" | "australia" => "AUD".to_string(),
        "krw" | "won" | "korea" => "KRW".to_string(),
        "cny" | "yuan" | "rmb" | "china" => "CNY".to_string(),
        "sar" | "riyal" | "arab" => "SAR".to_string(),
        "thb" | "baht" | "thailand" => "THB".to_string(),
        _ => {
            let up: String = t
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .take(3)
                .collect::<String>()
                .to_uppercase();
            if up.len() == 3 { up } else { "USD".to_string() }
        }
    }
}

fn fmt_number_id(val: f64, decimals: usize) -> String {
    if decimals == 0 {
        let rounded = val.round() as i64;
        let s = rounded.abs().to_string();
        let mut rev = String::new();
        for (i, ch) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                rev.push('.');
            }
            rev.push(ch);
        }
        if rounded < 0 {
            rev.push('-');
        }
        return rev.chars().rev().collect();
    }
    format!("{:.prec$}", val, prec = decimals)
}

pub async fn currency_rate(from: &str, to: &str, amount: f64) -> Result<String, String> {
    let base = normalize_currency_code(if from.trim().is_empty() { "USD" } else { from });
    let quote = normalize_currency_code(if to.trim().is_empty() { "IDR" } else { to });
    let amt = if amount.is_finite() && amount > 0.0 {
        amount
    } else {
        1.0
    };
    if base == quote {
        return Ok(format!("{} {} = {} {}.", amt, base, amt, quote));
    }

    let cl = shared_client()?;
    let primary = format!("https://open.er-api.com/v6/latest/{}", base);
    let mut found: Option<(f64, String)> = None;
    if let Ok(resp) = cl.get(&primary).send().await {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<Value>().await {
                if v.get("result").and_then(|r| r.as_str()) == Some("success") {
                    if let Some(rate) = v
                        .get("rates")
                        .and_then(|r| r.get(&quote))
                        .and_then(|x| x.as_f64())
                    {
                        found = Some((rate, "open.er-api.com".to_string()));
                    }
                }
            }
        }
    }
    if found.is_none() {
        let fb = format!("https://api.frankfurter.app/latest?from={}&to={}", base, quote);
        if let Ok(resp) = cl.get(&fb).send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>().await {
                    if let Some(rate) = v
                        .get("rates")
                        .and_then(|r| r.get(&quote))
                        .and_then(|x| x.as_f64())
                    {
                        found = Some((rate, "frankfurter.app".to_string()));
                    }
                }
            }
        }
    }

    let Some((unit_rate, source)) = found else {
        return Err(format!("gagal mengambil kurs {} ke {}", base, quote));
    };
    let converted = amt * unit_rate;
    let conv_str = if quote == "IDR" || converted >= 100.0 {
        fmt_number_id(converted, 0)
    } else {
        fmt_number_id(converted, 4)
    };
    let unit_str = if quote == "IDR" || unit_rate >= 100.0 {
        fmt_number_id(unit_rate, 0)
    } else {
        fmt_number_id(unit_rate, 4)
    };
    Ok(format!(
        "{} {} = {} {} (1 {} = {} {}, sumber: {}).",
        amt, base, conv_str, quote, base, unit_str, quote, source
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_fetch_rejects_local_and_private_urls() {
        assert!(is_public_http_url("https://example.com/berita?a=1").is_ok());
        assert!(is_public_http_url("http://api.open-meteo.com/v1/forecast").is_ok());

        assert!(is_public_http_url("file:///C:/Windows/win.ini").is_err());
        assert!(is_public_http_url("http://localhost:8080/admin").is_err());
        assert!(is_public_http_url("http://127.0.0.1:11434/v1/models").is_err());
        assert!(is_public_http_url("http://192.168.1.1/").is_err());
        assert!(is_public_http_url("http://10.0.0.5/").is_err());
        assert!(is_public_http_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(is_public_http_url("http://[::1]/").is_err());
        assert!(is_public_http_url("http://intranet/").is_err());
        assert!(is_public_http_url("http://127.1/").is_err());
        assert!(is_public_http_url("http://0x7f.0.0.1/").is_err());
        assert!(is_public_http_url("http://0177.0.0.1/").is_err());
        assert!(is_public_http_url("http://2130706433/").is_err());
    }

    #[test]
    fn html_to_text_and_rss_and_ddg_parsers_extract_clean_results() {
        let html = "<html><head><title>x</title><script>var a=1;</script></head><body><h1>Cuaca&nbsp;Bandung</h1><p>Suhu <b>24 C</b> &amp; cerah.</p></body></html>";
        let txt = strip_html_to_text(html);
        assert!(!txt.contains("var a=1"));
        assert!(txt.contains("Cuaca Bandung"));
        assert!(txt.contains("Suhu 24 C & cerah."));

        let rss = r#"<rss><channel><title>Bing</title><item><title><![CDATA[Cuaca Hari Ini]]></title><link>https://example.com/cuaca</link><description>Cerah berawan sepanjang hari.</description></item></channel></rss>"#;
        let bing = parse_bing_rss_results(rss, 5);
        assert_eq!(bing.len(), 1);
        assert_eq!(bing[0].0, "Cuaca Hari Ini");
        assert_eq!(bing[0].1, "https://example.com/cuaca");

        let ddg = r##"<div><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fbmkg.go.id%2Fcuaca&amp;rut=1">BMKG Cuaca</a><a class="result__snippet" href="#">Prakiraan <b>Bandung</b> hari ini.</a></div>"##;
        let parsed = parse_ddg_html_results(ddg, 5);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "BMKG Cuaca");
        assert_eq!(parsed[0].1, "https://bmkg.go.id/cuaca");
        assert_eq!(parsed[0].2, "Prakiraan Bandung hari ini.");
    }

    #[test]
    fn currency_normalization_and_wmo_weather_codes() {
        assert_eq!(normalize_currency_code("dolar"), "USD");
        assert_eq!(normalize_currency_code("Rupiah"), "IDR");
        assert_eq!(normalize_currency_code("yen"), "JPY");
        assert_eq!(normalize_currency_code("sgd"), "SGD");
        assert_eq!(wmo_weather_desc_id(0), "Cerah");
        assert_eq!(wmo_weather_desc_id(63), "Hujan sedang");
        assert_eq!(wmo_weather_desc_id(95), "Badai petir");
    }

    #[test]
    fn web_ttl_cache_expires_and_bounds_entries() {
        let mut cache = WebTtlCache::new();
        let t0 = Instant::now();
        cache.put("k1".to_string(), "v1".to_string(), t0);
        assert_eq!(cache.get("k1", t0 + Duration::from_secs(10)), Some("v1".to_string()));
        assert_eq!(cache.get("k1", t0 + Duration::from_secs(CACHE_TTL_SECS + 1)), None);
    }
}
