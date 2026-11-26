use super::{dsp, edge_tts};
use crate::state::now_unix_ms;
use regex::Regex;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{Interface, Result, HSTRING};
use windows::Media::SpeechSynthesis::{
    SpeechSynthesisStream, SpeechSynthesizer, VoiceInformation,
};
use windows::Storage::Streams::{DataReader, IInputStream};
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

impl VoiceInfo {
    pub fn display_label(&self) -> String {
        if self.language.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.language)
        }
    }

    pub fn is_indonesian(&self) -> bool {
        let l = self.language.to_ascii_lowercase();
        let n = self.name.to_ascii_lowercase();
        l == "id-id"
            || l.starts_with("id-")
            || n.contains("andika")
            || n.contains("gadis")
            || n.contains("ardi")
            || n.contains("indonesia")
    }

    pub fn is_edge_neural(&self) -> bool {
        self.id.starts_with("edge:")
    }
}

pub fn pick_best_voice_index(
    voices: &[VoiceInfo],
    preferred: &str,
    default_voice_id: Option<&str>,
) -> Option<usize> {
    if voices.is_empty() {
        return None;
    }
    let pref = preferred.trim();
    if !pref.is_empty() && !pref.eq_ignore_ascii_case("auto") {
        if let Some(idx) = voices
            .iter()
            .position(|v| v.id == pref || v.name.eq_ignore_ascii_case(pref))
        {
            return Some(idx);
        }
    }
    // 1. Prefer Indonesian voice (`id-ID` / Gadis / Ardi / Andika)
    if let Some(idx) = voices.iter().position(|v| v.is_indonesian()) {
        return Some(idx);
    }
    // 2. Closest phonetic relative (`ms-MY`)
    if let Some(idx) = voices
        .iter()
        .position(|v| v.language.to_ascii_lowercase().starts_with("ms"))
    {
        return Some(idx);
    }
    // 3. Follow active Windows system DefaultVoice
    if let Some(def_id) = default_voice_id {
        if let Some(idx) = voices.iter().position(|v| v.id == def_id) {
            return Some(idx);
        }
    }
    // 4. First available voice
    Some(0)
}

/// Picks the best offline Windows WinRT voice (`!id.starts_with("edge:")`) for offline fallback.
pub fn pick_best_offline_voice_index(
    voices: &[VoiceInfo],
    default_voice_id: Option<&str>,
) -> Option<usize> {
    // 1. Prefer local Indonesian voice (e.g. Microsoft Andika)
    if let Some(idx) = voices
        .iter()
        .position(|v| !v.is_edge_neural() && v.is_indonesian())
    {
        return Some(idx);
    }
    // 2. Closest phonetic relative (`ms-MY`)
    if let Some(idx) = voices
        .iter()
        .position(|v| !v.is_edge_neural() && v.language.to_ascii_lowercase().starts_with("ms"))
    {
        return Some(idx);
    }
    // 3. Windows system DefaultVoice if local
    if let Some(def_id) = default_voice_id {
        if let Some(idx) = voices
            .iter()
            .position(|v| !v.is_edge_neural() && v.id == def_id)
        {
            return Some(idx);
        }
    }
    // 4. First installed local WinRT voice
    voices.iter().position(|v| !v.is_edge_neural())
}

fn is_tts_stripped_symbol(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        0x1F000..=0x1FAFF
            | 0x2600..=0x27BF
            | 0x2300..=0x23FF
            | 0x2B00..=0x2BFF
            | 0xFE00..=0xFE0F
            | 0xE0020..=0xE007F
            | 0x200D
            | 0x20E3
    ) || matches!(
        c,
        '✓' | '✔' | '✗' | '✘' | '×' | '★' | '☆' | '▪' | '▸' | '►' | '→' | '←' | '↑' | '↓'
    )
}

pub fn sanitize_for_tts(raw: &str) -> String {
    static MD_LINK_RE: OnceLock<Regex> = OnceLock::new();
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    static WIN_PATH_RE: OnceLock<Regex> = OnceLock::new();
    static HEADING_RE: OnceLock<Regex> = OnceLock::new();
    static MULTI_WS_RE: OnceLock<Regex> = OnceLock::new();
    static MULTI_COMMA_RE: OnceLock<Regex> = OnceLock::new();

    if raw.trim().is_empty() {
        return String::new();
    }

    let mut s = MD_LINK_RE
        .get_or_init(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap())
        .replace_all(raw, "$1")
        .to_string();

    s = URL_RE
        .get_or_init(|| Regex::new(r"(?i)https?://\S+").unwrap())
        .replace_all(&s, "")
        .to_string();

    s = WIN_PATH_RE
        .get_or_init(|| Regex::new(r#"[A-Za-z]:\\[^\s,;:"'()\[\]<>]+"#).unwrap())
        .replace_all(&s, |caps: &regex::Captures| {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let trimmed = full.trim_end_matches('.');
            Path::new(trimmed)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(trimmed)
                .to_string()
        })
        .to_string();

    s = HEADING_RE
        .get_or_init(|| Regex::new(r"(?m)^\s*#{1,6}\s*").unwrap())
        .replace_all(&s, "")
        .to_string();

    s = s
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .replace("~~~", "")
        .replace('~', "")
        .replace('|', " ")
        .replace('$', "")
        .replace('*', "");

    let mut cleaned = String::with_capacity(s.len());
    for c in s.chars() {
        if is_tts_stripped_symbol(c) {
            cleaned.push(' ');
        } else if matches!(c, '•' | '·' | '—' | '–') {
            cleaned.push_str(", ");
        } else if c == '…' {
            cleaned.push('.');
        } else {
            cleaned.push(c);
        }
    }

    let ws_collapsed = MULTI_WS_RE
        .get_or_init(|| Regex::new(r"\s+").unwrap())
        .replace_all(&cleaned, " ")
        .to_string();
    let punct_cleaned = MULTI_COMMA_RE
        .get_or_init(|| Regex::new(r"([,.!?])(?:\s*[,.!?])+").unwrap())
        .replace_all(&ws_collapsed, "$1")
        .to_string();

    punct_cleaned
        .trim()
        .trim_start_matches(&[',', ';', ':'][..])
        .trim()
        .to_string()
}

fn escape_xml_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 12);
    for ch in raw.chars() {
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

fn read_speech_stream_bytes(stream: SpeechSynthesisStream) -> Result<Vec<u8>> {
    let size = stream.Size()? as usize;
    let input_stream: IInputStream = stream.cast()?;
    let reader = DataReader::CreateDataReader(&input_stream)?;
    reader.LoadAsync(size as u32)?.get()?;
    let mut buf = vec![0u8; size];
    reader.ReadBytes(&mut buf)?;
    Ok(buf)
}

pub struct WinRtTts {
    synthesizer: Option<SpeechSynthesizer>,
    voices: Vec<(VoiceInfo, Option<VoiceInformation>)>,
    active_voice: Arc<Mutex<Option<VoiceInfo>>>,
    default_voice_id: Option<String>,
    preferred_id: Arc<Mutex<String>>,
    last_fallback_offline: Arc<AtomicBool>,
    edge_cooldown_until_ms: Arc<AtomicU64>,
    edge_fail_streak: Arc<AtomicU64>,
    speak_gen: Arc<AtomicU64>,
    current_buffer: Arc<Mutex<Option<Vec<u8>>>>,
    speaking_until_ms: Arc<AtomicU64>,
}

static MTA_KEEPALIVE: OnceLock<usize> = OnceLock::new();

pub(crate) fn ensure_com_mta_alive() {
    MTA_KEEPALIVE.get_or_init(|| unsafe {
        match windows::Win32::System::Com::CoIncrementMTAUsage() {
            Ok(cookie) => cookie.0 as usize,
            Err(_) => 0,
        }
    });
}

pub(crate) fn should_enter_edge_cooldown(fail_streak: u64) -> bool {
    fail_streak >= 2
}

impl WinRtTts {
    pub fn new() -> Result<Self> {
        ensure_com_mta_alive();
        // Prepend built-in Edge Neural voices so Online Neural is default even before WinRT voices
        let mut voices: Vec<(VoiceInfo, Option<VoiceInformation>)> = edge_tts::builtin_edge_voices()
            .into_iter()
            .map(|info| (info, None))
            .collect();
        let mut default_voice_id: Option<String> = None;

        let synth = match SpeechSynthesizer::new() {
            Ok(s) => {
                if let Ok(def) = SpeechSynthesizer::DefaultVoice() {
                    if let Ok(did) = def.Id() {
                        default_voice_id = Some(did.to_string());
                    }
                }
                let mut winrt_count = 0usize;
                if let Ok(all) = SpeechSynthesizer::AllVoices() {
                    let count = all.Size().unwrap_or(0);
                    for i in 0..count {
                        if let Ok(v) = all.GetAt(i) {
                            let id = v.Id().map(|s| s.to_string()).unwrap_or_default();
                            let name = v.DisplayName().map(|s| s.to_string()).unwrap_or_default();
                            let language = v.Language().map(|s| s.to_string()).unwrap_or_default();
                            voices.push((VoiceInfo { id, name, language }, Some(v)));
                            winrt_count += 1;
                        }
                    }
                }
                if winrt_count == 0 {
                    None
                } else {
                    Some(s)
                }
            }
            Err(_) => None,
        };

        let instance = Self {
            synthesizer: synth,
            voices,
            active_voice: Arc::new(Mutex::new(None)),
            default_voice_id,
            preferred_id: Arc::new(Mutex::new("auto".to_string())),
            last_fallback_offline: Arc::new(AtomicBool::new(false)),
            edge_cooldown_until_ms: Arc::new(AtomicU64::new(0)),
            edge_fail_streak: Arc::new(AtomicU64::new(0)),
            speak_gen: Arc::new(AtomicU64::new(0)),
            current_buffer: Arc::new(Mutex::new(None)),
            speaking_until_ms: Arc::new(AtomicU64::new(0)),
        };
        instance.apply_voice_preference("auto");
        Ok(instance)
    }

    pub fn voice_infos(&self) -> Vec<VoiceInfo> {
        self.voices.iter().map(|(info, _)| info.clone()).collect()
    }

    /// Checks whether a local offline Indonesian WinRT voice (e.g. Microsoft Andika) is installed.
    pub fn has_indonesian_voice(&self) -> bool {
        self.voices
            .iter()
            .any(|(info, _)| !info.is_edge_neural() && info.is_indonesian())
    }

    pub fn active_voice_label(&self) -> String {
        if let Ok(guard) = self.active_voice.lock() {
            if let Some(ref v) = *guard {
                if v.is_edge_neural() {
                    if self.last_fallback_offline.load(Ordering::Relaxed) {
                        return format!("{} → Offline Fallback", v.name);
                    }
                    return format!("{} (Online • Fallback Offline)", v.name);
                }
                return format!("{} (Offline)", v.display_label());
            }
        }
        "Windows System SAPI5 (Offline)".to_string()
    }

    pub fn apply_voice_preference(&self, preferred: &str) {
        if let Ok(mut guard) = self.preferred_id.lock() {
            *guard = preferred.trim().to_string();
        }
        self.edge_cooldown_until_ms.store(0, Ordering::Relaxed);
        self.edge_fail_streak.store(0, Ordering::Relaxed);
        self.last_fallback_offline.store(false, Ordering::Relaxed);

        let infos: Vec<VoiceInfo> = self.voice_infos();
        let chosen_idx =
            pick_best_voice_index(&infos, preferred, self.default_voice_id.as_deref());
        if let Some(idx) = chosen_idx {
            let chosen = &infos[idx];
            if chosen.is_edge_neural() {
                // Pre-warm best local WinRT voice so offline fallback speaks immediately
                if let Some(off_idx) =
                    pick_best_offline_voice_index(&infos, self.default_voice_id.as_deref())
                {
                    if let (Some(synth), Some(winrt_voice)) =
                        (&self.synthesizer, &self.voices[off_idx].1)
                    {
                        let _ = synth.SetVoice(winrt_voice);
                    }
                }
            } else if let (Some(synth), Some(winrt_voice)) =
                (&self.synthesizer, &self.voices[idx].1)
            {
                let _ = synth.SetVoice(winrt_voice);
            }
            if let Ok(mut guard) = self.active_voice.lock() {
                *guard = Some(chosen.clone());
            }
        } else if let Ok(mut guard) = self.active_voice.lock() {
            *guard = None;
        }
    }

    pub fn list_voices(&self) -> Vec<serde_json::Value> {
        self.voices
            .iter()
            .map(|(v, _)| serde_json::json!({ "id": v.id, "name": v.name, "language": v.language }))
            .collect()
    }

    fn offline_voice_lang(&self) -> String {
        let infos = self.voice_infos();
        if let Ok(guard) = self.active_voice.lock() {
            if let Some(ref v) = *guard {
                if !v.is_edge_neural() && !v.language.is_empty() {
                    return v.language.clone();
                }
            }
        }
        if let Some(idx) =
            pick_best_offline_voice_index(&infos, self.default_voice_id.as_deref())
        {
            if !infos[idx].language.is_empty() {
                return infos[idx].language.clone();
            }
        }
        "id-ID".to_string()
    }

    fn synthesize_sapi_fallback(spoken: &str, prefer_id: bool) -> std::io::Result<Vec<u8>> {
        let hint = if prefer_id {
            "try { $s.SelectVoiceByHints([System.Speech.Synthesis.VoiceGender]::NotSet, [System.Speech.Synthesis.VoiceAge]::NotSet, 0, [System.Globalization.CultureInfo]::GetCultureInfo('id-ID')) } catch {};"
        } else {
            ""
        };
        let ps = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; {} $ms = New-Object System.IO.MemoryStream; $s.SetOutputToWaveStream($ms); $s.Speak($env:DN_TTS_TEXT); $s.Dispose(); $b = $ms.ToArray(); $o = [Console]::OpenStandardOutput(); $o.Write($b, 0, $b.Length); $o.Flush()",
            hint
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .env("DN_TTS_TEXT", spoken)
            .creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output()?;
        if out.status.success() && out.stdout.len() > 44 {
            Ok(out.stdout)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "SAPI5 synthesis failed",
            ))
        }
    }

    fn synthesize_offline_wav(&self, spoken: &str, mood: dsp::VoiceMood) -> Result<Vec<u8>> {
        let raw_wav = if let Some(ref synth) = self.synthesizer {
            let prosody = dsp::prosody_for_mood(mood);
            let lang = self.offline_voice_lang();
            let ssml = format!(
                "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><prosody rate='{}' pitch='{}'>{}</prosody></speak>",
                escape_xml_text(&lang),
                prosody.winrt_rate,
                prosody.winrt_pitch,
                escape_xml_text(spoken)
            );
            let stream = match synth
                .SynthesizeSsmlToStreamAsync(&HSTRING::from(ssml.as_str()))
                .and_then(|op| op.get())
            {
                Ok(s) => s,
                Err(_) => synth
                    .SynthesizeTextToStreamAsync(&HSTRING::from(spoken))?
                    .get()?,
            };
            read_speech_stream_bytes(stream)?
        } else {
            Self::synthesize_sapi_fallback(spoken, true).map_err(|e| {
                windows::core::Error::new(windows::core::HRESULT(-2147467259i32), e.to_string())
            })?
        };

        if let Some((samples, sr)) = dsp::decode_wav_mono_f64(&raw_wav) {
            let polished = dsp::polish_mono_to_stereo_wav(&samples, sr, mood);
            if !polished.is_empty() {
                return Ok(polished);
            }
        }
        Ok(raw_wav)
    }

    fn synthesize_with_fallback(
        &self,
        spoken: &str,
        mood: dsp::VoiceMood,
        bypass_cooldown: bool,
    ) -> Result<Vec<u8>> {
        let edge_short_name = self
            .active_voice
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|v| edge_tts::edge_short_name_from_id(&v.id)));

        if let Some(short_name) = edge_short_name {
            let now_ms = now_unix_ms() as u64;
            let cooldown_until = self.edge_cooldown_until_ms.load(Ordering::Relaxed);
            let in_cooldown = !bypass_cooldown && now_ms < cooldown_until;

            if !in_cooldown {
                match edge_tts::synthesize_edge_pcm(spoken, short_name, mood) {
                    Ok((samples, sr)) => {
                        let polished = dsp::polish_mono_to_stereo_wav(&samples, sr, mood);
                        if !polished.is_empty() {
                            self.edge_fail_streak.store(0, Ordering::Relaxed);
                            self.edge_cooldown_until_ms.store(0, Ordering::Relaxed);
                            self.last_fallback_offline.store(false, Ordering::Relaxed);
                            return Ok(polished);
                        }
                        crate::backend::logfile::append(
                            "[tts] Edge neural polished output empty, falling back offline",
                        );
                    }
                    Err(e) => {
                        let streak = self.edge_fail_streak.fetch_add(1, Ordering::Relaxed) + 1;
                        crate::backend::logfile::append(&format!(
                            "[tts] Edge neural fallback -> offline (streak={}): {}",
                            streak, e
                        ));
                        // Only enter a brief 6s cooldown after 2+ consecutive full-retry failures
                        // when network looks down/unstable; never punish a single transient hiccup.
                        if should_enter_edge_cooldown(streak)
                            && crate::backend::system::internet_connected() != Some(false)
                        {
                            self.edge_cooldown_until_ms
                                .store(now_ms.saturating_add(6_000), Ordering::Relaxed);
                        }
                    }
                }
            }
            self.last_fallback_offline.store(true, Ordering::Relaxed);
            return self.synthesize_offline_wav(spoken, mood);
        }

        self.last_fallback_offline.store(false, Ordering::Relaxed);
        self.synthesize_offline_wav(spoken, mood)
    }

    pub fn speak(&self, text: &str) -> Result<u64> {
        self.speak_with_options(text, false)
    }

    pub fn speak_with_options(&self, text: &str, bypass_cooldown: bool) -> Result<u64> {
        let mood = dsp::detect_voice_mood(text);
        let spoken = sanitize_for_tts(text);
        if spoken.is_empty() {
            return Ok(0);
        }

        let my_gen = self.speak_gen.fetch_add(1, Ordering::SeqCst) + 1;

        let use_block_in_place = matches!(
            tokio::runtime::Handle::try_current(),
            Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        );
        let buffer = if use_block_in_place {
            tokio::task::block_in_place(|| {
                self.synthesize_with_fallback(&spoken, mood, bypass_cooldown)
            })?
        } else {
            self.synthesize_with_fallback(&spoken, mood, bypass_cooldown)?
        };

        // Drop result cleanly if `stop()` or a newer `speak()` superseded this request
        if self.speak_gen.load(Ordering::SeqCst) != my_gen {
            return Ok(0);
        }

        let size = buffer.len();
        self.stop_playback_only();

        let byte_rate = if buffer.len() >= 32 {
            let br = u32::from_le_bytes([buffer[28], buffer[29], buffer[30], buffer[31]]);
            if br > 1000 { br as f64 } else { 32000.0 }
        } else {
            32000.0
        };
        let duration_ms = if size > 44 {
            (((size - 44) as f64 / byte_rate) * 1000.0).round() as u64
        } else {
            (spoken.len() as u64 * 65 + 600).max(800)
        };

        let until = (now_unix_ms() as u64).saturating_add(duration_ms);
        self.speaking_until_ms.store(until, Ordering::SeqCst);

        let mut guard = self.current_buffer.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(buffer);
        if let Some(ref buf) = *guard {
            unsafe {
                PlaySoundW(
                    buf.as_ptr() as *const u16,
                    std::ptr::null_mut(),
                    SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
                );
            }
        }
        Ok(duration_ms)
    }

    pub fn is_speaking(&self) -> bool {
        let until = self.speaking_until_ms.load(Ordering::Relaxed);
        (now_unix_ms() as u64) < until
    }

    fn stop_playback_only(&self) {
        self.speaking_until_ms.store(0, Ordering::SeqCst);
        unsafe {
            PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
        }
        if let Ok(mut guard) = self.current_buffer.lock() {
            *guard = None;
        }
    }

    pub fn stop(&self) {
        self.speak_gen.fetch_add(1, Ordering::SeqCst);
        self.stop_playback_only();
    }

    pub fn release_if_idle(&self) {
        if !self.is_speaking() {
            if let Ok(mut guard) = self.current_buffer.lock() {
                *guard = None;
            }
        }
    }
}

unsafe impl Send for WinRtTts {}
unsafe impl Sync for WinRtTts {}

#[cfg(test)]
mod tests {
    use super::{
        pick_best_offline_voice_index, pick_best_voice_index, sanitize_for_tts, VoiceInfo,
    };
    use crate::voice::edge_tts::{builtin_edge_voices, EDGE_VOICE_ARDI_ID, EDGE_VOICE_GADIS_ID};

    #[test]
    fn sanitize_strips_emoji_markdown_urls_and_paths() {
        let out = sanitize_for_tts(
            "### Halo! 🎉😊 **Volume** diatur ke 50% ✓\n• Cek `C:\\Users\\Demo\\Downloads\\CV_Final.pdf`\n• Buka [Gemini](https://gemini.google.com) https://example.com/foo 🔥",
        );
        assert!(!out.contains('🎉'));
        assert!(!out.contains('😊'));
        assert!(!out.contains('🔥'));
        assert!(!out.contains('✓'));
        assert!(!out.contains("**"));
        assert!(!out.contains("https://"));
        assert!(!out.contains("C:\\Users\\Demo"));
        assert!(out.contains("Volume diatur ke 50%"));
        assert!(out.contains("CV_Final.pdf"));
        assert!(out.contains("Gemini"));
    }

    #[test]
    fn sanitize_empty_symbols_returns_empty() {
        assert_eq!(sanitize_for_tts("🔥 🎉 ✓ ×"), "");
    }

    #[test]
    fn auto_voice_selection_prefers_indonesian_then_system_default() {
        let voices_with_id = vec![
            VoiceInfo {
                id: "en1".into(),
                name: "Microsoft Zira".into(),
                language: "en-US".into(),
            },
            VoiceInfo {
                id: "id1".into(),
                name: "Microsoft Andika".into(),
                language: "id-ID".into(),
            },
        ];
        assert_eq!(
            pick_best_voice_index(&voices_with_id, "auto", Some("en1")),
            Some(1)
        );
        assert_eq!(
            pick_best_voice_index(&voices_with_id, "en1", Some("id1")),
            Some(0)
        );

        let voices_no_id = vec![
            VoiceInfo {
                id: "jp1".into(),
                name: "Microsoft Haruka".into(),
                language: "ja-JP".into(),
            },
            VoiceInfo {
                id: "en1".into(),
                name: "Microsoft David".into(),
                language: "en-US".into(),
            },
        ];
        assert_eq!(
            pick_best_voice_index(&voices_no_id, "auto", Some("en1")),
            Some(1)
        );
        assert_eq!(pick_best_voice_index(&voices_no_id, "auto", None), Some(0));
        assert_eq!(pick_best_voice_index(&[], "auto", None), None);
    }

    #[test]
    fn offline_fallback_skips_edge_voices_and_prefers_local_indonesian() {
        let mut voices = builtin_edge_voices();
        voices.push(VoiceInfo {
            id: "en1".into(),
            name: "Microsoft David".into(),
            language: "en-US".into(),
        });
        voices.push(VoiceInfo {
            id: "id1".into(),
            name: "Microsoft Andika".into(),
            language: "id-ID".into(),
        });

        // Auto chooses Gadis Neural first
        assert_eq!(
            pick_best_voice_index(&voices, "auto", Some("en1")),
            Some(0)
        );
        assert_eq!(voices[0].id, EDGE_VOICE_GADIS_ID);
        assert_eq!(voices[1].id, EDGE_VOICE_ARDI_ID);

        // Offline fallback skips both Edge voices and chooses Microsoft Andika
        assert_eq!(
            pick_best_offline_voice_index(&voices, Some("en1")),
            Some(3)
        );

        // When only Edge voices exist, offline helper returns None cleanly
        assert_eq!(
            pick_best_offline_voice_index(&builtin_edge_voices(), None),
            None
        );
    }

    #[test]
    fn edge_cooldown_ignores_single_transient_glitch() {
        assert!(!super::should_enter_edge_cooldown(0));
        assert!(!super::should_enter_edge_cooldown(1));
        assert!(super::should_enter_edge_cooldown(2));
        assert!(super::should_enter_edge_cooldown(5));
    }
}
