use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PRIMARY_URL: &str = "https://open.er-api.com/v6/latest/USD";
const FALLBACK_URL: &str = "https://api.frankfurter.app/latest?from=USD&to=IDR";
const PRIMARY_NAME: &str = "open.er-api.com";
const FALLBACK_NAME: &str = "frankfurter.app";
const UA: &str = "SelaBar/0.2.0 (rates)";
const CACHE_TTL_MS: u64 = 10 * 60 * 1000;
const MIN_RATE: f64 = 1_000.0;
const MAX_RATE: f64 = 100_000.0;

type CacheEntry = Option<(u64, f64, String)>;

fn cache() -> &'static Mutex<CacheEntry> {
    static CELL: OnceLock<Mutex<CacheEntry>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch_primary(cl: &reqwest::Client) -> Result<(f64, Option<String>), String> {
    let resp = cl
        .get(PRIMARY_URL)
        .send()
        .await
        .map_err(|e| format!("{}: {}", PRIMARY_NAME, e))?;
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", PRIMARY_NAME, resp.status().as_u16()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("{}: {}", PRIMARY_NAME, e))?;
    if let Some(result) = v.get("result").and_then(|x| x.as_str()) {
        if result != "success" {
            return Err(format!("{}: result={}", PRIMARY_NAME, result));
        }
    }
    let rate = v
        .get("rates")
        .and_then(|r| r.get("IDR"))
        .and_then(|x| x.as_f64())
        .ok_or_else(|| format!("{}: field rates.IDR tidak ditemukan", PRIMARY_NAME))?;
    let updated = v
        .get("time_last_update_utc")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    Ok((rate, updated))
}

async fn fetch_fallback(cl: &reqwest::Client) -> Result<(f64, Option<String>), String> {
    let resp = cl
        .get(FALLBACK_URL)
        .send()
        .await
        .map_err(|e| format!("{}: {}", FALLBACK_NAME, e))?;
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", FALLBACK_NAME, resp.status().as_u16()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("{}: {}", FALLBACK_NAME, e))?;
    let rate = v
        .get("rates")
        .and_then(|r| r.get("IDR"))
        .and_then(|x| x.as_f64())
        .ok_or_else(|| format!("{}: field rates.IDR tidak ditemukan", FALLBACK_NAME))?;
    let updated = v.get("date").and_then(|x| x.as_str()).map(|s| s.to_string());
    Ok((rate, updated))
}

fn plausible(rate: f64) -> bool {
    rate.is_finite() && rate >= MIN_RATE && rate <= MAX_RATE
}

pub async fn get_usd_idr() -> Value {
    let now = now_ms();
    if let Ok(st) = cache().lock() {
        if let Some((fetched_at, rate, source)) = st.as_ref() {
            if now.saturating_sub(*fetched_at) < CACHE_TTL_MS && plausible(*rate) {
                return json!({
                    "ok": true,
                    "base": "USD",
                    "quote": "IDR",
                    "rate": rate,
                    "source": source,
                    "fetched_at_ms": fetched_at,
                    "cached": true,
                });
            }
        }
    }

    let cl = match client() {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "reason": e }),
    };

    let mut errors = Vec::new();
    let mut implausible = None;
    let mut fetched = None;

    match fetch_primary(&cl).await {
        Ok((rate, updated)) => {
            if plausible(rate) {
                fetched = Some((rate, PRIMARY_NAME.to_string(), updated));
            } else {
                implausible = Some(format!("{}: nilai tidak masuk akal ({:.2})", PRIMARY_NAME, rate));
            }
        }
        Err(e) => errors.push(e),
    }

    if fetched.is_none() && implausible.is_none() {
        match fetch_fallback(&cl).await {
            Ok((rate, updated)) => {
                if plausible(rate) {
                    fetched = Some((rate, FALLBACK_NAME.to_string(), updated));
                } else {
                    implausible = Some(format!("{}: nilai tidak masuk akal ({:.2})", FALLBACK_NAME, rate));
                }
            }
            Err(e) => errors.push(e),
        }
    }

    let (rate, source, updated) = match fetched {
        Some(v) => v,
        None => {
            let reason = match implausible {
                Some(msg) => msg,
                None if errors.is_empty() => "gagal mengambil kurs USD/IDR".to_string(),
                None => errors.join(" | "),
            };
            return json!({ "ok": false, "reason": reason });
        }
    };

    let fetched_at = now_ms();
    if let Ok(mut st) = cache().lock() {
        *st = Some((fetched_at, rate, source.clone()));
    }

    let mut out = json!({
        "ok": true,
        "base": "USD",
        "quote": "IDR",
        "rate": rate,
        "source": source,
        "fetched_at_ms": fetched_at,
        "cached": false,
    });
    if let (Some(o), Some(u)) = (out.as_object_mut(), updated) {
        o.insert("provider_updated_at".to_string(), json!(u));
    }
    out
}
