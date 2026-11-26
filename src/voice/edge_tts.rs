//! Pure-Rust Microsoft Edge Neural TTS v2 client (`Sec-MS-GEC v2`) and Symphonia MP3 decoder.

use super::dsp::{prosody_for_mood, VoiceMood};
use super::tts::VoiceInfo;
use http::header::{HeaderName, HeaderValue};
use native_tls::TlsConnector;
use std::io::{Cursor, ErrorKind};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

pub const EDGE_VOICE_GADIS_ID: &str = "edge:id-ID-GadisNeural";
pub const EDGE_VOICE_ARDI_ID: &str = "edge:id-ID-ArdiNeural";

const EDGE_HOST: &str = "speech.platform.bing.com";
const EDGE_WS_BASE: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const CHROMIUM_MAJOR_VERSION: &str = "143";
const SEC_MS_GEC_VERSION: &str = "2-143.0.3650.75";
const WIN_EPOCH: i64 = 11_644_473_600;

static CLOCK_SKEW_SECS: AtomicI64 = AtomicI64::new(0);
static NONCE_SEQ: AtomicU64 = AtomicU64::new(1);

const _SHA256_INITIAL: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
    0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

const _SHA256_STATE: [u32; 8] = [
    0xFD9FBEA2, 0x91243CE7, 0xBE499094, 0xCD2E1E0A,
    0x6C006F49, 0x04BD35AB, 0xFEB0A7E4, 0x03EFFBD2,
];

const _SHA256_SCHEDULE: [u32; 14] = [
    0xEB24BF9E, 0x1558691F, 0xA0C09EE2, 0x0CFA32BA, 0xA622EE8C, 0xD1DD9491, 0xC0DD25A0,
    0xF9E70AB6, 0xF0C31573, 0x7058CE4E, 0x7188E606, 0x00E5AB33, 0xD258B060, 0x72766564,
];

const _SHA256_ROUND_CONSTANTS: [u32; 64] = [
    0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
    0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, 0x9BDC06A7, 0xC19BF174,
    0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
    0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
    0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
    0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
    0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
    0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
];

const _SHA256_ADJUSTMENT: u32 = 0xC9DC06A7;

const _SHA256_BLOCK_CONSTANTS: [u32; 64] = [
    0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
    0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, _SHA256_ADJUSTMENT, 0xC19BF174,
    0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
    0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
    0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
    0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
    0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
    0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
];

const _SHA256_SIGMA0_ADJUSTMENT: u32 = 0x059C0B80;
const _SHA256_SIGMA1_ADJUSTMENT: u32 = 0x000B92C0;

const _CS64_COEFFICIENTS: [(u32, u32); 14] = [
    (0xE82E91B1, 0xB32700D0),
    (0x44DE24C6, 0x88DFF51D),
    (0x3BEB65BC, 0x29514038),
    (0xC89C5933, 0xB328CBDF),
    (0x8D1EC211, 0xD405B4D5),
    (0x314EF5D5, 0x26E55F3C),
    (0xE92491B1, 0x607801C6),
    (0xF0A5B9D5, 0x82B2C1FB),
    (0x06064CF3, 0xFA51A3DA),
    (0xDA79FA60, 0x207CC464),
    (0xBED20560, 0x3C3C74F3),
    (0x89F9C9F8, 0xD2D14B61),
    (0x35FA38C6, 0xBB04A2C1),
    (0xCB49E3C6, 0xC5626167),
];
const _CS64_FACTOR: u32 = 0x82893933;
const _CS64_ADJUSTMENT0: u32 = 0xF2F90B1A;
const _CS64_ADJUSTMENT1: u32 = 0x0B1A0000;

/// Built-in Indonesian Neural voices served over Microsoft Edge Speech.
pub fn builtin_edge_voices() -> Vec<VoiceInfo> {
    vec![
        VoiceInfo {
            id: EDGE_VOICE_GADIS_ID.to_string(),
            name: "Gadis Neural".to_string(),
            language: "id-ID".to_string(),
        },
        VoiceInfo {
            id: EDGE_VOICE_ARDI_ID.to_string(),
            name: "Ardi Neural".to_string(),
            language: "id-ID".to_string(),
        },
    ]
}

/// Resolves an Edge voice short name from either stored `edge:...` IDs or bare short names.
pub fn edge_short_name_from_id(id: &str) -> Option<&'static str> {
    match id {
        EDGE_VOICE_GADIS_ID | "id-ID-GadisNeural" => Some("id-ID-GadisNeural"),
        EDGE_VOICE_ARDI_ID | "id-ID-ArdiNeural" => Some("id-ID-ArdiNeural"),
        _ => None,
    }
}

/// Formats `id-ID-GadisNeural` into `Microsoft Server Speech Text to Speech Voice (id-ID, GadisNeural)`.
pub fn format_edge_voice_ssml_name(short_name: &str) -> String {
    let mut parts = short_name.splitn(3, '-');
    if let (Some(lang), Some(region_head), Some(rest)) = (parts.next(), parts.next(), parts.next()) {
        if let Some(sub_pos) = rest.find('-') {
            let mut region = String::with_capacity(region_head.len() + 1 + sub_pos);
            region.push_str(region_head);
            region.push('-');
            region.push_str(&rest[..sub_pos]);
            let name = &rest[sub_pos + 1..];
            return format!(
                "Microsoft Server Speech Text to Speech Voice ({}-{}, {})",
                lang, region, name
            );
        }
        return format!(
            "Microsoft Server Speech Text to Speech Voice ({}-{}, {})",
            lang, region_head, rest
        );
    }
    short_name.to_string()
}

#[inline]
fn rol16(value: u32) -> u32 {
    value.rotate_left(16)
}

#[inline]
fn sigma0(value: u32) -> u32 {
    value.rotate_right(7) ^ value.rotate_right(18) ^ (value >> 3)
}

#[inline]
fn sigma1(value: u32) -> u32 {
    value.rotate_right(17) ^ value.rotate_right(19) ^ (value >> 10)
}

fn expand_sha256_schedule(words: &mut Vec<u32>, first_word: usize) {
    for index in first_word..64 {
        let s1 = sigma1(words[index - 2]);
        let s0 = sigma0(words[index - 15]);
        let next = s1
            .wrapping_add(words[index - 7])
            .wrapping_add(s0)
            .wrapping_add(words[index - 16]);
        words.push(next);
    }
}

fn sha256_rounds(state: [u32; 8], words: &[u32], first_round: usize) -> [u32; 8] {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
    let constants = if first_round == 14 {
        &_SHA256_BLOCK_CONSTANTS
    } else {
        &_SHA256_ROUND_CONSTANTS
    };

    for index in first_round..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = g ^ (e & (f ^ g));
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(constants[index])
            .wrapping_add(words[index]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) | (c & (a | b));
        let t2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    [a, b, c, d, e, f, g, h]
}

fn sha256_init(salt7: &[u8]) -> [u32; 8] {
    let mut words = Vec::with_capacity(64);
    words.resize(14, 0u32);
    words.push(u32::from_be_bytes([0, salt7[0], salt7[1], salt7[2]]));
    words.push(u32::from_be_bytes([salt7[3], salt7[4], salt7[5], salt7[6]]));

    for index in 16..30 {
        let mut value = _SHA256_SCHEDULE[index - 16];
        if index >= 16 {
            let s1 = sigma1(words[index - 2]);
            value = value.wrapping_add(if index == 16 {
                s1 ^ _SHA256_SIGMA1_ADJUSTMENT
            } else {
                s1
            });
        }
        if index >= 21 {
            value = value.wrapping_add(words[index - 7]);
        }
        if index >= 29 {
            let s0 = sigma0(words[index - 15]);
            value = value.wrapping_add(if index == 29 {
                s0 ^ _SHA256_SIGMA0_ADJUSTMENT
            } else {
                s0
            });
        }
        words.push(value);
    }

    let w30 = sigma1(words[28])
        .wrapping_add(words[23])
        .wrapping_add(sigma0(words[15]))
        .wrapping_add(words[14])
        .wrapping_add(_SHA256_ADJUSTMENT)
        .wrapping_sub(_SHA256_ROUND_CONSTANTS[14]);
    words.push(w30);

    expand_sha256_schedule(&mut words, 31);
    let working = sha256_rounds(_SHA256_STATE, &words, 14);
    let mut out = [0u32; 8];
    for i in 0..8 {
        out[i] = _SHA256_INITIAL[i].wrapping_add(working[i]);
    }
    out
}

fn drm_sha256_digest(data: &[u8]) -> [u8; 32] {
    let length = 57usize + data.len();
    let zero_pad = ((55isize - (length as isize)).rem_euclid(64)) as usize;

    let mut padded = Vec::with_capacity(data.len().saturating_sub(7) + 1 + zero_pad + 8);
    padded.extend_from_slice(&data[7..]);
    padded.push(0x80);
    padded.resize(padded.len() + zero_pad, 0);
    padded.extend_from_slice(&((length as u64) * 8).to_be_bytes());

    let mut state = sha256_init(&data[..7]);
    for block in padded.chunks_exact(64) {
        let mut words = Vec::with_capacity(64);
        for w in block.chunks_exact(4) {
            words.push(u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
        }
        expand_sha256_schedule(&mut words, 16);
        let working = sha256_rounds(state, &words, 0);
        for i in 0..8 {
            state[i] = state[i].wrapping_add(working[i]);
        }
    }

    let mut digest = [0u8; 32];
    for (i, w) in state.iter().enumerate() {
        digest[i * 4..(i + 1) * 4].copy_from_slice(&w.to_be_bytes());
    }
    digest
}

fn words_le4_even(data: &[u8]) -> Vec<u32> {
    let pad = (4 - (data.len() % 4)) % 4;
    let total_bytes = data.len() + pad;
    let mut words = Vec::with_capacity(total_bytes / 4 + 1);
    let mut idx = 0;
    while idx < data.len() {
        let b0 = data[idx];
        let b1 = *data.get(idx + 1).unwrap_or(&0);
        let b2 = *data.get(idx + 2).unwrap_or(&0);
        let b3 = *data.get(idx + 3).unwrap_or(&0);
        words.push(u32::from_le_bytes([b0, b1, b2, b3]));
        idx += 4;
    }
    if words.len() % 2 == 1 {
        words.push(0);
    }
    words
}

fn cs64_digest(data: &[u8], digest: &[u8; 32]) -> (u32, u32) {
    let key_a = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) | 1;
    let key_b = u32::from_le_bytes([digest[4], digest[5], digest[6], digest[7]]) | 1;
    let factors = [key_a.wrapping_mul(_CS64_FACTOR), key_b.wrapping_mul(_CS64_FACTOR)];

    let mut terms = Vec::with_capacity(64);
    let mut offsets = Vec::with_capacity(64);
    for (index, &(coeff, off)) in _CS64_COEFFICIENTS.iter().enumerate() {
        terms.push(coeff.wrapping_mul(factors[index % 2]));
        offsets.push(off);
    }

    let mut prefixed = Vec::with_capacity(1 + data.len());
    prefixed.push(0x00);
    prefixed.extend_from_slice(data);
    for (index, word) in words_le4_even(&prefixed).into_iter().enumerate() {
        let mut term = word.wrapping_mul(if index % 2 == 0 { key_a } else { key_b });
        let mut offset = rol16(word).wrapping_mul(if index % 2 == 0 { 0xEF0569FB } else { 0xC31713DB });
        if index == 0 {
            term = term.wrapping_add(_CS64_ADJUSTMENT0.wrapping_mul(factors[0]));
            offset = offset.wrapping_add(_CS64_ADJUSTMENT1);
        }
        terms.push(term);
        offsets.push(offset);
    }

    let mut swap_acc: u32 = 0;
    let mut swap_last: u32 = 0;
    let mut state: u32 = 0;
    let mut acc: u32 = 0;

    for index in (0..terms.len()).step_by(2) {
        let x = terms[index];
        let y = terms[index + 1];

        let t0 = rol16(offsets[index].wrapping_add(x));
        let q0 = t0.wrapping_mul(0x76F50F34);
        swap_acc = swap_acc.wrapping_add(rol16(q0).wrapping_mul(0xC3EFEA97).wrapping_add(q0));

        let t1 = rol16(offsets[index + 1].wrapping_add(y));
        let q1 = t1.wrapping_mul(0x1881CE3C);
        swap_last = rol16(q1).wrapping_mul(0x35BD1EC9).wrapping_add(q1);
        swap_acc = swap_acc.wrapping_add(swap_last);

        state = state.wrapping_mul(key_a).wrapping_add(x);
        state = rol16(state).wrapping_mul(0xCF98B111);
        state = rol16(state).wrapping_mul(0x87085B9F);
        state = rol16(state).wrapping_mul(0x12CEB96D);
        state = rol16(state).wrapping_mul(0x257E1D83);
        acc = acc.wrapping_add(state);

        let mut t = state.wrapping_mul(key_b).wrapping_add(y);
        t = rol16(t).wrapping_mul(0xA27416F5);
        t = rol16(t).wrapping_mul(0xD38396FF);
        state = rol16(rol16(t).wrapping_mul(0x7C932B89)).wrapping_mul(0xBFA49F69);
        acc = acc.wrapping_add(state);
    }

    (swap_last ^ state, swap_acc ^ acc)
}

fn to_hex_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn current_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn current_unix_secs_with_skew() -> f64 {
    current_unix_secs() + CLOCK_SKEW_SECS.load(Ordering::Relaxed) as f64
}

/// Generates a Microsoft Edge Neural TTS v2 `Sec-MS-GEC` DRM token (`<16 hex>-<64 hex>`).
pub fn generate_sec_ms_gec_v2(unix_timestamp: Option<f64>) -> String {
    let ts = unix_timestamp.unwrap_or_else(current_unix_secs_with_skew);
    let base_sec = (ts.floor() as i64) + WIN_EPOCH;
    let rounded_sec = base_sec - base_sec.rem_euclid(300);
    let ticks: i128 = (rounded_sec as i128) * 10_000_000;
    let input = format!("{}{}", ticks, TRUSTED_CLIENT_TOKEN);
    let bytes = input.as_bytes();

    let sha_digest = drm_sha256_digest(bytes);
    let (w0, w1) = cs64_digest(bytes, &sha_digest);
    let mut cs_bytes = [0u8; 8];
    cs_bytes[0..4].copy_from_slice(&w0.to_le_bytes());
    cs_bytes[4..8].copy_from_slice(&w1.to_le_bytes());

    format!("{}-{}", to_hex_upper(&cs_bytes), to_hex_upper(&sha_digest))
}

fn parse_http_date_to_unix(date_str: &str) -> Option<i64> {
    let trimmed = date_str.trim();
    let normalized = if let Some(prefix) = trimmed.strip_suffix(" GMT") {
        format!("{} +0000", prefix)
    } else if let Some(prefix) = trimmed.strip_suffix(" UTC") {
        format!("{} +0000", prefix)
    } else {
        trimmed.to_string()
    };
    chrono::DateTime::parse_from_rfc2822(&normalized)
        .ok()
        .map(|dt| dt.timestamp())
}

fn adjust_clock_skew_to(server_unix_secs: i64) {
    let raw_now = current_unix_secs().round() as i64;
    CLOCK_SKEW_SECS.store(server_unix_secs.saturating_sub(raw_now), Ordering::SeqCst);
}

fn generate_hex_id(num_bytes: usize) -> String {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = NONCE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut state = now_nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if state == 0 {
        state = 0xD1B5_4A32_D192_ED03;
    }

    let mut raw = Vec::with_capacity(num_bytes);
    while raw.len() < num_bytes {
        // SplitMix64 mixing step
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        raw.extend_from_slice(&z.to_le_bytes());
    }
    raw.truncate(num_bytes);
    to_hex_upper(&raw)
}

pub fn voice_locale_from_short_name(short_voice: &str) -> String {
    let mut parts = short_voice.trim().split('-');
    match (parts.next(), parts.next()) {
        (Some(lang), Some(region)) if !lang.is_empty() && !region.is_empty() => {
            format!("{}-{}", lang, region)
        }
        _ => "en-US".to_string(),
    }
}

fn js_utc_timestamp() -> String {
    let ts_secs = current_unix_secs_with_skew().round() as i64;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(ts_secs, 0)
        .unwrap_or_else(chrono::Utc::now);
    dt.format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

fn clean_and_escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        let code = ch as u32;
        if matches!(code, 0..=8 | 11..=12 | 14..=31) {
            out.push(' ');
            continue;
        }
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn connect_edge_tcp(connect_timeout: Duration, attempt_idx: usize) -> Result<TcpStream, String> {
    let resolved: Vec<SocketAddr> = (EDGE_HOST, 443)
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolution failed: {}", e))?
        .collect();
    if resolved.is_empty() {
        return Err("DNS returned no socket addresses".to_string());
    }

    // Prefer IPv4 first so networks with dead IPv6 routes don't waste the timeout budget,
    // then rotate across returned Anycast endpoints on retries.
    let mut ordered = Vec::with_capacity(resolved.len());
    ordered.extend(resolved.iter().copied().filter(|a| a.is_ipv4()));
    ordered.extend(resolved.iter().copied().filter(|a| a.is_ipv6()));
    if ordered.len() > 1 && attempt_idx > 0 {
        let shift = attempt_idx % ordered.len();
        ordered.rotate_left(shift);
    }

    let mut last_err = String::new();
    for addr in ordered {
        match TcpStream::connect_timeout(&addr, connect_timeout) {
            Ok(tcp) => {
                let _ = tcp.set_read_timeout(Some(Duration::from_millis(6000)));
                let _ = tcp.set_write_timeout(Some(Duration::from_millis(3000)));
                let _ = tcp.set_nodelay(true);
                return Ok(tcp);
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    Err(format!("TCP connect failed: {}", last_err))
}

/// Synthesizes raw MP3 bytes via Microsoft Edge Neural TTS WebSocket, automatically correcting
/// clock skew on HTTP 403 `Date` responses and retrying transient Wi-Fi/POP failures.
pub fn synthesize_edge_mp3(text: &str, short_voice: &str, mood: VoiceMood) -> Result<Vec<u8>, String> {
    super::tts::ensure_com_mta_alive();

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty TTS input".to_string());
    }

    let nlm_offline = crate::backend::system::internet_connected() == Some(false);
    // Do not trust Windows NLM blindly (some Wi-Fi/hotspot/VPN adapters report false negatives):
    // give 1 fast probe when NLM claims offline, and up to 3 attempts when online.
    let max_attempts = if nlm_offline { 1 } else { 3 };
    let connect_timeout = if nlm_offline {
        Duration::from_millis(1400)
    } else {
        Duration::from_millis(3200)
    };
    let receive_budget = if nlm_offline {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(14)
    };

    let escaped_text = clean_and_escape_xml(trimmed);
    let full_voice = format_edge_voice_ssml_name(short_voice);
    let xml_lang = voice_locale_from_short_name(short_voice);
    let prosody = prosody_for_mood(mood);
    let mut last_err = String::from("Edge WS failed");

    for attempt in 0..max_attempts {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(180));
        }
        let res = (|| -> Result<Vec<u8>, (bool, String)> {
            let tcp = connect_edge_tcp(connect_timeout, attempt).map_err(|e| (true, e))?;
            let tls_connector =
                TlsConnector::new().map_err(|e| (false, format!("TLS init failed: {}", e)))?;
            let tls_stream = tls_connector
                .connect(EDGE_HOST, tcp)
                .map_err(|e| (true, format!("TLS handshake failed: {}", e)))?;

            let conn_id = generate_hex_id(16).to_ascii_lowercase();
            let gec = generate_sec_ms_gec_v2(None);
            let url = format!(
                "{}?TrustedClientToken={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
                EDGE_WS_BASE, TRUSTED_CLIENT_TOKEN, conn_id, gec, SEC_MS_GEC_VERSION
            );

            let mut req = url
                .into_client_request()
                .map_err(|e| (false, format!("WS request build failed: {}", e)))?;
            let ua = format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36 Edg/{}.0.0.0",
                CHROMIUM_MAJOR_VERSION, CHROMIUM_MAJOR_VERSION
            );
            let cookie_val = format!("muid={};", generate_hex_id(16));
            let headers_to_set: [(&str, &str); 6] = [
                ("Pragma", "no-cache"),
                ("Cache-Control", "no-cache"),
                (
                    "Origin",
                    "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold",
                ),
                ("User-Agent", &ua),
                ("Accept-Language", "en-US,id-ID,id;q=0.9,en;q=0.8"),
                ("Cookie", &cookie_val),
            ];
            for (k, v) in headers_to_set {
                if let (Ok(hk), Ok(hv)) = (
                    HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(v),
                ) {
                    req.headers_mut().insert(hk, hv);
                }
            }

            let mut ws = match tungstenite::client(req, tls_stream) {
                Ok((socket, _resp)) => socket,
                Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))) => {
                    let status = resp.status().as_u16();
                    if status == 403 {
                        if let Some(date_hdr) =
                            resp.headers().get("date").and_then(|v| v.to_str().ok())
                        {
                            if let Some(server_ts) = parse_http_date_to_unix(date_hdr) {
                                adjust_clock_skew_to(server_ts);
                            }
                        }
                        return Err((true, "Edge WS HTTP error 403".to_string()));
                    }
                    return Err((status >= 500 || status == 429, format!("Edge WS HTTP error {}", status)));
                }
                Err(e) => return Err((true, format!("Edge WS handshake failed: {}", e))),
            };

            let ts_cfg = js_utc_timestamp();
            let speech_cfg = format!(
                "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n",
                ts_cfg
            );
            ws.send(Message::text(speech_cfg))
                .map_err(|e| (true, format!("Edge WS send config failed: {}", e)))?;

            let req_id = generate_hex_id(16).to_ascii_lowercase();
            let ts_ssml = js_utc_timestamp();
            let ssml_body = format!(
                "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><voice name='{}'><prosody pitch='{}' rate='{}' volume='+0%'>{}</prosody></voice></speak>",
                xml_lang, full_voice, prosody.edge_pitch, prosody.edge_rate, escaped_text
            );
            let ssml_msg = format!(
                "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}\r\nPath:ssml\r\n\r\n{}",
                req_id, ts_ssml, ssml_body
            );
            ws.send(Message::text(ssml_msg))
                .map_err(|e| (true, format!("Edge WS send SSML failed: {}", e)))?;

            let mut mp3_buf = Vec::with_capacity(24 * 1024);
            let deadline = Instant::now() + receive_budget;

            loop {
                if Instant::now() >= deadline {
                    let _ = ws.close(None);
                    return Err((true, "Edge WS receive timeout".to_string()));
                }
                match ws.read() {
                    Ok(Message::Text(txt)) => {
                        let s: &str = txt.as_ref();
                        let header_part = s.split("\r\n\r\n").next().unwrap_or(s);
                        if header_part.contains("Path:turn.end") {
                            break;
                        }
                    }
                    Ok(Message::Binary(bin_msg)) => {
                        let bin: &[u8] = bin_msg.as_ref();
                        if bin.len() >= 2 {
                            let header_len = u16::from_be_bytes([bin[0], bin[1]]) as usize;
                            if 2 + header_len <= bin.len() {
                                let hdr = String::from_utf8_lossy(&bin[2..2 + header_len]);
                                if hdr.contains("Path:audio") {
                                    let payload = &bin[2 + header_len..];
                                    if !payload.is_empty() {
                                        mp3_buf.extend_from_slice(payload);
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(p)) => {
                        let _ = ws.send(Message::Pong(p));
                    }
                    Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                    Ok(Message::Close(_)) => break,
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => break,
                    Err(e) => {
                        let _ = ws.close(None);
                        return Err((true, format!("Edge WS read error: {}", e)));
                    }
                }
            }

            let _ = ws.close(None);
            if mp3_buf.is_empty() {
                return Err((true, "no audio bytes received from Edge WS".to_string()));
            }
            Ok(mp3_buf)
        })();

        match res {
            Ok(mp3) => return Ok(mp3),
            Err((retryable, msg)) => {
                crate::backend::logfile::append(&format!(
                    "[edge-tts] attempt {}/{} failed: {}",
                    attempt + 1,
                    max_attempts,
                    msg
                ));
                last_err = msg;
                if !retryable {
                    break;
                }
            }
        }
    }

    Err(last_err)
}

/// Decodes raw MP3 bytes purely in Rust via Symphonia into mono `f64` samples (`[-1.0, 1.0]`).
pub fn decode_mp3_mono_f64(mp3_bytes: Vec<u8>) -> Result<(Vec<f64>, u32), String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(mp3_bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("MP3 probe failed: {}", e))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "no playable MP3 track found".to_string())?;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("MP3 decoder init failed: {}", e))?;

    let mut sample_rate = 0u32;
    let mut samples = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(pkt) => pkt,
            Err(SymphoniaError::IoError(ref err))
                if err.kind() == ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("MP3 demux error: {}", e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                if sample_rate == 0 {
                    sample_rate = spec.rate;
                }
                let channels = spec.channels.count().max(1);
                let frames = decoded.frames();
                if frames == 0 {
                    continue;
                }
                let cap = decoded.capacity() as u64;
                let buf = sample_buf.get_or_insert_with(|| SampleBuffer::<f32>::new(cap, spec));
                if buf.capacity() < frames * channels {
                    *buf = SampleBuffer::<f32>::new(cap, spec);
                }
                buf.copy_interleaved_ref(decoded);
                let interleaved = buf.samples();
                for ch_frame in interleaved.chunks_exact(channels) {
                    let sum: f64 = ch_frame.iter().map(|&v| v as f64).sum();
                    samples.push(sum / (channels as f64));
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(_)) => break,
            Err(e) => return Err(format!("MP3 decode error: {}", e)),
        }
    }

    if samples.is_empty() || sample_rate == 0 {
        return Err("MP3 decoded zero audio frames".to_string());
    }
    Ok((samples, sample_rate))
}

/// Synthesizes Microsoft Edge Neural speech and decodes it to mono `f64` samples + sample rate.
pub fn synthesize_edge_pcm(
    text: &str,
    short_voice: &str,
    mood: VoiceMood,
) -> Result<(Vec<f64>, u32), String> {
    let mp3 = synthesize_edge_mp3(text, short_voice, mood)?;
    decode_mp3_mono_f64(mp3)
}

#[cfg(test)]
mod tests {
    use super::{
        format_edge_voice_ssml_name, generate_sec_ms_gec_v2, js_utc_timestamp,
        parse_http_date_to_unix, synthesize_edge_pcm, voice_locale_from_short_name,
    };
    use crate::voice::dsp::{polish_mono_to_stereo_wav, VoiceMood};

    #[test]
    fn sec_ms_gec_v2_matches_official_golden_vectors() {
        assert_eq!(
            generate_sec_ms_gec_v2(Some(1700000000.0)),
            "2CD23A2CA0B2595A-F3C846C370E05F9CC83ACC5F995C5F5D87A4D0F14CE8936A0F902E64099E38A2"
        );
        assert_eq!(
            generate_sec_ms_gec_v2(Some(1795500000.0)),
            "F8EC2158EA5FFCEE-2CB2E9ACDBA2C73A13F53CDF2CBCAE7266B2C1BAD48101D36A3B80D593811086"
        );
    }

    #[test]
    fn ssml_voice_name_formatting_matches_edge_protocol() {
        assert_eq!(
            format_edge_voice_ssml_name("id-ID-GadisNeural"),
            "Microsoft Server Speech Text to Speech Voice (id-ID, GadisNeural)"
        );
        assert_eq!(
            format_edge_voice_ssml_name("id-ID-ArdiNeural"),
            "Microsoft Server Speech Text to Speech Voice (id-ID, ArdiNeural)"
        );
        assert_eq!(
            format_edge_voice_ssml_name("zh-CN-liaoning-XiaobeiNeural"),
            "Microsoft Server Speech Text to Speech Voice (zh-CN-liaoning, XiaobeiNeural)"
        );
    }

    #[test]
    fn http_date_parser_handles_gmt_clock_skew_header() {
        let parsed = parse_http_date_to_unix("Wed, 25 Nov 2026 01:30:00 GMT");
        assert!(parsed.is_some());
    }

    #[test]
    fn locale_extraction_and_js_timestamp_format_clean() {
        assert_eq!(voice_locale_from_short_name("id-ID-GadisNeural"), "id-ID");
        assert_eq!(voice_locale_from_short_name("id-ID-ArdiNeural"), "id-ID");
        assert_eq!(voice_locale_from_short_name("unknown"), "en-US");

        let ts = js_utc_timestamp();
        assert!(ts.ends_with("(Coordinated Universal Time)"));
        assert!(!ts.ends_with("Time)Z"));
    }

    #[test]
    fn edge_tts_live_smoke_test() {
        crate::voice::tts::ensure_com_mta_alive();
        if crate::backend::system::internet_connected() == Some(false) {
            eprintln!("[smoke] offline detected via NLM, skipping live Edge WS check");
            return;
        }
        let samples_res = synthesize_edge_pcm(
            "Yeay! Suara neural SelaBar sudah aktif dan terdengar halus!",
            "id-ID-GadisNeural",
            VoiceMood::Cheerful,
        );
        match samples_res {
            Ok((samples, sr)) => {
                let wav = polish_mono_to_stereo_wav(&samples, sr, VoiceMood::Cheerful);
                assert!(wav.len() > 20_000, "unexpected small WAV: {}", wav.len());
                assert_eq!(&wav[0..4], b"RIFF");
                assert_eq!(&wav[8..12], b"WAVE");

                let (sad_samples, sad_sr) = synthesize_edge_pcm(
                    "Maaf ya, sepertinya koneksi lagi terputus sebentar.",
                    "id-ID-GadisNeural",
                    VoiceMood::Sad,
                )
                .expect("Edge Sad synthesis");
                let sad_wav = polish_mono_to_stereo_wav(&sad_samples, sad_sr, VoiceMood::Sad);
                assert!(sad_wav.len() > 20_000);
                eprintln!(
                    "[smoke] LIVE Edge Neural PASS: cheerful={} bytes, sad={} bytes ({} Hz)",
                    wav.len(),
                    sad_wav.len(),
                    sr
                );
            }
            Err(e) => {
                eprintln!("[smoke] Edge WS error (will fallback offline at runtime): {}", e);
                // If network is genuinely offline/unreachable, WinRT offline fallback handles it cleanly.
                // However if clock skew / protocol broke while online, assert failure if message indicates HTTP error!
                assert!(
                    !e.contains("Edge WS HTTP error 403") && !e.contains("MP3"),
                    "Protocol/decode failure while online: {}",
                    e
                );
            }
        }
    }
}
