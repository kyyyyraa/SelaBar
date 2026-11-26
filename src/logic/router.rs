use crate::backend::store::StoreManager;
use crate::events::UiSender;
use crate::state::{now_unix_ms, ConvoEntry};
use chrono::{Datelike, Local};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const GEMINI_OPENAI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const FALLBACK_MODEL: &str = "gpt-4o-mini";
const RATE_LIMIT_COOLDOWN_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiProviderKind {
    Router,
    Gemini,
    OpenAi,
}

impl AiProviderKind {
    fn idx(self) -> usize {
        match self {
            Self::Router => 0,
            Self::Gemini => 1,
            Self::OpenAi => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiProviderEndpoint {
    pub kind: AiProviderKind,
    pub label: &'static str,
    pub chat_url: String,
    pub models_url: String,
    pub api_key: String,
    pub model: String,
    pub fallback_model: Option<String>,
}

static PROVIDER_COOLDOWNS: OnceLock<Mutex<[u64; 3]>> = OnceLock::new();

fn cooldown_store() -> &'static Mutex<[u64; 3]> {
    PROVIDER_COOLDOWNS.get_or_init(|| Mutex::new([0u64; 3]))
}

pub fn mark_provider_rate_limited(kind: AiProviderKind) {
    let until = (now_unix_ms() as u64).saturating_add(RATE_LIMIT_COOLDOWN_MS);
    if let Ok(mut guard) = cooldown_store().lock() {
        guard[kind.idx()] = until;
    }
}

pub fn clear_provider_cooldown(kind: AiProviderKind) {
    if let Ok(mut guard) = cooldown_store().lock() {
        guard[kind.idx()] = 0;
    }
}

pub fn is_provider_in_cooldown(kind: AiProviderKind) -> bool {
    let now = now_unix_ms() as u64;
    cooldown_store()
        .lock()
        .map(|g| now < g[kind.idx()])
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub tool: String,
    pub args: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ActionPlan {
    pub steps: Vec<PlanStep>,
    pub say: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub args: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct AgentTurnResult {
    pub content: String,
    pub tool_calls: Vec<AgentToolCall>,
    pub raw_assistant_msg: Option<Value>,
    pub used_native_tools: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AskResult {
    pub text: String,
    pub ms: u64,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterSurface {
    Voice,
    Conversation,
    General,
}

pub fn normalize_base_url(raw: &str) -> String {
    let mut u = raw.trim().trim_end_matches('/').to_string();
    if u.to_ascii_lowercase().ends_with("/v1") {
        let len = u.len();
        u.truncate(len - 3);
    }
    u.trim_end_matches('/').to_string()
}

/// Resolves `(chat_completions_url, models_url)` from any base URL format (bare host, `/v1`,
/// Gemini `/v1beta/openai`, Groq `/openai/v1`, OpenRouter `/api/v1`, or full `/chat/completions`).
pub fn build_endpoint_urls(raw_base: &str) -> Option<(String, String)> {
    let u = raw_base.trim().trim_end_matches('/');
    if u.is_empty() {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    if lower.ends_with("/chat/completions") {
        let prefix = u[..u.len() - "/chat/completions".len()].trim_end_matches('/');
        if prefix.is_empty() {
            return None;
        }
        return Some((u.to_string(), format!("{}/models", prefix)));
    }
    if [
        "/v1",
        "/openai",
        "/v1beta/openai",
        "/v1beta",
        "/v4",
        "/api/v1",
    ]
    .iter()
    .any(|suf| lower.ends_with(suf))
    {
        return Some((format!("{}/chat/completions", u), format!("{}/models", u)));
    }
    Some((
        format!("{}/v1/chat/completions", u),
        format!("{}/v1/models", u),
    ))
}

fn is_local_router_url(url: &str) -> bool {
    let l = url.trim().to_ascii_lowercase();
    l.starts_with("http://127.0.0.1:")
        || l.starts_with("http://127.0.0.1/")
        || l.starts_with("http://localhost:")
        || l.starts_with("http://localhost/")
}

pub fn is_allowed_router_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    is_local_router_url(u) || u.starts_with("https://")
}

/// Detects rate limits, quota exhaustion, or billing limits so SelaBar fails over automatically.
pub fn is_rate_limit_or_quota(status: u16, body: &str) -> bool {
    if status == 429 || status == 402 {
        return true;
    }
    let l = body.to_ascii_lowercase();
    l.contains("rate_limit")
        || l.contains("rate limit")
        || l.contains("resource_exhausted")
        || l.contains("insufficient_quota")
        || l.contains("quota exceeded")
        || l.contains("too many requests")
        || l.contains("limit exceeded")
}

pub fn mask_key(key: &str) -> String {
    let prefix: String = key.chars().take(4).collect();
    format!("{}…", prefix)
}

pub fn model_for(store: &StoreManager, surface: RouterSurface) -> String {
    let s = store.get_settings();
    let pick = |cand: &str| -> String {
        let t = cand.trim();
        if !t.is_empty() && !t.eq_ignore_ascii_case("voice") {
            t.to_string()
        } else {
            s.dn_router_model.clone()
        }
    };
    match surface {
        RouterSurface::Voice => pick(&s.dn_router_voice_model),
        RouterSurface::Conversation => pick(&s.dn_router_convo_model),
        RouterSurface::General => s.dn_router_model,
    }
}

pub fn order_providers_with_cooldowns(
    candidates: Vec<AiProviderEndpoint>,
    primary: &str,
    cooldown_flags: &[bool; 3],
) -> Vec<AiProviderEndpoint> {
    let order = match primary.trim().to_ascii_lowercase().as_str() {
        "gemini" => [
            AiProviderKind::Gemini,
            AiProviderKind::OpenAi,
            AiProviderKind::Router,
        ],
        "openai" => [
            AiProviderKind::OpenAi,
            AiProviderKind::Gemini,
            AiProviderKind::Router,
        ],
        _ => [
            AiProviderKind::Router,
            AiProviderKind::Gemini,
            AiProviderKind::OpenAi,
        ],
    };

    let mut ordered = Vec::with_capacity(candidates.len());
    for kind in order {
        for ep in &candidates {
            if ep.kind == kind && !cooldown_flags[ep.kind.idx()] {
                ordered.push(ep.clone());
            }
        }
    }
    for kind in order {
        for ep in &candidates {
            if ep.kind == kind && cooldown_flags[ep.kind.idx()] {
                ordered.push(ep.clone());
            }
        }
    }
    ordered
}

pub fn resolve_configured_providers(
    store: &StoreManager,
    ui: Option<&UiSender>,
    surface: RouterSurface,
) -> Vec<AiProviderEndpoint> {
    let s = store.get_settings();
    let mut candidates = Vec::new();

    // 1. Local / Custom Router
    let router_key = store.read_router_api_key(ui);
    if let Some((chat_url, models_url)) = build_endpoint_urls(&s.dn_router_base_url) {
        if is_allowed_router_url(&chat_url)
            && (is_local_router_url(&chat_url) || !router_key.trim().is_empty())
        {
            let chosen = model_for(store, surface).trim().to_string();
            let model = if !chosen.is_empty() {
                chosen
            } else if !s.dn_router_model.trim().is_empty() {
                s.dn_router_model.trim().to_string()
            } else {
                FALLBACK_MODEL.to_string()
            };
            let fb_model = if !s.dn_router_model.trim().is_empty()
                && !s.dn_router_model.trim().eq_ignore_ascii_case("voice")
                && s.dn_router_model.trim() != model
            {
                Some(s.dn_router_model.trim().to_string())
            } else if model != FALLBACK_MODEL && !s.dn_router_model.trim().is_empty() {
                Some(FALLBACK_MODEL.to_string())
            } else {
                None
            };
            candidates.push(AiProviderEndpoint {
                kind: AiProviderKind::Router,
                label: "Local Router",
                chat_url,
                models_url,
                api_key: router_key,
                model,
                fallback_model: fb_model,
            });
        }
    }

    // 2. Google Gemini (AI Studio)
    let gemini_key = store.read_gemini_api_key(ui);
    if !gemini_key.trim().is_empty() {
        if let Some((chat_url, models_url)) = build_endpoint_urls(GEMINI_OPENAI_BASE_URL) {
            let gem_model = if s.dn_gemini_model.trim().is_empty() {
                DEFAULT_GEMINI_MODEL.to_string()
            } else {
                s.dn_gemini_model.trim().to_string()
            };
            let fb_model = if gem_model != "gemini-2.0-flash" {
                Some("gemini-2.0-flash".to_string())
            } else {
                None
            };
            candidates.push(AiProviderEndpoint {
                kind: AiProviderKind::Gemini,
                label: "Gemini (AI Studio)",
                chat_url,
                models_url,
                api_key: gemini_key,
                model: gem_model,
                fallback_model: fb_model,
            });
        }
    }

    // 3. OpenAI-Compatible Endpoint
    let openai_key = store.read_openai_api_key(ui);
    if let Some((chat_url, models_url)) = build_endpoint_urls(&s.dn_openai_base_url) {
        if is_allowed_router_url(&chat_url)
            && (is_local_router_url(&chat_url) || !openai_key.trim().is_empty())
        {
            let oa_model = if s.dn_openai_model.trim().is_empty() {
                DEFAULT_OPENAI_MODEL.to_string()
            } else {
                s.dn_openai_model.trim().to_string()
            };
            candidates.push(AiProviderEndpoint {
                kind: AiProviderKind::OpenAi,
                label: "OpenAI-Compatible",
                chat_url,
                models_url,
                api_key: openai_key,
                model: oa_model,
                fallback_model: None,
            });
        }
    }

    let cooldown_flags = [
        is_provider_in_cooldown(AiProviderKind::Router),
        is_provider_in_cooldown(AiProviderKind::Gemini),
        is_provider_in_cooldown(AiProviderKind::OpenAi),
    ];
    order_providers_with_cooldowns(candidates, &s.dn_ai_primary_provider, &cooldown_flags)
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())
}

async fn send_with_hiccup_retry(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    api_key: &str,
    body: Option<&Value>,
    allow_retry: bool,
) -> Result<(u16, String), String> {
    if !is_allowed_router_url(url) {
        return Err("url tidak diizinkan (gunakan http://127.0.0.1, http://localhost, atau https://)".to_string());
    }
    let attempts = if allow_retry { 2 } else { 1 };
    for attempt in 0..attempts {
        let mut req = client.request(method.clone(), url);
        if !api_key.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
        }
        if let Some(b) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(b.to_string());
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                return Ok((status, text));
            }
            Err(e) => {
                if attempt + 1 < attempts {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    continue;
                }
                return Err(format!("tidak bisa menghubungi endpoint: {}", e));
            }
        }
    }
    Err("tidak bisa menghubungi endpoint".to_string())
}

async fn probe_all_providers(
    store: &StoreManager,
    ui: Option<&UiSender>,
) -> (bool, Vec<String>, String) {
    let providers = resolve_configured_providers(store, ui, RouterSurface::General);
    if providers.is_empty() {
        return (
            false,
            Vec::new(),
            "Belum ada AI provider terkonfigurasi. Isi Gemini API Key, OpenAI Endpoint, atau URL Router dulu.".to_string(),
        );
    }
    let cl = match http_client() {
        Ok(c) => c,
        Err(e) => return (false, Vec::new(), e),
    };

    let mut any_ok = false;
    let mut first_models = Vec::new();
    let mut parts = Vec::with_capacity(providers.len());

    for ep in providers {
        match send_with_hiccup_retry(
            &cl,
            reqwest::Method::GET,
            &ep.models_url,
            &ep.api_key,
            None,
            false,
        )
        .await
        {
            Ok((status, body)) if (200..300).contains(&status) => {
                let mut ids = Vec::new();
                if let Ok(json) = serde_json::from_str::<Value>(&body) {
                    if let Some(arr) = json.get("data").and_then(|x| x.as_array()) {
                        for item in arr {
                            if let Some(id) = item.get("id").and_then(|x| x.as_str()) {
                                if !id.is_empty() {
                                    ids.push(id.to_string());
                                }
                            }
                        }
                    } else if let Some(arr) = json.get("models").and_then(|x| x.as_array()) {
                        for item in arr {
                            if let Some(id) = item.get("id").or_else(|| item.get("name")).and_then(|x| x.as_str()) {
                                if !id.is_empty() {
                                    ids.push(id.to_string());
                                }
                            }
                        }
                    }
                }
                clear_provider_cooldown(ep.kind);
                any_ok = true;
                let count = ids.len();
                if first_models.is_empty() && !ids.is_empty() {
                    first_models = ids;
                }
                parts.push(format!("{}: OK ({} model)", ep.label, count));
            }
            Ok((status, body)) => {
                if is_rate_limit_or_quota(status, &body) {
                    mark_provider_rate_limited(ep.kind);
                    parts.push(format!("{}: limit (HTTP {})", ep.label, status));
                } else {
                    parts.push(format!("{}: HTTP {}", ep.label, status));
                }
            }
            Err(_) => {
                parts.push(format!("{}: gagal terhubung", ep.label));
            }
        }
    }

    (any_ok, first_models, parts.join(" • "))
}

pub async fn list_models(store: &StoreManager, ui: Option<&UiSender>) -> Result<Vec<String>, String> {
    let (ok, models, msg) = probe_all_providers(store, ui).await;
    if ok {
        Ok(models)
    } else {
        Err(msg)
    }
}

pub async fn test_connection(
    store: &StoreManager,
    ui: Option<&UiSender>,
) -> (bool, Vec<String>, String) {
    probe_all_providers(store, ui).await
}

pub fn extract_content(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if let Some(c) = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| {
                c.get("message")
                    .and_then(|m| m.get("content"))
                    .or_else(|| c.get("delta").and_then(|d| d.get("content")))
                    .or_else(|| c.get("text"))
            })
            .and_then(|x| x.as_str())
        {
            return c.to_string();
        }
        if let Some(c) = v.get("content").and_then(|x| x.as_str()) {
            return c.to_string();
        }
    }

    if trimmed.contains("data:") {
        let mut acc = String::new();
        for line in trimmed.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("data:") {
                let d = rest.trim();
                if d.is_empty() || d == "[DONE]" {
                    continue;
                }
                if let Ok(chunk) = serde_json::from_str::<Value>(d) {
                    if let Some(part) = chunk
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| {
                            c.get("delta")
                                .and_then(|x| x.get("content"))
                                .or_else(|| c.get("message").and_then(|x| x.get("content")))
                        })
                        .and_then(|x| x.as_str())
                    {
                        acc.push_str(part);
                    }
                }
            }
        }
        if !acc.is_empty() {
            return acc;
        }
    }
    String::new()
}

const NO_PROVIDER_MSG: &str = "Belum ada AI provider yang diisi. Buka Settings → General untuk mengisi Gemini AI Studio, OpenAI-Compatible, atau URL Router.";

async fn dispatch_with_fallback<F>(
    providers: &[AiProviderEndpoint],
    mut build_body: F,
    require_non_empty_content: bool,
) -> Result<(AiProviderEndpoint, String, String), String>
where
    F: FnMut(&str) -> Value,
{
    if providers.is_empty() {
        return Err(NO_PROVIDER_MSG.to_string());
    }
    let cl = http_client()?;
    let allow_retry = providers.len() == 1;
    let mut errors = Vec::new();

    for ep in providers {
        let mut used_model = ep.model.clone();
        let body = build_body(&used_model);
        let (mut status, mut resp_text) = match send_with_hiccup_retry(
            &cl,
            reqwest::Method::POST,
            &ep.chat_url,
            &ep.api_key,
            Some(&body),
            allow_retry,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[SelaBar][AI] {} koneksi gagal ({}), cek fallback", ep.label, e);
                errors.push(format!("{} ({})", ep.label, e));
                continue;
            }
        };

        if is_rate_limit_or_quota(status, &resp_text) {
            mark_provider_rate_limited(ep.kind);
            eprintln!(
                "[SelaBar][AI] {} terkena limit (HTTP {}), fallback ke provider berikutnya",
                ep.label, status
            );
            errors.push(format!("{} (HTTP {} limit)", ep.label, status));
            continue;
        }

        if status == 400 || status == 404 || status >= 500 {
            if let Some(ref fb) = ep.fallback_model {
                if fb != &used_model {
                    let fb_body = build_body(fb);
                    if let Ok((st2, rt2)) = send_with_hiccup_retry(
                        &cl,
                        reqwest::Method::POST,
                        &ep.chat_url,
                        &ep.api_key,
                        Some(&fb_body),
                        false,
                    )
                    .await
                    {
                        if is_rate_limit_or_quota(st2, &rt2) {
                            mark_provider_rate_limited(ep.kind);
                            errors.push(format!("{} (HTTP {} limit)", ep.label, st2));
                            continue;
                        }
                        if (200..300).contains(&st2) {
                            status = st2;
                            resp_text = rt2;
                            used_model = fb.clone();
                        }
                    }
                }
            }
        }

        if !(200..300).contains(&status) {
            errors.push(format!("{} (HTTP {})", ep.label, status));
            continue;
        }

        if require_non_empty_content && extract_content(&resp_text).trim().is_empty() {
            errors.push(format!("{} (respons kosong)", ep.label));
            continue;
        }

        clear_provider_cooldown(ep.kind);
        return Ok((ep.clone(), used_model, resp_text));
    }

    Err(format!("Semua provider AI gagal: {}", errors.join(" • ")))
}

pub async fn ask(
    store: &StoreManager,
    ui: Option<&UiSender>,
    text: &str,
    image_data_url: Option<&str>,
    surface: RouterSurface,
    no_remember: bool,
) -> Result<AskResult, String> {
    let t0 = Instant::now();
    let providers = resolve_configured_providers(store, ui, surface);
    if providers.is_empty() {
        return Err(NO_PROVIDER_MSG.to_string());
    }

    let sys_prompt = match surface {
        RouterSurface::Conversation => {
            "Kamu adalah asisten SelaBar di Windows dalam mode percakapan. Bantu pengguna menjawab, menganalisis gambar/tangkapan layar, dan menjelaskan dengan ramah, jelas, dan akurat dalam bahasa Indonesia."
        }
        RouterSurface::General => {
            "Kamu adalah asisten SelaBar di Windows. Jawab pertanyaan pengguna dengan jelas, akurat, ringkas, dan dalam bahasa Indonesia."
        }
        RouterSurface::Voice => {
            "Kamu adalah asisten suara SelaBar di Windows. Jawabanmu dibacakan langsung lewat suara TTS, jadi jawab sangat singkat dan padat (1-2 kalimat) dalam bahasa Indonesia. Jangan pernah menolak dengan alasan tidak punya izin atau akses ke laptop, karena aplikasi SelaBar yang menangani integrasi sistem."
        }
    };

    let mut messages = vec![json!({ "role": "system", "content": sys_prompt })];
    for entry in convo_recent(store, 6) {
        if entry.role == "user" && entry.text == text {
            continue;
        }
        messages.push(json!({ "role": entry.role, "content": entry.text }));
    }

    let user_msg = if let Some(img) = image_data_url {
        json!({
            "role": "user",
            "content": [
                { "type": "text", "text": text },
                { "type": "image_url", "image_url": { "url": img } }
            ]
        })
    } else {
        json!({ "role": "user", "content": text })
    };
    messages.push(user_msg);

    let (_ep, used_model, resp_text) = dispatch_with_fallback(
        &providers,
        |mdl| {
            json!({
                "model": mdl,
                "messages": messages,
                "stream": false
            })
        },
        true,
    )
    .await?;

    let out = extract_content(&resp_text);
    if !no_remember {
        convo_remember(store, ui, "user", text);
        convo_remember(store, ui, "assistant", &out);
    }

    Ok(AskResult {
        text: out,
        ms: t0.elapsed().as_millis() as u64,
        model: used_model,
    })
}

fn build_action_prompt(tools: &[(&str, &str, &str)]) -> String {
    let now = Local::now();
    let now_str = now.format("%Y-%m-%d %H:%M (WIB)").to_string();
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|(name, args, desc)| json!({ "name": name, "args": args, "desc": desc }))
        .collect();

    format!(
        "Kamu adalah agen AI asisten Windows bernama SelaBar. Aplikasi SelaBar yang mengeksekusi rencana tool langsung di laptop pengguna (dengan kartu izin jika perlu).\n\
Jangan pernah menolak, jangan minta user mengetik perintah sendiri, dan jangan minta konfirmasi teks.\n\
Pahami maksud user dari kalimat alami (bukan kata kunci kaku). Contoh: \"coba cari file dengan kata cv ada apa saja\" = cari file bernama cv (query=\"cv\"), bukan menyalin seluruh kalimat. Untuk pertanyaan tentang sistem (dolar/kurs, install, kondisi laptop), pilih tool yang sesuai atau jawab langsung; jangan memperlakukan semua input sebagai perintah parser.\n\
Balas HANYA SATU objek JSON valid berbentuk {{\"steps\":[{{\"tool\":\"<nama tool>\",\"args\":{{...}}}}],\"say\":\"<konfirmasi pendek>\"}} (maksimal 6 langkah).\n\
Jika pertanyaan murni pengetahuan umum (misal \"siapa presiden indonesia\"), balas {{\"steps\":[],\"say\":\"<jawaban singkat 1-2 kalimat>\"}}.\n\
Aturan tool:\n\
- WAJIB hemat & senyap untuk pertanyaan informasi internet (cuaca, suhu, kurs mata uang, berita, harga, jadwal, definisi, fakta umum): SELALU gunakan `weather`, `currency_rate`/`rate_usd_idr`, `web_search`, atau `web_fetch` di latar belakang TANPA membuka browser. JANGAN pernah memanggil `open_tab` kecuali pengguna secara eksplisit meminta membuka situs/browser (misal \"buka browser\", \"buka wa web\", \"putar youtube\").\n\
- diagnose_system ({{}}) untuk cek pemakaian RAM, CPU, disk, atau proses apa yang membuat laptop lambat.\n\
- youtube_play ({{\"query\",\"channel\",\"newest\"}}) untuk mencari & langsung memutar video di YouTube (newest:true HANYA jika diminta paling baru/terbaru; newest:false untuk paling hits/populer/lagu). youtube untuk mencari daftar video.\n\
- open_tab (bukan open_app) HANYA jika user eksplisit meminta membuka situs web di layar (whatsapp/wa web, youtube, github, twitter, gmail, zoom, teams, meet).\n\
- calendar_add ({{\"title\",\"start\",\"end\",\"note\"}}) untuk menambah jadwal/agenda ke kalender (format start YYYY-MM-DDTHH:MM). title = nama acara SINGKAT dan bersih (mis. \"Kelas Sosiologi\"), JANGAN menyalin seluruh kalimat user. Jika ada link meeting (meet.google.com/zoom/teams), taruh URL LENGKAP di \"note\" supaya tombol Join muncul. \"end\" opsional.\n\
- Jika user mengirim URL meeting menyusul setelah membuat jadwal (mis. \"https://meet.google.com/... masukin linknya\"), itu LINK untuk jadwal yang BARU dibuat: panggil calendar_add dengan title & start SAMA persis seperti jadwal sebelumnya dan note = URL lengkap. Sistem akan meng-update jadwal itu, bukan membuat duplikat.\n\
- whatsapp ({{\"contact\",\"message\"}}) untuk mengirim pesan WhatsApp.\n\
- search_file ({{\"query\",\"hint\"}}) menaruh hasil file ke shelf notch.\n\
- delete_file ({{\"query\",\"hint\"}}) untuk menghapus ke Recycle Bin (jangan mengarang path di shell).\n\
- shell menggunakan PowerShell Windows yang valid (jangan perintah Linux seperti find/rm/gio).\n\
Contoh:\n\
1) \"buka chrome\" -> {{\"steps\":[{{\"tool\":\"open_app\",\"args\":{{\"app\":\"chrome\"}}}}],\"say\":\"Membuka Chrome.\"}}\n\
2) \"buka wa web\" -> {{\"steps\":[{{\"tool\":\"open_tab\",\"args\":{{\"site\":\"whatsapp\"}}}}],\"say\":\"Membuka WhatsApp Web.\"}}\n\
3) \"cari video windah paling hits dan play\" -> {{\"steps\":[{{\"tool\":\"youtube_play\",\"args\":{{\"query\":\"windah basudara\",\"channel\":\"windah basudara\",\"newest\":false}}}}],\"say\":\"Memutar video populer Windah Basudara.\"}}\n\
4) \"putar lagu santai di youtube\" -> {{\"steps\":[{{\"tool\":\"youtube_play\",\"args\":{{\"query\":\"lagu santai\",\"newest\":false}}}}],\"say\":\"Memutar musik santai di YouTube.\"}}\n\
5) \"cek ram / apa yang makan banyak ram\" -> {{\"steps\":[{{\"tool\":\"diagnose_system\",\"args\":{{}}}}],\"say\":\"Mengecek RAM & proses berjalan.\"}}\n\
6) \"buatkan jadwal rapat besok jam 15:00 linknya https://meet.google.com/abc-defg-hij\" -> {{\"steps\":[{{\"tool\":\"calendar_add\",\"args\":{{\"title\":\"Rapat\",\"start\":\"2026-11-19T15:00\",\"note\":\"https://meet.google.com/abc-defg-hij\"}}}}],\"say\":\"Menyiapkan jadwal rapat dengan link Join.\"}}\n\
7) \"install ffmpeg\" -> {{\"steps\":[{{\"tool\":\"shell\",\"args\":{{\"cmd\":\"winget install Gyan.FFmpeg\"}}}}],\"say\":\"Menyiapkan instalasi ffmpeg.\"}}\n\
8) \"kirim wa ke budi halo apa kabar\" -> {{\"steps\":[{{\"tool\":\"whatsapp\",\"args\":{{\"contact\":\"budi\",\"message\":\"halo apa kabar\"}}}}],\"say\":\"Menyiapkan pesan WhatsApp ke Budi.\"}}\n\
9) \"carikan file cv di download\" -> {{\"steps\":[{{\"tool\":\"search_file\",\"args\":{{\"query\":\"cv\",\"hint\":\"download\"}}}}],\"say\":\"Mencari CV di Downloads.\"}}\n\
10) \"cek isi folder C:\\\\Users\" -> {{\"steps\":[{{\"tool\":\"shell\",\"args\":{{\"cmd\":\"Get-ChildItem C:\\\\Users | Select-Object -First 15 Name\"}}}}],\"say\":\"Mengecek folder.\"}}\n\
11) \"tambah tugas beli kopi\" -> {{\"steps\":[{{\"tool\":\"todo_add\",\"args\":{{\"text\":\"beli kopi\"}}}}],\"say\":\"Ditambahkan ke to-do.\"}}\n\
12) \"siapa penemu lampu\" -> {{\"steps\":[],\"say\":\"Thomas Alva Edison dikenal sebagai penemu bola lampu pijar komersial.\"}}\n\
Waktu lokal saat ini: {}\n\
Daftar tool: {}",
        now_str,
        serde_json::to_string(&tools_json).unwrap_or_default()
    )
}

pub async fn ask_action(
    store: &StoreManager,
    ui: Option<&UiSender>,
    text: &str,
    tools: &[(&str, &str, &str)],
    surface: RouterSurface,
) -> ActionPlan {
    let providers = resolve_configured_providers(store, ui, surface);
    if providers.is_empty() {
        return ActionPlan {
            steps: Vec::new(),
            say: String::new(),
            error: Some(NO_PROVIDER_MSG.to_string()),
        };
    }

    let sys = build_action_prompt(tools);
    let mut messages = vec![json!({ "role": "system", "content": sys })];
    for entry in convo_recent(store, 6) {
        if entry.role == "user" && entry.text == text {
            continue;
        }
        messages.push(json!({ "role": entry.role, "content": entry.text }));
    }
    messages.push(json!({ "role": "user", "content": text }));

    match dispatch_with_fallback(
        &providers,
        |mdl| {
            json!({
                "model": mdl,
                "messages": messages,
                "stream": false,
                "response_format": { "type": "json_object" }
            })
        },
        true,
    )
    .await
    {
        Ok((_ep, _mdl, resp_text)) => {
            let content = extract_content(&resp_text);
            parse_plan(&content)
        }
        Err(e) => ActionPlan {
            steps: Vec::new(),
            say: String::new(),
            error: Some(e),
        },
    }
}

pub async fn summarize_tool_result(
    store: &StoreManager,
    ui: Option<&UiSender>,
    surface: RouterSurface,
    user_text: &str,
    tool_outputs: &str,
) -> Result<String, String> {
    let prompt = format!(
        "Permintaan pengguna:\n{}\n\nHasil nyata yang baru saja dijalankan SelaBar di laptop pengguna:\n{}\n\n{}",
        user_text,
        tool_outputs,
        if surface == RouterSurface::Voice {
            "Jawab sangat singkat dalam 1-2 kalimat bahasa Indonesia agar enak dibacakan TTS berdasarkan hasil di atas."
        } else {
            "Berikan jawaban final yang jelas, ringkas, langsung ke inti dalam bahasa Indonesia berdasarkan hasil eksekusi nyata di atas. Jangan suruh user menjalankan perintah lagi."
        }
    );
    let res = ask(store, ui, &prompt, None, surface, true).await?;
    Ok(res.text)
}

fn fn_schema(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": props,
                "required": required,
            }
        }
    })
}

pub fn build_openai_tools_schema() -> Value {
    Value::Array(vec![
        fn_schema(
            "shell",
            "Jalankan perintah PowerShell di laptop Windows pengguna untuk inspeksi sistem, baca file/folder, atau otomasi sistem (perintah ubah/hapus otomatis meminta izin user).",
            json!({ "cmd": { "type": "string", "description": "Satu perintah PowerShell Windows yang valid." } }),
            &["cmd"],
        ),
        fn_schema(
            "search_file",
            "Cari file secara rekursif di Desktop, Documents, Downloads, OneDrive, dan D:\\ (opsional filter hint). Gunakan add_to_shelf=true HANYA jika user meminta menaruh ke shelf.",
            json!({
                "query": { "type": "string", "description": "Kata kunci nama file, misal 'cv' atau 'laporan'." },
                "hint": { "type": "string", "enum": ["desktop", "documents", "downloads"] },
                "add_to_shelf": { "type": "boolean", "description": "Set true hanya jika user minta menaruh file ke shelf notch." }
            }),
            &["query"],
        ),
        fn_schema(
            "shelf_add",
            "Tambahkan satu path file lengkap yang sudah ada ke Shelf SelaBar.",
            json!({ "path": { "type": "string", "description": "Path file lengkap di Windows." } }),
            &["path"],
        ),
        fn_schema(
            "delete_file",
            "Hapus file (mencari nama file lalu pindahkan ke Recycle Bin; selalu meminta izin user).",
            json!({
                "query": { "type": "string" },
                "hint": { "type": "string", "enum": ["desktop", "documents", "downloads"] }
            }),
            &["query"],
        ),
        fn_schema(
            "diagnose_system",
            "Cek CPU, RAM, Disk C bebas, dan proses teratas yang memakai memori terbanyak di laptop.",
            json!({}),
            &[],
        ),
        fn_schema(
            "open_app",
            "Buka atau fokuskan aplikasi desktop terinstal di Windows (misal Discord, Spotify, VS Code, Explorer).",
            json!({ "app": { "type": "string" } }),
            &["app"],
        ),
        fn_schema(
            "open_tab",
            "Buka URL atau pakai ulang tab browser yang sudah ada (whatsapp, youtube, meet, gmail, github, atau URL penuh). HANYA gunakan jika pengguna secara eksplisit meminta membuka situs/browser di layar. JANGAN dipakai untuk mencari jawaban pertanyaan, cuaca, kurs, atau berita (gunakan web_search/web_fetch/weather/currency_rate).",
            json!({
                "site": { "type": "string" },
                "url": { "type": "string" },
                "browser": { "type": "string", "description": "Browser target opsional bila diminta pengguna (chrome, zen, edge, firefox)." }
            }),
            &["site"],
        ),
        fn_schema(
            "web_search",
            "Cari informasi terkini di internet secara diam-diam di latar belakang TANPA membuka browser (cuaca, berita, skor, harga, dokumentasi, info umum). Selalu prioritaskan ini dibanding open_tab untuk menjawab pertanyaan.",
            json!({
                "query": { "type": "string", "description": "Kata kunci pencarian yang ringkas dan jelas." },
                "limit": { "type": "integer", "description": "Jumlah hasil (1-8, default 5)." }
            }),
            &["query"],
        ),
        fn_schema(
            "web_fetch",
            "Ambil dan baca isi teks bersih dari URL publik http/https di latar belakang TANPA membuka browser (cocok untuk membaca halaman hasil web_search).",
            json!({
                "url": { "type": "string", "description": "URL publik diawali http:// atau https://." },
                "max_chars": { "type": "integer", "description": "Batas karakter output (default 2500)." }
            }),
            &["url"],
        ),
        fn_schema(
            "weather",
            "Cek cuaca terkini dan prakiraan hari ini/besok untuk suatu kota atau lokasi pengguna saat ini TANPA membuka browser.",
            json!({
                "location": { "type": "string", "description": "Nama kota/daerah opsional (misal 'Bandung', 'Jakarta'). Kosongkan untuk lokasi pengguna saat ini." }
            }),
            &[],
        ),
        fn_schema(
            "currency_rate",
            "Cek kurs dan konversi mata uang terkini secara langsung TANPA membuka browser (misal USD ke IDR, JPY ke IDR, SGD ke IDR).",
            json!({
                "from": { "type": "string", "description": "Kode/nama mata uang asal, default USD." },
                "to": { "type": "string", "description": "Kode/nama mata uang tujuan, default IDR." },
                "amount": { "type": "number", "description": "Jumlah konversi, default 1." }
            }),
            &[],
        ),
        fn_schema(
            "youtube",
            "Buka halaman hasil pencarian YouTube di browser (di tab YouTube yang sudah ada jika terbuka).",
            json!({
                "query": { "type": "string" },
                "browser": { "type": "string", "description": "Browser target opsional bila diminta pengguna (chrome, zen, edge, firefox)." }
            }),
            &["query"],
        ),
        fn_schema(
            "youtube_play",
            "Cari video YouTube dan langsung putar video teratas/paling cocok di browser (memakai ulang tab YouTube yang sudah ada).",
            json!({
                "query": { "type": "string" },
                "channel": { "type": "string" },
                "newest": { "type": "boolean" },
                "browser": { "type": "string", "description": "Browser target opsional bila diminta pengguna (chrome, zen, edge, firefox)." }
            }),
            &["query"],
        ),
        fn_schema(
            "spotify",
            "Cari atau putar lagu di Spotify.",
            json!({ "query": { "type": "string" } }),
            &["query"],
        ),
        fn_schema(
            "whatsapp",
            "Siapkan pengiriman pesan WhatsApp ke kontak atau nomor tertentu (meminta izin user).",
            json!({
                "contact": { "type": "string" },
                "message": { "type": "string" }
            }),
            &["contact", "message"],
        ),
        fn_schema(
            "calendar_list",
            "Lihat daftar jadwal/agenda di kalender SelaBar.",
            json!({ "date": { "type": "string", "description": "Opsional: YYYY-MM-DD, today, tomorrow, atau all." } }),
            &[],
        ),
        fn_schema(
            "calendar_add",
            "Tambah acara/rapat baru ke kalender SelaBar. Pisahkan judul bersih dari link rapat (taruh link di note).",
            json!({
                "title": { "type": "string", "description": "Judul acara yang singkat & bersih tanpa URL atau waktu." },
                "start": { "type": "string", "description": "Waktu mulai format lokal YYYY-MM-DDTHH:MM." },
                "note": { "type": "string", "description": "Catatan atau URL meeting (Meet/Zoom/Teams) jika ada." }
            }),
            &["title", "start"],
        ),
        fn_schema(
            "calendar_update",
            "Ubah judul, waktu, atau catatan/link pada jadwal kalender yang sudah ada.",
            json!({
                "id": { "type": "string" },
                "title": { "type": "string", "description": "Judul jadwal lama untuk dicocokkan jika id tidak diketahui." },
                "new_title": { "type": "string" },
                "start": { "type": "string", "description": "YYYY-MM-DDTHH:MM" },
                "note": { "type": "string" }
            }),
            &[],
        ),
        fn_schema(
            "calendar_delete",
            "Hapus jadwal dari kalender SelaBar (meminta izin user).",
            json!({
                "id": { "type": "string" },
                "title": { "type": "string" }
            }),
            &[],
        ),
        fn_schema(
            "todo_list",
            "Lihat daftar to-do/tugas di SelaBar.",
            json!({}),
            &[],
        ),
        fn_schema(
            "todo_add",
            "Tambah satu tugas baru ke daftar to-do SelaBar.",
            json!({ "text": { "type": "string" } }),
            &["text"],
        ),
        fn_schema(
            "todo_done",
            "Tandai tugas to-do selesai berdasarkan id atau potongan teksnya.",
            json!({ "id": { "type": "string" }, "query": { "type": "string" } }),
            &[],
        ),
        fn_schema(
            "reminder_add",
            "Pasang pengingat di SelaBar (pakai minutes untuk relatif, atau start ISO YYYY-MM-DDTHH:MM, atau at epoch ms).",
            json!({
                "text": { "type": "string" },
                "minutes": { "type": "integer" },
                "start": { "type": "string" },
                "at": { "type": "integer" }
            }),
            &["text"],
        ),
        fn_schema(
            "notifs_list",
            "Baca daftar notifikasi Windows terbaru yang masuk ke SelaBar.",
            json!({
                "unread_only": { "type": "boolean" },
                "limit": { "type": "integer" }
            }),
            &[],
        ),
        fn_schema(
            "rate_usd_idr",
            "Cek kurs USD ke IDR terkini.",
            json!({}),
            &[],
        ),
        fn_schema(
            "volume",
            "Atur volume audio Windows (level 0-100 atau delta +/-).",
            json!({
                "level": { "type": "integer" },
                "delta": { "type": "integer" }
            }),
            &[],
        ),
        fn_schema(
            "media",
            "Kontrol pemutar media aktif (play-pause, next, previous).",
            json!({ "action": { "type": "string", "enum": ["play-pause", "next", "previous"] } }),
            &["action"],
        ),
        fn_schema(
            "theme",
            "Ubah tema Windows ke dark atau light.",
            json!({ "mode": { "type": "string", "enum": ["dark", "light"] } }),
            &["mode"],
        ),
        fn_schema(
            "type_text",
            "Ketik atau tempel teks ke jendela aktif saat ini (meminta izin user).",
            json!({ "text": { "type": "string" } }),
            &["text"],
        ),
        fn_schema(
            "press_key",
            "Kirim shortcut keyboard seperti ctrl+s atau enter (meminta izin user).",
            json!({ "key": { "type": "string" } }),
            &["key"],
        ),
        fn_schema(
            "memory_save",
            "Simpan fakta jangka panjang tentang pengguna (nama, preferensi, jurusan, proyek, kebiasaan) agar diingat di sesi berikutnya.",
            json!({
                "fact": { "type": "string" },
                "category": { "type": "string", "description": "profil|preferensi|proyek|kontak|umum" }
            }),
            &["fact"],
        ),
        fn_schema(
            "memory_delete",
            "Hapus entri memori jangka panjang berdasarkan id atau kata kunci.",
            json!({ "id": { "type": "string" }, "query": { "type": "string" } }),
            &[],
        ),
        fn_schema(
            "memory_list",
            "Lihat semua fakta dalam memori jangka panjang SelaBar.",
            json!({}),
            &[],
        ),
        fn_schema(
            "browser_snapshot",
            "Baca elemen interaktif (@b1, @b2, ...) beserta teks halaman web dalam urutan visual atas-ke-bawah via UI Automation (menampilkan status [checked] & val=\"...\").",
            json!({
                "browser": { "type": "string", "description": "Browser target opsional (chrome, zen, edge, firefox)." },
                "query": { "type": "string", "description": "Filter opsional kata kunci nama tombol/link/kolom." },
                "include_text": { "type": "boolean", "description": "Set true untuk membaca soal, label pertanyaan, atau teks halaman di atas tiap opsi." }
            }),
            &[],
        ),
        fn_schema(
            "browser_act",
            "Jalankan aksi cepat di halaman web (skip iklan YouTube, klik @bN/teks, isi kolom, scroll mouse, navigasi, atau `batch` multi-soal sekaligus + `snapshot_after=true`).",
            json!({
                "action": { "type": "string", "enum": ["skip_ad", "click", "type", "key", "scroll", "navigate", "batch"] },
                "browser": { "type": "string", "description": "Browser target opsional (chrome, zen, edge, firefox)." },
                "target": { "type": "string", "description": "Ref @bN atau teks label elemen, down/up/top/bottom untuk scroll, atau URL untuk navigate." },
                "text": { "type": "string", "description": "Teks untuk action=type atau URL untuk action=navigate." },
                "key": { "type": "string", "description": "Nama tombol untuk action=key (l, j, k, f, m) atau 'enter' sesudah mengetik." },
                "clear": { "type": "boolean", "description": "Set true untuk Ctrl+A+Backspace sebelum mengetik teks baru." },
                "amount": { "type": "integer", "description": "Jumlah langkah scroll mouse (1-12, default 5) untuk action=scroll." },
                "snapshot_after": { "type": "boolean", "description": "Set true agar langsung mengembalikan browser_snapshot(include_text=true) terbaru setelah aksi/scroll selesai." },
                "actions": {
                    "type": "array",
                    "description": "Daftar hingga 15 langkah untuk action=batch (contoh: jawab semua soal @bN yang terlihat lalu scroll down di akhir).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["click", "type", "key", "scroll"] },
                            "target": { "type": "string" },
                            "text": { "type": "string" },
                            "key": { "type": "string" },
                            "clear": { "type": "boolean" },
                            "amount": { "type": "integer" }
                        },
                        "required": ["action"]
                    }
                }
            }),
            &["action"],
        ),
        fn_schema(
            "desktop_snapshot",
            "Inspeksi elemen UI interaktif (@d1, @d2, dst) di aplikasi desktop non-browser seperti Word, Excel, Notepad, atau Explorer.",
            json!({
                "app": { "type": "string", "description": "Nama aplikasi atau judul jendela target, mis. Word, Excel." },
                "query": { "type": "string", "description": "Filter opsional nama tombol/menu." }
            }),
            &[],
        ),
        fn_schema(
            "desktop_act",
            "Interaksi UI pada aplikasi desktop di luar browser (fokus jendela, klik tombol/menu, ketik teks, kirim shortcut).",
            json!({
                "action": { "type": "string", "enum": ["click", "type", "key", "focus"] },
                "app": { "type": "string", "description": "Nama aplikasi target, mis. Word atau Excel." },
                "target": { "type": "string", "description": "Ref @dN/@s... atau teks label elemen." },
                "text": { "type": "string", "description": "Teks untuk action=type." },
                "key": { "type": "string", "description": "Nama tombol/shortcut untuk action=key atau 'enter'." }
            }),
            &["action"],
        ),
    ])
}

pub fn build_agent_system_prompt(store: &StoreManager, surface: RouterSurface) -> String {
    let now = Local::now();
    let hari = match now.weekday() {
        chrono::Weekday::Mon => "Senin",
        chrono::Weekday::Tue => "Selasa",
        chrono::Weekday::Wed => "Rabu",
        chrono::Weekday::Thu => "Kamis",
        chrono::Weekday::Fri => "Jumat",
        chrono::Weekday::Sat => "Sabtu",
        chrono::Weekday::Sun => "Minggu",
    };
    let user_profile = std::env::var("USERPROFILE")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    let mem_block = crate::logic::memory::prompt_block(store);

    let style_rule = if surface == RouterSurface::Voice {
        "Mode Suara: wajib jawab 100% dalam bahasa Indonesia alami (JANGAN pernah mengeluarkan huruf Mandarin/CJK atau bahasa asing lain di luar istilah teknis/judul). Jawaban final dibacakan langsung lewat TTS, wajib sangat singkat, padat, dan to the point (1-2 kalimat bahasa Indonesia alami). DILARANG memakai emoji, simbol (✓, ×, •), markdown (**, `, #), URL lengkap, atau path file panjang."
    } else {
        "Wajib jawab 100% dalam bahasa Indonesia yang natural, ringkas, dan jelas (JANGAN pernah memakai huruf Mandarin/CJK)."
    };

    let mut prompt = format!(
        "Kamu adalah asisten agen desktop SelaBar yang berjalan langsung di Windows milik pengguna.\n\
Waktu lokal sekarang: {}, {} ({}).\n\
Folder user Windows: {}.\n\n\
Aturan perilaku agen:\n\
1. Kamu memiliki tool nyata untuk mengakses internet di latar belakang (web_search, web_fetch, weather, currency_rate), laptop, file, PowerShell, kalender, pengingat, tugas, notifikasi, browser, media, dan memori. JANGAN pernah menyuruh pengguna menjalankan perintah sendiri jika kamu bisa memanggil tool langsung.\n\
2. Pahami niat pengguna dulu baru pilih tool & argumen dengan rapi (bukan copy mentah kalimat user):\n\
   - PERTANYAAN INTERNET / CUACA / KURS / FAKTA TERKINI (PENTING): Jika pengguna bertanya cuaca, kurs/mata uang, berita, jadwal pertandingan, harga, pengetahuan terkini, atau informasi web apa pun, WAJIB gunakan `weather`, `currency_rate` / `rate_usd_idr`, `web_search`, dan `web_fetch` secara diam-diam di latar belakang. DILARANG membuka browser (`open_tab` / `open_app`) kecuali pengguna secara eksplisit meminta membuka situs atau browser (misal \"buka google\", \"buka browser\", \"buka tab ...\", \"putar youtube\").\n\
   - Untuk mencari file, panggil search_file (query singkat seperti \"cv\", JANGAN masukkan kalimat seperti \"dan taruh di shelf\" ke query). Set add_to_shelf=true hanya kalau user jelas meminta menaruh ke shelf, atau laporkan daftar hasilnya dulu lalu panggil shelf_add(path) untuk file yang sesuai.\n\
   - Untuk jadwal/agenda: gunakan calendar_add dengan title bersih (misal \"Rapat Himpunan\"), start=\"YYYY-MM-DDTHH:MM\", dan taruh link Google Meet/Zoom/Teams di field note (JANGAN masukkan URL ke title). Untuk mengedit atau menempel link ke jadwal yang sudah ada, cek calendar_list dulu lalu panggil calendar_update.\n\
   - Browser target: jika user menyebut browser tertentu (misal \"di Google Chrome\", \"di Chrome\", \"di Zen\", \"di Edge\"), SELALU isi parameter browser=\"chrome\" / \"zen\" / \"msedge\" / \"firefox\" pada open_tab, youtube, youtube_play, browser_snapshot, dan browser_act. Jika tidak disebut, kosongkan browser.\n\
   - Browser tahap awal (buka situs / cari / putar YouTube): gunakan open_tab, youtube, atau youtube_play (youtube & youtube_play otomatis memakai ulang tab YouTube yang sudah ada di browser target). JANGAN panggil browser_snapshot/browser_act jika hanya membuka halaman.\n\
   - Browser tahap lanjutan (interaksi halaman, Google Form, kuis multi-soal, YouTube, scroll):\n\
     * Untuk skip iklan: browser_act(action=\"skip_ad\"); untuk kontrol YouTube: browser_act(action=\"key\", key=\"l\"/\"j\"/\"k\"/\"space\").\n\
     * Untuk mengisi Google Form, kuis (misal 10-30 soal), atau beberapa input di halaman web: WAJIB awali dengan browser_snapshot(include_text=true) (snapshot otomatis menampilkan teks soal tepat di atas opsi @bN beserta status [checked] / val=\"...\" dalam urutan visual). Lalu kerjakan SEMUA soal yang sudah terlihat sekaligus dalam SATU panggilan browser_act(action=\"batch\", actions=[...klik/ketik @bN..., {{\"action\":\"scroll\",\"target\":\"down\",\"amount\":6}}], snapshot_after=true). Ulangi sampai seluruh halaman/soal selesai! JANGAN pernah memakai ulang @bN lama setelah scroll tanpa snapshot terbaru, dan jangan klik tombol Kirim/Submit sebelum semua bagian yang terlihat selesai diisi.\n\\n\
   - Ringkas pesan belum terbaca WhatsApp: cek notifs_list(unread_only=true), lalu buka open_tab(site=\"whatsapp\") dan panggil browser_snapshot(include_text=true) (jangan filter query=\"unread\"); bila perlu klik tombol filter \"Belum dibaca\" via browser_act(action=\"click\", target=\"Belum dibaca\") lalu browser_snapshot(include_text=true) lagi untuk merangkum pengirim dan isi pesannya.\n\
   - Aplikasi desktop di luar browser (misal buka/edit laporan di Word, olah data di Excel, klik menu aplikasi lokal): gunakan search_file + shell (PowerShell/COM/Python untuk olah data Word/Excel) dan/atau desktop_snapshot + desktop_act (UI Automation / agent-desktop). Jangan pakai desktop_* untuk halaman web browser.\n\
   - Untuk inspeksi laptop, cek file spesifik, isi dokumen/folder, atau menjalankan perintah Windows, gunakan shell dengan perintah PowerShell satu baris yang valid.\n\
   - Gunakan memory_save saat pengguna memberi tahu fakta penting/preferensi jangka panjang tentang dirinya, atau meminta diingat; jangan simpan obrolan sementara.\n\
   - Jika hasil tool masih belum cukup atau perlu langkah lanjutan, panggil tool berikutnya sebelum menjawab.\n\
3. {}",
        hari,
        now.format("%Y-%m-%d %H:%M"),
        now.format("%Y-%m-%dT%H:%M"),
        user_profile,
        style_rule
    );

    if !mem_block.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&mem_block);
    }
    prompt
}

fn parse_loose_param_val(raw: &str) -> Value {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(n) = t.parse::<i64>() {
        return Value::Number(n.into());
    }
    if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return v;
        }
    }
    Value::String(t.to_string())
}

pub fn parse_xml_tool_calls(text: &str) -> Vec<AgentToolCall> {
    static INVOKE_RE: OnceLock<Regex> = OnceLock::new();
    static PARAM_RE: OnceLock<Regex> = OnceLock::new();
    static TC_JSON_RE: OnceLock<Regex> = OnceLock::new();

    let invoke_re = INVOKE_RE.get_or_init(|| {
        Regex::new(r#"(?is)<invoke\s+name\s*=\s*["']([^"']+)["']\s*>([\s\S]*?)</invoke>"#).unwrap()
    });
    let param_re = PARAM_RE.get_or_init(|| {
        Regex::new(r#"(?is)<parameter\s+name\s*=\s*["']([^"']+)["']\s*>([\s\S]*?)</parameter>"#)
            .unwrap()
    });
    let tc_re = TC_JSON_RE
        .get_or_init(|| Regex::new(r"(?is)<tool_call>\s*(\{[\s\S]*?\})\s*</tool_call>").unwrap());

    let mut out = Vec::new();
    for (idx, cap) in invoke_re.captures_iter(text).enumerate() {
        let raw_name = cap[1].trim();
        let body = &cap[2];
        let mut raw_args = Map::new();
        for pcap in param_re.captures_iter(body) {
            let k = pcap[1].trim().to_string();
            let v = parse_loose_param_val(&pcap[2]);
            raw_args.insert(k, v);
        }
        let step_val = json!({ "name": raw_name, "args": raw_args });
        if let Some(step) = dn_normalize_step(&step_val) {
            out.push(AgentToolCall {
                id: format!("xml-call-{}", idx + 1),
                name: step.tool,
                args: step.args,
            });
        }
    }
    if !out.is_empty() {
        return out;
    }

    for (idx, cap) in tc_re.captures_iter(text).enumerate() {
        if let Ok(v) = serde_json::from_str::<Value>(cap[1].trim()) {
            let name = v
                .get("name")
                .or_else(|| v.get("tool"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let args_v = v
                .get("arguments")
                .or_else(|| v.get("args"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let step_val = json!({ "name": name, "args": args_v });
            if let Some(step) = dn_normalize_step(&step_val) {
                out.push(AgentToolCall {
                    id: format!("tc-call-{}", idx + 1),
                    name: step.tool,
                    args: step.args,
                });
            }
        }
    }
    out
}

pub fn parse_openai_tool_calls_from_msg(msg: &Value) -> Vec<AgentToolCall> {
    let Some(arr) = msg.get("tool_calls").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, tc) in arr.iter().enumerate() {
        let id = tc
            .get("id")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("call-{}", idx + 1));
        let fn_obj = tc.get("function");
        let raw_name = fn_obj
            .and_then(|f| f.get("name"))
            .or_else(|| tc.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let raw_args_val = fn_obj
            .and_then(|f| f.get("arguments"))
            .or_else(|| tc.get("arguments"));
        let args_map = match raw_args_val {
            Some(Value::Object(m)) => Value::Object(m.clone()),
            Some(Value::String(s)) => serde_json::from_str::<Value>(s.trim()).unwrap_or_else(|_| json!({})),
            _ => json!({}),
        };
        let step_val = json!({ "name": raw_name, "args": args_map });
        if let Some(step) = dn_normalize_step(&step_val) {
            out.push(AgentToolCall {
                id,
                name: step.tool,
                args: step.args,
            });
        }
    }
    out
}

pub async fn agent_step(
    store: &StoreManager,
    ui: Option<&UiSender>,
    surface: RouterSurface,
    messages: &[Value],
    allow_tools: bool,
) -> AgentTurnResult {
    let err_res = |msg: String| AgentTurnResult {
        content: String::new(),
        tool_calls: Vec::new(),
        raw_assistant_msg: None,
        used_native_tools: false,
        error: Some(msg),
    };

    let providers = resolve_configured_providers(store, ui, surface);
    if providers.is_empty() {
        return err_res(NO_PROVIDER_MSG.to_string());
    }

    let cl = match http_client() {
        Ok(c) => c,
        Err(e) => return err_res(e),
    };

    let build_body = |mdl: &str, msgs: &[Value], with_tools: bool| -> Value {
        if with_tools {
            json!({
                "model": mdl,
                "messages": msgs,
                "tools": build_openai_tools_schema(),
                "tool_choice": "auto",
                "stream": false
            })
        } else {
            json!({
                "model": mdl,
                "messages": msgs,
                "stream": false
            })
        }
    };

    static THINK_RE: OnceLock<Regex> = OnceLock::new();
    let think_re = THINK_RE.get_or_init(|| Regex::new(r"(?is)<think>[\s\S]*?</think>").unwrap());

    let allow_retry = providers.len() == 1;
    let mut errors = Vec::new();

    for ep in providers {
        let mut use_tools = allow_tools;
        let model = ep.model.clone();
        let body = build_body(&model, messages, use_tools);
        let (mut status, mut resp_text) = match send_with_hiccup_retry(
            &cl,
            reqwest::Method::POST,
            &ep.chat_url,
            &ep.api_key,
            Some(&body),
            allow_retry,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[SelaBar][AI] {} koneksi gagal ({}), cek fallback", ep.label, e);
                errors.push(format!("{} ({})", ep.label, e));
                continue;
            }
        };

        if is_rate_limit_or_quota(status, &resp_text) {
            mark_provider_rate_limited(ep.kind);
            eprintln!(
                "[SelaBar][AI] {} terkena limit (HTTP {}), fallback ke provider berikutnya",
                ep.label, status
            );
            errors.push(format!("{} (HTTP {} limit)", ep.label, status));
            continue;
        }

        // If the endpoint/model rejects `tools`, retry without native tools using JSON instructions.
        if use_tools && (status == 400 || status == 422) {
            use_tools = false;
            let mut fallback_msgs = messages.to_vec();
            fallback_msgs.push(json!({
                "role": "system",
                "content": "Endpoint tidak mendukung parameter `tools`. Jika perlu memanggil tool, balas JSON {\"steps\":[{\"tool\":\"nama_tool\",\"args\":{...}}],\"say\":\"\"}."
            }));
            let no_tool_body = build_body(&model, &fallback_msgs, false);
            if let Ok((st2, rt2)) = send_with_hiccup_retry(
                &cl,
                reqwest::Method::POST,
                &ep.chat_url,
                &ep.api_key,
                Some(&no_tool_body),
                false,
            )
            .await
            {
                if is_rate_limit_or_quota(st2, &rt2) {
                    mark_provider_rate_limited(ep.kind);
                    errors.push(format!("{} (HTTP {} limit)", ep.label, st2));
                    continue;
                }
                status = st2;
                resp_text = rt2;
            }
        }

        if status == 404 || status >= 500 {
            if let Some(ref fb) = ep.fallback_model {
                if fb != &model {
                    let fb_body = build_body(fb, messages, use_tools);
                    if let Ok((st2, rt2)) = send_with_hiccup_retry(
                        &cl,
                        reqwest::Method::POST,
                        &ep.chat_url,
                        &ep.api_key,
                        Some(&fb_body),
                        false,
                    )
                    .await
                    {
                        if is_rate_limit_or_quota(st2, &rt2) {
                            mark_provider_rate_limited(ep.kind);
                            errors.push(format!("{} (HTTP {} limit)", ep.label, st2));
                            continue;
                        }
                        if (200..300).contains(&st2) {
                            status = st2;
                            resp_text = rt2;
                        }
                    }
                }
            }
        }

        if !(200..300).contains(&status) {
            errors.push(format!("{} (HTTP {})", ep.label, status));
            continue;
        }

        clear_provider_cooldown(ep.kind);

        if let Ok(resp_json) = serde_json::from_str::<Value>(&resp_text) {
            if let Some(msg) = resp_json
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
            {
                let native_calls = parse_openai_tool_calls_from_msg(msg);
                if !native_calls.is_empty() {
                    let raw_content = msg
                        .get("content")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    let cleaned = think_re.replace_all(raw_content, "").trim().to_string();
                    let mut raw_msg = msg.clone();
                    if let Some(obj) = raw_msg.as_object_mut() {
                        obj.entry("role").or_insert_with(|| json!("assistant"));
                    }
                    return AgentTurnResult {
                        content: clean_markdown(&cleaned),
                        tool_calls: native_calls,
                        raw_assistant_msg: Some(raw_msg),
                        used_native_tools: true,
                        error: None,
                    };
                }

                let raw_content = msg
                    .get("content")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let cleaned = think_re.replace_all(raw_content, "").trim().to_string();
                let xml_calls = parse_xml_tool_calls(&cleaned);
                if !xml_calls.is_empty() {
                    return AgentTurnResult {
                        content: String::new(),
                        tool_calls: xml_calls,
                        raw_assistant_msg: Some(json!({ "role": "assistant", "content": cleaned })),
                        used_native_tools: false,
                        error: None,
                    };
                }

                if allow_tools
                    && cleaned.contains('{')
                    && (cleaned.contains("\"tool\"")
                        || cleaned.contains("\"steps\"")
                        || cleaned.contains("\"actions\""))
                {
                    let plan = parse_plan(&cleaned);
                    if !plan.steps.is_empty() {
                        let calls = plan
                            .steps
                            .into_iter()
                            .enumerate()
                            .map(|(i, s)| AgentToolCall {
                                id: format!("plan-call-{}", i + 1),
                                name: s.tool,
                                args: s.args,
                            })
                            .collect();
                        return AgentTurnResult {
                            content: clean_markdown(&plan.say),
                            tool_calls: calls,
                            raw_assistant_msg: Some(
                                json!({ "role": "assistant", "content": cleaned }),
                            ),
                            used_native_tools: false,
                            error: None,
                        };
                    }
                }

                if !cleaned.is_empty() {
                    return AgentTurnResult {
                        content: clean_markdown(&cleaned),
                        tool_calls: Vec::new(),
                        raw_assistant_msg: Some(json!({ "role": "assistant", "content": cleaned })),
                        used_native_tools: false,
                        error: None,
                    };
                }
            }
        }

        let extracted = extract_content(&resp_text);
        let cleaned = think_re.replace_all(&extracted, "").trim().to_string();
        if !cleaned.is_empty() {
            return AgentTurnResult {
                content: clean_markdown(&cleaned),
                tool_calls: Vec::new(),
                raw_assistant_msg: Some(json!({ "role": "assistant", "content": cleaned })),
                used_native_tools: false,
                error: None,
            };
        }
        errors.push(format!("{} (respons kosong)", ep.label));
    }

    err_res(format!("Semua provider AI gagal: {}", errors.join(" • ")))
}

pub fn dn_canon_tool(name: &str, obj: Option<&Map<String, Value>>) -> String {
    let low = name.trim().to_lowercase();
    if low.is_empty() {
        if let Some(m) = obj {
            if m.contains_key("command") || m.contains_key("cmd") || m.contains_key("script") {
                return "shell".to_string();
            }
        }
        return String::new();
    }
    match low.as_str() {
        "powershell" | "pwsh" | "ps" | "terminal" | "cmd" | "command" | "run" | "exec"
        | "execute" | "bash" | "sh" | "shell_command" | "run_command" | "system" | "script"
        | "powershell_command" => "shell".to_string(),
        "open" | "launch" | "start" | "app" | "open_application" | "run_app" => {
            "open_app".to_string()
        }
        "url" | "browser" | "tab" | "open_url" | "open_browser" | "website" | "open_site"
        | "youtube_tab" | "browser_tab" => "open_tab".to_string(),
        "find_file" | "find" | "file_search" | "search_files" | "locate" | "file" => {
            "search_file".to_string()
        }
        "volume_set" | "sound" | "audio" => "volume".to_string(),
        "play" | "play_video" => "youtube_play".to_string(),
        "youtube_search" | "search_youtube" => "youtube".to_string(),
        "todo" | "task" | "add_todo" | "add_task" => "todo_add".to_string(),
        "list_todos" | "todos" | "get_todos" => "todo_list".to_string(),
        "complete_todo" | "finish_todo" | "done_todo" => "todo_done".to_string(),
        "reminder" | "remind" | "alarm" | "add_reminder" => "reminder_add".to_string(),
        "add_calendar" | "create_event" | "schedule" | "add_event" | "add_schedule" => {
            "calendar_add".to_string()
        }
        "list_calendar" | "get_calendar" | "agenda" | "calendar_events" | "events" => {
            "calendar_list".to_string()
        }
        "update_calendar" | "edit_calendar" | "calendar_link" | "update_event" => {
            "calendar_update".to_string()
        }
        "delete_calendar" | "remove_calendar" | "delete_event" => "calendar_delete".to_string(),
        "add_shelf" | "put_shelf" | "pin_shelf" | "add_to_shelf" => "shelf_add".to_string(),
        "save_memory" | "remember" | "add_memory" | "memorize" => "memory_save".to_string(),
        "delete_memory" | "forget" | "remove_memory" => "memory_delete".to_string(),
        "list_memory" | "memories" | "get_memory" => "memory_list".to_string(),
        "list_notifs" | "notifications" | "get_notifs" => "notifs_list".to_string(),
        "exchange_rate" | "usd_idr" | "kurs" => "rate_usd_idr".to_string(),
        "media_control" | "music" | "player" => "media".to_string(),
        "theme_toggle" | "dark_mode" | "light_mode" => "theme".to_string(),
        "browser_use" | "browser_inspect" | "web_snapshot" | "page_snapshot" => {
            "browser_snapshot".to_string()
        }
        "browser_click" | "browser_type" | "skip_ad" | "youtube_skip_ad" | "web_click" => {
            "browser_act".to_string()
        }
        "agent_desktop" | "desktop_inspect" | "app_snapshot" | "ui_snapshot" => {
            "desktop_snapshot".to_string()
        }
        "desktop_click" | "desktop_type" | "desktop_key" | "app_click" => {
            "desktop_act".to_string()
        }
        _ => low,
    }
}

fn dn_normalize_step(raw: &Value) -> Option<PlanStep> {
    let obj = raw.as_object()?;
    let raw_tool = obj
        .get("tool")
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("action"))
        .or_else(|| obj.get("type"))
        .or_else(|| obj.get("command"))
        .or_else(|| obj.get("cmd"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool = dn_canon_tool(raw_tool, Some(obj));
    if tool.is_empty() {
        return None;
    }

    let mut args = match obj.get("args").and_then(|v| v.as_object()) {
        Some(a) => a.clone(),
        None => Map::new(),
    };

    const LIFT_KEYS: &[&str] = &[
        "cmd",
        "command",
        "script",
        "commandline",
        "command_line",
        "shell",
        "app",
        "site",
        "url",
        "browser",
        "include_text",
        "actions",
        "amount",
        "clear",
        "snapshot_after",
        "query",
        "channel",
        "level",
        "delta",
        "action",
        "target",
        "contact",
        "message",
        "text",
        "at",
        "hint",
        "mode",
        "key",
        "path",
        "file",
        "name",
        "value",
        "title",
        "start",
        "end",
        "note",
        "fact",
        "category",
        "date",
        "unread_only",
        "limit",
        "add_to_shelf",
        "id",
        "newest",
        "minutes",
        "new_title",
    ];
    for k in LIFT_KEYS {
        if !args.contains_key(*k) {
            if let Some(v) = obj.get(*k) {
                args.insert((*k).to_string(), v.clone());
            }
        }
    }

    let str_arg = |m: &Map<String, Value>, k: &str| -> Option<String> {
        m.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    match tool.as_str() {
        "shell" => {
            if !args.contains_key("cmd") {
                if let Some(c) = str_arg(&args, "command")
                    .or_else(|| str_arg(&args, "script"))
                    .or_else(|| str_arg(&args, "commandline"))
                    .or_else(|| str_arg(&args, "command_line"))
                    .or_else(|| str_arg(&args, "shell"))
                {
                    args.insert("cmd".to_string(), Value::String(c));
                }
            }
        }
        "open_app" => {
            if !args.contains_key("app") {
                if let Some(a) = str_arg(&args, "name") {
                    args.insert("app".to_string(), Value::String(a));
                }
            }
        }
        "open_tab" => {
            if !args.contains_key("site") {
                if let Some(s) = str_arg(&args, "url")
                    .or_else(|| str_arg(&args, "app"))
                    .or_else(|| str_arg(&args, "name"))
                {
                    args.insert("site".to_string(), Value::String(s));
                }
            }
        }
        "search_file" | "delete_file" => {
            if !args.contains_key("query") {
                if let Some(q) = str_arg(&args, "file").or_else(|| str_arg(&args, "name")) {
                    args.insert("query".to_string(), Value::String(q));
                }
            }
        }
        "shelf_add" => {
            if !args.contains_key("path") {
                if let Some(p) = str_arg(&args, "file")
                    .or_else(|| str_arg(&args, "query"))
                    .or_else(|| str_arg(&args, "value"))
                {
                    args.insert("path".to_string(), Value::String(p));
                }
            }
        }
        "calendar_add" | "calendar_update" => {
            if !args.contains_key("title") {
                if let Some(t) = str_arg(&args, "name").or_else(|| str_arg(&args, "text")) {
                    args.insert("title".to_string(), Value::String(t));
                }
            }
            if !args.contains_key("note") {
                if let Some(u) = str_arg(&args, "url") {
                    args.insert("note".to_string(), Value::String(u));
                }
            }
        }
        "memory_save" => {
            if !args.contains_key("fact") {
                if let Some(f) = str_arg(&args, "text")
                    .or_else(|| str_arg(&args, "message"))
                    .or_else(|| str_arg(&args, "value"))
                {
                    args.insert("fact".to_string(), Value::String(f));
                }
            }
        }
        "memory_delete" => {
            if !args.contains_key("id") && !args.contains_key("query") {
                if let Some(q) = str_arg(&args, "fact").or_else(|| str_arg(&args, "text")) {
                    args.insert("query".to_string(), Value::String(q));
                }
            }
        }
        "todo_add" | "reminder_add" => {
            if !args.contains_key("text") {
                if let Some(t) = str_arg(&args, "message") {
                    args.insert("text".to_string(), Value::String(t));
                }
            }
        }
        "todo_done" => {
            if !args.contains_key("id") && !args.contains_key("query") {
                if let Some(t) = str_arg(&args, "text") {
                    args.insert("query".to_string(), Value::String(t));
                }
            }
        }
        "browser_act" => {
            if !args.contains_key("action") {
                let raw_low = raw_tool.trim().to_lowercase();
                let inferred = if matches!(raw_low.as_str(), "skip_ad" | "youtube_skip_ad") {
                    "skip_ad"
                } else if raw_low == "browser_type" {
                    "type"
                } else {
                    "click"
                };
                args.insert("action".to_string(), Value::String(inferred.to_string()));
            }
        }
        "desktop_act" => {
            if !args.contains_key("action") {
                let raw_low = raw_tool.trim().to_lowercase();
                let inferred = if raw_low == "desktop_type" {
                    "type"
                } else if raw_low == "desktop_key" {
                    "key"
                } else {
                    "click"
                };
                args.insert("action".to_string(), Value::String(inferred.to_string()));
            }
        }
        _ => {}
    }

    Some(PlanStep { tool, args })
}

pub fn parse_plan(content: &str) -> ActionPlan {
    static THINK_RE: OnceLock<Regex> = OnceLock::new();
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();
    let think_re = THINK_RE.get_or_init(|| Regex::new(r"(?is)<think>[\s\S]*?</think>").unwrap());
    let fence_re =
        FENCE_RE.get_or_init(|| Regex::new(r"(?is)```(?:json)?\s*([\s\S]*?)\s*```").unwrap());

    let cleaned = think_re.replace_all(content, "").trim().to_string();
    let mut candidates = Vec::new();
    if let Some(cap) = fence_re.captures(&cleaned) {
        candidates.push(cap[1].trim().to_string());
    }
    candidates.push(cleaned.clone());
    if let (Some(a), Some(b)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if b > a {
            candidates.push(cleaned[a..=b].to_string());
        }
    }
    if let (Some(a), Some(b)) = (cleaned.find('['), cleaned.rfind(']')) {
        if b > a {
            candidates.push(cleaned[a..=b].to_string());
        }
    }

    let mut parsed: Option<Value> = None;
    for cand in candidates {
        if let Ok(v) = serde_json::from_str::<Value>(&cand) {
            parsed = Some(v);
            break;
        }
    }

    let Some(val) = parsed else {
        let fallback_say = clean_markdown(&cleaned);
        return ActionPlan {
            steps: Vec::new(),
            say: fallback_say,
            error: None,
        };
    };

    let raw_steps: Vec<Value> = if let Some(arr) = val.as_array() {
        arr.clone()
    } else if let Some(arr) = val
        .get("steps")
        .or_else(|| val.get("actions"))
        .or_else(|| val.get("plan"))
        .or_else(|| val.get("tools"))
        .and_then(|x| x.as_array())
    {
        arr.clone()
    } else if val.is_object() {
        let m = val.as_object().unwrap();
        if m.contains_key("tool")
            || m.contains_key("name")
            || m.contains_key("action")
            || m.contains_key("command")
            || m.contains_key("cmd")
        {
            vec![val.clone()]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let steps: Vec<PlanStep> = raw_steps
        .iter()
        .filter_map(dn_normalize_step)
        .take(6)
        .collect();

    let say = val
        .get("say")
        .or_else(|| val.get("message"))
        .or_else(|| val.get("reply"))
        .or_else(|| val.get("response"))
        .or_else(|| val.get("explanation"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    ActionPlan {
        steps,
        say,
        error: None,
    }
}

fn sup_char(ch: char) -> char {
    match ch {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => ch,
    }
}

fn sup_str(s: &str) -> String {
    s.chars().map(sup_char).collect()
}

fn math_sym(name: &str) -> Option<&'static str> {
    Some(match name {
        "times" => "×",
        "cdot" => "·",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "sim" => "≃",
        "infty" => "∞",
        "sum" => "Σ",
        "prod" => "Π",
        "int" => "∫",
        "partial" => "∂",
        "pi" => "π",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "theta" => "θ",
        "lambda" => "λ",
        "mu" => "µ",
        "sigma" => "σ",
        "phi" => "φ",
        "omega" => "ω",
        "Delta" => "Δ",
        "delta" => "δ",
        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "Rightarrow" => "⇒",
        "ldots" | "dots" | "cdots" => "…",
        "prime" => "′",
        "circ" => "°",
        "quad" | "qquad" => " ",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        _ => return None,
    })
}

fn read_balanced_arg(chars: &[char], idx: usize) -> Option<(String, usize)> {
    let open = *chars.get(idx)?;
    let close = match open {
        '{' => '}',
        '(' => ')',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut out = String::new();
    for j in idx..chars.len() {
        let c = chars[j];
        if c == open {
            depth += 1;
            if depth == 1 && j == idx {
                continue;
            }
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some((out, j + 1));
            }
        }
        out.push(c);
    }
    Some((out, chars.len()))
}

fn strip_math_dollars(input: &str) -> String {
    let no_block = input.replace("$$", "");
    let chars: Vec<char> = no_block.chars().collect();
    let mut out = String::with_capacity(no_block.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && (i == 0 || chars[i - 1] != '\\') {
            let mut j = i + 1;
            let mut close = None;
            while j < chars.len() && chars[j] != '\n' && chars[j] != '\r' {
                if chars[j] == '$' && chars[j - 1] != '\\' {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(c) = close {
                let inner: String = chars[i + 1..c].iter().collect();
                let first_ws = inner.chars().next().is_some_and(|ch| ch.is_whitespace());
                let last_ws = inner.chars().last().is_some_and(|ch| ch.is_whitespace());
                let first_digit = inner.chars().next().is_some_and(|ch| ch.is_ascii_digit());
                let looks_math = !inner.is_empty()
                    && !first_ws
                    && !last_ws
                    && !inner.starts_with('(')
                    && (inner.contains('\\')
                        || inner.contains('^')
                        || inner.contains('{')
                        || (!first_digit && inner.len() <= 10 && !inner.contains(' ')));
                if looks_math {
                    out.push_str(&inner);
                    i = c + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn dn_de_math(input: &str, depth: usize) -> String {
    if input.is_empty() {
        return String::new();
    }
    if depth > 3 {
        return input.to_string();
    }
    let s = strip_math_dollars(input);
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            if name.is_empty() {
                if chars.get(i + 1) == Some(&'\\') {
                    out.push('\n');
                    i += 2;
                } else {
                    out.push('\\');
                    i += 1;
                }
                continue;
            }
            if name == "n" {
                out.push('\n');
                i = j;
                continue;
            }
            if name == "t" {
                out.push(' ');
                i = j;
                continue;
            }
            if matches!(name.as_str(), "frac" | "dfrac" | "tfrac") {
                if let Some((a, a_end)) = read_balanced_arg(&chars, j) {
                    let mut k = a_end;
                    while chars.get(k) == Some(&' ') {
                        k += 1;
                    }
                    if let Some((b, b_end)) = read_balanced_arg(&chars, k) {
                        out.push('(');
                        out.push_str(&dn_de_math(&a, depth + 1));
                        out.push_str(") / (");
                        out.push_str(&dn_de_math(&b, depth + 1));
                        out.push(')');
                        i = b_end;
                        continue;
                    }
                    out.push('(');
                    out.push_str(&a);
                    out.push(')');
                    i = a_end;
                    continue;
                }
            }
            if name == "sqrt" {
                if let Some((r, r_end)) = read_balanced_arg(&chars, j) {
                    out.push_str("√(");
                    out.push_str(&dn_de_math(&r, depth + 1));
                    out.push(')');
                    i = r_end;
                    continue;
                }
            }
            if matches!(
                name.as_str(),
                "text" | "mathrm" | "mathbf" | "mathit" | "operatorname" | "mbox" | "textbf"
                    | "textit" | "mathsf"
            ) {
                if let Some((q, q_end)) = read_balanced_arg(&chars, j) {
                    out.push_str(&q);
                    i = q_end;
                    continue;
                }
            }
            if name == "left" || name == "right" {
                i = j;
                continue;
            }
            if let Some(sym) = math_sym(&name) {
                out.push_str(sym);
            } else {
                out.push('\\');
                out.push_str(&name);
            }
            i = j;
            continue;
        }

        if ch == '^' {
            if let Some(&nxt) = chars.get(i + 1) {
                if nxt == '{' || nxt == '(' {
                    if let Some((sr, end)) = read_balanced_arg(&chars, i + 1) {
                        out.push_str(&sup_str(&sr));
                        i = end;
                        continue;
                    }
                }
                if nxt.is_ascii_digit() || nxt.is_ascii_alphabetic() {
                    out.push(sup_char(nxt));
                    i += 2;
                    continue;
                }
            }
        }

        if ch == '_' {
            if let Some(&nxt) = chars.get(i + 1) {
                if nxt == '{' || nxt == '(' {
                    if let Some((br, end)) = read_balanced_arg(&chars, i + 1) {
                        out.push('(');
                        out.push_str(&br);
                        out.push(')');
                        i = end;
                        continue;
                    }
                }
                let prev_alnum = i > 0 && chars[i - 1].is_ascii_alphanumeric();
                let next_after = chars.get(i + 2).copied();
                let single_digit_sub = prev_alnum
                    && nxt.is_ascii_digit()
                    && next_after.map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_');
                if single_digit_sub {
                    out.push('(');
                    out.push(nxt);
                    out.push(')');
                    i += 2;
                    continue;
                }
            }
        }

        out.push(ch);
        i += 1;
    }

    let cleaned = out.replace(['{', '}'], "");
    static HS_RE: OnceLock<Regex> = OnceLock::new();
    static NL_RE: OnceLock<Regex> = OnceLock::new();
    let hs = HS_RE.get_or_init(|| Regex::new(r"[ \t]{2,}").unwrap());
    let nl = NL_RE.get_or_init(|| Regex::new(r"\n{3,}").unwrap());
    let s1 = hs.replace_all(&cleaned, " ");
    nl.replace_all(&s1, "\n\n").trim().to_string()
}

pub fn clean_markdown(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    static FENCE_OPEN_RE: OnceLock<Regex> = OnceLock::new();
    static HR_RE: OnceLock<Regex> = OnceLock::new();
    static HEAD_RE: OnceLock<Regex> = OnceLock::new();
    static BQ_RE: OnceLock<Regex> = OnceLock::new();
    static TBL_SEP_RE: OnceLock<Regex> = OnceLock::new();
    static BOLD_RE: OnceLock<Regex> = OnceLock::new();
    static ITALIC_RE: OnceLock<Regex> = OnceLock::new();
    static CODE_RE: OnceLock<Regex> = OnceLock::new();
    static BULLET_RE: OnceLock<Regex> = OnceLock::new();
    static HS_RE: OnceLock<Regex> = OnceLock::new();
    static NL_RE: OnceLock<Regex> = OnceLock::new();

    let mut t = dn_de_math(text, 0).replace("\r\n", "\n");
    t = FENCE_OPEN_RE
        .get_or_init(|| Regex::new(r"(?i)```[a-z]*\n?").unwrap())
        .replace_all(&t, "")
        .replace("```", "");
    t = HR_RE
        .get_or_init(|| Regex::new(r"\s*[-*_]{3,}\s*").unwrap())
        .replace_all(&t, "\n\n")
        .to_string();
    t = HEAD_RE
        .get_or_init(|| Regex::new(r"[ \t]*#{1,6}[ \t]*").unwrap())
        .replace_all(&t, "\n\n")
        .to_string();
    t = BQ_RE
        .get_or_init(|| Regex::new(r"(?m)^[ \t]*>[ \t]*").unwrap())
        .replace_all(&t, "\n")
        .to_string();
    t = TBL_SEP_RE
        .get_or_init(|| Regex::new(r"\|[-:\s|]+\|").unwrap())
        .replace_all(&t, "\n")
        .replace('|', " ");
    t = BOLD_RE
        .get_or_init(|| Regex::new(r"\*\*(.+?)\*\*").unwrap())
        .replace_all(&t, "$1")
        .to_string();
    t = ITALIC_RE
        .get_or_init(|| Regex::new(r"\*([^*\n]+)\*").unwrap())
        .replace_all(&t, "$1")
        .to_string();
    t = CODE_RE
        .get_or_init(|| Regex::new(r"`([^`\n]+)`").unwrap())
        .replace_all(&t, "$1")
        .to_string();
    t = BULLET_RE
        .get_or_init(|| Regex::new(r"(?m)^[ \t]*[-*+][ \t]+").unwrap())
        .replace_all(&t, "• ")
        .to_string();
    t = HS_RE
        .get_or_init(|| Regex::new(r"[ \t]{2,}").unwrap())
        .replace_all(&t, " ")
        .to_string();
    NL_RE
        .get_or_init(|| Regex::new(r"\n{3,}").unwrap())
        .replace_all(&t, "\n\n")
        .trim()
        .to_string()
}

pub fn convo_load(store: &StoreManager) -> Vec<ConvoEntry> {
    let raw = store.get("dnConvo");
    let arr_val = if let Some(s) = raw.as_str() {
        serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
    } else {
        raw
    };
    let Some(arr) = arr_val.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<ConvoEntry> = arr
        .iter()
        .filter_map(|v| {
            let role = v.get("role").and_then(|x| x.as_str())?;
            let text = v.get("text").and_then(|x| x.as_str())?;
            let norm_role = if role.eq_ignore_ascii_case("user") {
                "user"
            } else {
                "assistant"
            };
            let clipped: String = clean_markdown(text).chars().take(8000).collect();
            if clipped.is_empty() {
                return None;
            }
            let at = v.get("at").and_then(|x| x.as_i64()).unwrap_or(0);
            let cmd = v
                .get("cmd")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let cmd_ok = v.get("cmd_ok").and_then(|x| x.as_bool());
            Some(ConvoEntry {
                role: norm_role.to_string(),
                text: clipped,
                at,
                cmd,
                cmd_ok,
            })
        })
        .collect();
    if out.len() > 30 {
        out.drain(..out.len() - 30);
    }
    out
}

pub fn convo_recent(store: &StoreManager, n: usize) -> Vec<ConvoEntry> {
    let list = convo_load(store);
    if list.len() <= n {
        list
    } else {
        list[list.len() - n..].to_vec()
    }
}

pub fn convo_remember(store: &StoreManager, ui: Option<&UiSender>, role: &str, text: &str) {
    convo_remember_with_cmd(store, ui, role, text, "", None);
}

pub fn convo_remember_with_cmd(
    store: &StoreManager,
    ui: Option<&UiSender>,
    role: &str,
    text: &str,
    cmd: &str,
    cmd_ok: Option<bool>,
) {
    let norm_role = if role.eq_ignore_ascii_case("user") {
        "user"
    } else {
        "assistant"
    };
    let clipped: String = clean_markdown(text).chars().take(8000).collect();
    if clipped.is_empty() {
        return;
    }
    let at = now_unix_ms();
    let mut list = convo_load(store);
    if let Some(last) = list.last() {
        if last.role == norm_role && last.text == clipped && (at - last.at).abs() < 60_000 {
            return;
        }
    }
    list.push(ConvoEntry {
        role: norm_role.to_string(),
        text: clipped,
        at,
        cmd: cmd.to_string(),
        cmd_ok,
    });
    if list.len() > 30 {
        list.drain(..list.len() - 30);
    }
    if let Ok(v) = serde_json::to_value(&list) {
        store.set(ui, "dnConvo", v);
    }
}

pub fn convo_clear(store: &StoreManager, ui: Option<&UiSender>) {
    store.set(ui, "dnConvo", Value::Array(Vec::new()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demath_and_markdown() {
        assert_eq!(dn_de_math(r"$$x^2 + \frac{1}{2} = \pi$$", 0), "x² + (1) / (2) = π");
        assert_eq!(
            dn_de_math(r"$os = Get-CimInstance Win32_OperatingSystem", 0),
            "$os = Get-CimInstance Win32_OperatingSystem"
        );
        assert_eq!(
            dn_de_math(r"C:\Users\Demo\my_file_01", 0),
            r"C:\Users\Demo\my_file_01"
        );
        assert_eq!(clean_markdown("**Halo**\n- satu\n- dua"), "Halo\n• satu\n• dua");
    }

    #[test]
    fn parses_plan_fenced_and_aliases() {
        let raw = "<think>hmm</think>\n```json\n{\"steps\":[{\"tool\":\"powershell\",\"command\":\"Get-Date\"}],\"say\":\"Oke\"}\n```";
        let plan = parse_plan(raw);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].tool, "shell");
        assert_eq!(
            plan.steps[0].args.get("cmd").and_then(|v| v.as_str()),
            Some("Get-Date")
        );
        assert_eq!(plan.say, "Oke");
    }

    #[test]
    fn parses_openai_tool_calls_xml_invoke_and_calendar_lift() {
        let msg = json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "search_file",
                    "arguments": "{\"query\":\"cv\",\"add_to_shelf\":false}"
                }
            }]
        });
        let calls = parse_openai_tool_calls_from_msg(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_123");
        assert_eq!(calls[0].name, "search_file");
        assert_eq!(calls[0].args.get("query").and_then(|v| v.as_str()), Some("cv"));
        assert_eq!(calls[0].args.get("add_to_shelf").and_then(|v| v.as_bool()), Some(false));

        let xml = r#"<minimax:tool_call><invoke name="calendar_add"><parameter name="title">Rapat Himpunan</parameter><parameter name="start">2026-11-19T19:45</parameter><parameter name="note">https://meet.google.com/hya-ermn-afg</parameter></invoke></minimax:tool_call>"#;
        let xcalls = parse_xml_tool_calls(xml);
        assert_eq!(xcalls.len(), 1);
        assert_eq!(xcalls[0].name, "calendar_add");
        assert_eq!(xcalls[0].args.get("title").and_then(|v| v.as_str()), Some("Rapat Himpunan"));
        assert_eq!(xcalls[0].args.get("start").and_then(|v| v.as_str()), Some("2026-11-19T19:45"));
        assert_eq!(
            xcalls[0].args.get("note").and_then(|v| v.as_str()),
            Some("https://meet.google.com/hya-ermn-afg")
        );

        let lifted = json!({
            "steps": [{
                "tool": "schedule",
                "title": "Rapat Himpunan",
                "start": "2026-11-19T19:45",
                "note": "https://meet.google.com/hya-ermn-afg"
            }]
        });
        let plan = parse_plan(&lifted.to_string());
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].tool, "calendar_add");
        assert_eq!(
            plan.steps[0].args.get("note").and_then(|v| v.as_str()),
            Some("https://meet.google.com/hya-ermn-afg")
        );

        let xml_auto = r#"<invoke name="skip_ad"></invoke><invoke name="agent_desktop"><parameter name="app">Excel</parameter></invoke>"#;
        let auto_calls = parse_xml_tool_calls(xml_auto);
        assert_eq!(auto_calls.len(), 2);
        assert_eq!(auto_calls[0].name, "browser_act");
        assert_eq!(auto_calls[0].args.get("action").and_then(|v| v.as_str()), Some("skip_ad"));
        assert_eq!(auto_calls[1].name, "desktop_snapshot");
        assert_eq!(auto_calls[1].args.get("app").and_then(|v| v.as_str()), Some("Excel"));
    }

    #[test]
    fn allows_local_and_https_router_urls_only() {
        assert!(is_allowed_router_url("http://127.0.0.1:11434/v1/chat/completions"));
        assert!(is_allowed_router_url("http://localhost:11434/v1/models"));
        assert!(is_allowed_router_url("https://openrouter.ai/api/v1/chat/completions"));
        assert!(is_allowed_router_url("https://api.openai.com/v1/chat/completions"));
        assert!(!is_allowed_router_url("http://example.com/v1/chat/completions"));
        assert!(!is_allowed_router_url("file:///C:/secret.txt"));
        assert!(!is_allowed_router_url("https://evil.com/path with spaces"));
    }

    #[test]
    fn build_endpoint_urls_handles_common_provider_formats() {
        assert_eq!(build_endpoint_urls(""), None);
        assert_eq!(build_endpoint_urls("   "), None);
        assert_eq!(
            build_endpoint_urls("http://127.0.0.1:11434"),
            Some((
                "http://127.0.0.1:11434/v1/chat/completions".to_string(),
                "http://127.0.0.1:11434/v1/models".to_string(),
            ))
        );
        assert_eq!(
            build_endpoint_urls("https://api.openai.com/v1/"),
            Some((
                "https://api.openai.com/v1/chat/completions".to_string(),
                "https://api.openai.com/v1/models".to_string(),
            ))
        );
        assert_eq!(
            build_endpoint_urls(GEMINI_OPENAI_BASE_URL),
            Some((
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions".to_string(),
                "https://generativelanguage.googleapis.com/v1beta/openai/models".to_string(),
            ))
        );
        assert_eq!(
            build_endpoint_urls("https://api.groq.com/openai/v1"),
            Some((
                "https://api.groq.com/openai/v1/chat/completions".to_string(),
                "https://api.groq.com/openai/v1/models".to_string(),
            ))
        );
        assert_eq!(
            build_endpoint_urls("https://openrouter.ai/api/v1"),
            Some((
                "https://openrouter.ai/api/v1/chat/completions".to_string(),
                "https://openrouter.ai/api/v1/models".to_string(),
            ))
        );
        assert_eq!(
            build_endpoint_urls("https://custom.example.com/proxy/chat/completions"),
            Some((
                "https://custom.example.com/proxy/chat/completions".to_string(),
                "https://custom.example.com/proxy/models".to_string(),
            ))
        );
    }

    #[test]
    fn detects_rate_limit_and_quota_exhaustion() {
        assert!(is_rate_limit_or_quota(429, ""));
        assert!(is_rate_limit_or_quota(402, ""));
        assert!(is_rate_limit_or_quota(400, "{\"error\":{\"status\":\"RESOURCE_EXHAUSTED\"}}"));
        assert!(is_rate_limit_or_quota(403, "insufficient_quota for project"));
        assert!(!is_rate_limit_or_quota(200, "{\"choices\":[]}"));
        assert!(!is_rate_limit_or_quota(500, "internal server error"));
    }

    #[test]
    fn provider_ordering_prefers_primary_and_demotes_cooldown() {
        let ep = |kind: AiProviderKind, label: &'static str| AiProviderEndpoint {
            kind,
            label,
            chat_url: "https://example.com/v1/chat/completions".to_string(),
            models_url: "https://example.com/v1/models".to_string(),
            api_key: "k".to_string(),
            model: "m".to_string(),
            fallback_model: None,
        };
        let list = vec![
            ep(AiProviderKind::Router, "R"),
            ep(AiProviderKind::Gemini, "G"),
            ep(AiProviderKind::OpenAi, "O"),
        ];

        let no_cd = [false, false, false];
        let g_first = order_providers_with_cooldowns(list.clone(), "gemini", &no_cd);
        assert_eq!(
            g_first.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![AiProviderKind::Gemini, AiProviderKind::OpenAi, AiProviderKind::Router]
        );

        let gem_limited = [false, true, false];
        let g_demoted = order_providers_with_cooldowns(list, "gemini", &gem_limited);
        assert_eq!(
            g_demoted.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![AiProviderKind::OpenAi, AiProviderKind::Router, AiProviderKind::Gemini]
        );
    }
}
