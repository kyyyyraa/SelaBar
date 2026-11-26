use regex::Regex;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Resolves local `whisper-cuda/`, `whisper-cpu/`, `models/`, and `desknotch-models/` folders
/// relative to the binary, AppData (`SelaBar` or legacy `DeskNotchNative`), runtime env override,
/// or dev cargo workspace.
pub fn runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let push_uniq = |list: &mut Vec<PathBuf>, p: PathBuf| {
        if !p.as_os_str().is_empty() && !list.contains(&p) {
            list.push(p);
        }
    };
    for env_key in ["SELABAR_RUNTIME_DIR", "DESKNOTCH_RUNTIME_DIR"] {
        if let Ok(custom) = std::env::var(env_key) {
            if !custom.trim().is_empty() {
                push_uniq(&mut roots, PathBuf::from(custom));
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            push_uniq(&mut roots, exe_dir.to_path_buf());
            if let Some(p1) = exe_dir.parent() {
                push_uniq(&mut roots, p1.to_path_buf());
                if let Some(p2) = p1.parent() {
                    push_uniq(&mut roots, p2.to_path_buf());
                }
            }
        }
    }
    for app_folder in ["SelaBar", "DeskNotchNative"] {
        for var in ["APPDATA", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                if !base.trim().is_empty() {
                    push_uniq(&mut roots, PathBuf::from(base).join(app_folder));
                }
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        push_uniq(&mut roots, PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    }
    roots
}

fn cli_path_in_roots(roots: &[PathBuf], folder: &str) -> Option<PathBuf> {
    for root in roots {
        let dir = root.join(folder);
        for name in ["whisper-cli.exe", "main.exe"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn model_path_in_roots(roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        for cand in [
            root.join("models").join("ggml-small.bin"),
            root.join("desknotch-models").join("ggml-small.bin"),
            root.join("models").join("ggml-small-q5_1.bin"),
            root.join("desknotch-models").join("ggml-small-q5_1.bin"),
            root.join("models").join("ggml-base.bin"),
            root.join("desknotch-models").join("ggml-base.bin"),
            root.join("whisper-cuda").join("ggml-small.bin"),
            root.join("whisper-cpu").join("ggml-small.bin"),
        ] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

pub fn cuda_cli_path() -> Option<PathBuf> {
    cli_path_in_roots(&runtime_roots(), "whisper-cuda")
}

pub fn cpu_cli_path() -> Option<PathBuf> {
    cli_path_in_roots(&runtime_roots(), "whisper-cpu")
}

pub fn has_cuda_cli() -> bool {
    cuda_cli_path().is_some()
}

pub fn has_cpu_cli() -> bool {
    cpu_cli_path().is_some()
}

pub fn has_model() -> bool {
    model_path().is_some()
}

pub fn cli_path() -> Option<PathBuf> {
    let roots = runtime_roots();
    cli_path_in_roots(&roots, "whisper-cuda").or_else(|| cli_path_in_roots(&roots, "whisper-cpu"))
}

pub fn model_path() -> Option<PathBuf> {
    model_path_in_roots(&runtime_roots())
}

pub fn active_backend_label() -> Option<String> {
    let cli = cli_path()?;
    let model = model_path()?;
    let is_cuda = cli
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_ascii_lowercase().contains("cuda"))
        .unwrap_or(false);
    let backend = if is_cuda { "CUDA" } else { "CPU" };
    let model_name = model
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "model.bin".to_string());
    Some(format!("{} • {}", backend, model_name))
}

pub fn is_available() -> bool {
    cli_path().is_some() && model_path().is_some()
}

pub fn f32_to_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&32000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav
}

fn run_whisper_cli(
    cli: &std::path::Path,
    model: &std::path::Path,
    lang: &str,
    wav: &std::path::Path,
    no_gpu: bool,
) -> std::io::Result<std::process::Output> {
    let mut cmd = std::process::Command::new(cli);
    if let Some(dir) = cli.parent() {
        cmd.current_dir(dir);
        let cur_path = std::env::var("PATH").unwrap_or_default();
        let dir_s = dir.to_string_lossy();
        let next_path = if cur_path.is_empty() {
            dir_s.to_string()
        } else {
            format!("{};{}", dir_s, cur_path)
        };
        cmd.env("PATH", next_path);
    }
    let mut args = vec![
        "-m".to_string(),
        model.to_string_lossy().to_string(),
        "-l".to_string(),
        lang.to_string(),
        "-nt".to_string(),
        "-np".to_string(),
        "-sns".to_string(),
    ];
    if no_gpu {
        args.push("-ng".to_string());
    }
    args.push("-f".to_string());
    args.push(wav.to_string_lossy().to_string());
    cmd.args(&args);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.output()
}

pub fn transcribe_samples_sync(samples: &[f32], lang: &str) -> Result<(String, u64), String> {
    if samples.is_empty() {
        return Ok((String::new(), 0));
    }
    let cli = cli_path().ok_or("whisper-cli.exe tidak ditemukan")?;
    let model = model_path().ok_or("Model Whisper tidak ditemukan")?;
    let t0 = Instant::now();

    let tmp = std::env::temp_dir().join(format!(
        "selabar-stt-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 + d.as_secs() * 1_000_000_000)
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, f32_to_wav(samples)).map_err(|e| format!("wav write: {}", e))?;

    let mut out = run_whisper_cli(&cli, &model, lang, &tmp, false);
    if let Ok(ref o) = out {
        if !o.status.success() {
            // Automatic CPU fallback (-ng) if CUDA driver/VRAM fails
            out = run_whisper_cli(&cli, &model, lang, &tmp, true);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    match out {
        Ok(o) => {
            if !o.status.success() {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return Err(format!(
                    "whisper-cli exit {:?}: {}",
                    o.status.code(),
                    if err.is_empty() { "gagal" } else { &err }
                ));
            }
            static NST_RE: OnceLock<Regex> = OnceLock::new();
            let nst_re = NST_RE.get_or_init(|| {
                Regex::new(r"(?i)(?:\[(?:musik|music|blank_audio|applause|laughter|noise|silence|hening|tepuk tangan)[^\]]*\]|\((?:musik|music|blank_audio|applause|laughter|noise|silence|hening)[^\)]*\)|\*[^\*]{1,24}\*)")
                    .unwrap()
            });
            let raw_text = String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| nst_re.replace_all(l, " ").trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let cleaned = apply_fixes(&clean_fillers(raw_text.trim()));
            Ok((cleaned, t0.elapsed().as_millis() as u64))
        }
        Err(e) => Err(format!("whisper-cli gagal jalan: {}", e)),
    }
}

pub async fn transcribe_samples(samples: Vec<f32>, lang: String) -> Result<(String, u64), String> {
    tokio::task::spawn_blocking(move || transcribe_samples_sync(&samples, &lang))
        .await
        .map_err(|e| format!("join: {}", e))?
}

const FIXES: &[(&str, &str)] = &[
    ("winda basuh darah", "windah basudara"),
    ("windah basuh darah", "windah basudara"),
    ("winda basuh udara", "windah basudara"),
    ("windah basuh udara", "windah basudara"),
    ("windah basudara", "windah basudara"),
    ("setelah gua balon puri", "setel balonku"),
    ("setelah gue balon puri", "setel balonku"),
    ("winda basudara", "windah basudara"),
    ("bukatan apitasi", "buka aplikasi"),
    ("di whatsappnya", "di whatsapp"),
    ("langguh india", "hindia"),
    ("basuh darah", "basudara"),
    ("basuh udara", "basudara"),
    ("basu udara", "basudara"),
    ("to due list", "to do list"),
    ("di whats up", "di whatsapp"),
    ("tu du list", "to do list"),
    ("tu duilis", "to do list"),
    ("tuh dulis", "to do list"),
    ("tu dulist", "to do list"),
    ("to do lis", "to do list"),
    ("aplikasion", "aplikasi"),
    ("bu ka trom", "buka chrome"),
    ("terkenjang", "terkencang"),
    ("balon puri", "balonku"),
    ("bupa prom", "buka chrome"),
    ("dikteknya", "diktenya"),
    ("remainder", "reminder"),
    ("spaceteam", "spotify"),
    ("terkenjan", "terkencang"),
    ("keresiden", "presiden"),
    ("watch up", "whatsapp"),
    ("balon ku", "balonku"),
    ("putarkan", "putar"),
    ("note pad", "notepad"),
    ("buat kan", "buatkan"),
    ("spotipai", "spotify"),
    ("spotifai", "spotify"),
    ("exploler", "explorer"),
    ("tu duis", "to do list"),
    ("tudulis", "to do list"),
    ("wats up", "whatsapp"),
    ("wat sap", "whatsapp"),
    ("whatsap", "whatsapp"),
    ("noteped", "notepad"),
    ("putarin", "putar"),
    ("langguh", "hindia"),
    ("meyerek", "merek"),
    ("apitasi", "aplikasi"),
    ("schedul", "jadwal"),
    ("meeting", "meeting"),
    ("vs code", "vscode"),
    ("buatkan", "buatkan"),
    ("balonku", "balonku"),
    ("spotify", "spotify"),
    ("whatsapp", "whatsapp"),
    ("explorer", "explorer"),
    ("youtube", "youtube"),
    ("toylet", "toilet"),
    ("toilet", "toilet"),
    ("meting", "meeting"),
    ("mitink", "meeting"),
    ("jadual", "jadwal"),
    ("watsap", "whatsapp"),
    ("yutube", "youtube"),
    ("ngepad", "notepad"),
    ("nottet", "notepad"),
    ("nattet", "notepad"),
    ("diktek", "dikte"),
    ("bukein", "buka"),
    ("di war", "di Word"),
    ("z-code", "ZCode"),
    ("z code", "ZCode"),
    ("google", "google"),
    ("kroom", "chrome"),
    ("crome", "chrome"),
    ("yutub", "youtube"),
    ("gugel", "google"),
    ("gogle", "google"),
    ("vscod", "vscode"),
    ("ekcel", "excel"),
    ("exsel", "excel"),
    ("excel", "excel"),
    ("banti", "bantu"),
    ("tu du", "to do"),
    ("tual", "jadwal"),
    ("krom", "chrome"),
    ("lago", "lagu"),
    ("apk", "aplikasi"),
];

pub fn apply_fixes(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    static FIX_RE: OnceLock<(Regex, Vec<(&'static str, &'static str)>)> = OnceLock::new();
    let (re, sorted) = FIX_RE.get_or_init(|| {
        let mut pairs: Vec<(&'static str, &'static str)> = FIXES.to_vec();
        pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        pairs.dedup_by(|a, b| a.0 == b.0);
        let alts: Vec<String> = pairs.iter().map(|(k, _)| regex::escape(k)).collect();
        let pat = format!(r"(?i)\b(?:{})\b", alts.join("|"));
        (Regex::new(&pat).unwrap(), pairs)
    });

    re.replace_all(text, |caps: &regex::Captures| {
        let m = &caps[0];
        let low = m.to_lowercase();
        if let Some(&(_, fixed)) = sorted.iter().find(|(k, _)| *k == low) {
            let is_upper = m
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false);
            if is_upper {
                let mut chs = fixed.chars();
                match chs.next() {
                    Some(first) => first.to_uppercase().to_string() + chs.as_str(),
                    None => fixed.to_string(),
                }
            } else {
                fixed.to_string()
            }
        } else {
            m.to_string()
        }
    })
    .to_string()
}

pub fn clean_fillers(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    static FILLER_RE: OnceLock<Regex> = OnceLock::new();
    static PHRASE_RE: OnceLock<Regex> = OnceLock::new();
    static YA_RE: OnceLock<Regex> = OnceLock::new();
    static MULTI_SPACE_RE: OnceLock<Regex> = OnceLock::new();
    static SPACE_PUNCT_RE: OnceLock<Regex> = OnceLock::new();

    let filler_re = FILLER_RE.get_or_init(|| {
        Regex::new(r"(?i)[,;]?\s*\b(?:u+h*m+|u+h+|e+r+m*|e+m{2,}|a+h+|h+m+|m+h+m+|m{2,}|e{2,}|a{3,})\b\s*[,;]?")
            .unwrap()
    });
    let phrase_re = PHRASE_RE.get_or_init(|| {
        Regex::new(r"(?i)[,;]?\s*\b(?:apa\s+namanya|gimana\s+ya(?:h)?)\b\s*[,;]?\s*").unwrap()
    });
    let ya_re = YA_RE.get_or_init(|| Regex::new(r"(?i)\s+\b(?:ya|yah)\b\s+").unwrap());
    let multi_sp = MULTI_SPACE_RE.get_or_init(|| Regex::new(r"\s{2,}").unwrap());
    let sp_punct = SPACE_PUNCT_RE.get_or_init(|| Regex::new(r"\s+([,.!?])").unwrap());

    let mut out = filler_re
        .replace_all(text, |caps: &regex::Captures| {
            let m = &caps[0];
            if m.contains(',') || m.contains(';') {
                ", "
            } else {
                " "
            }
        })
        .to_string();

    // Only strip Indonesian hesitation phrases when >= 2 words remain after them
    if let Some(m) = phrase_re.find(&out) {
        let after = out[m.end()..].trim();
        if after.split_whitespace().count() >= 2 {
            out = phrase_re.replace_all(&out, " ").to_string();
        }
    }

    // Mid-sentence "ya"/"yah" before non-whitespace
    loop {
        let next = ya_re.replace(&out, " ").to_string();
        if next == out {
            break;
        }
        out = next;
    }

    out = multi_sp.replace_all(&out, " ").to_string();
    out = sp_punct.replace_all(&out, "$1").to_string();
    out = out.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == ';').trim().to_string();

    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_whisper_cuda_and_model_locally() {
        assert!(
            cli_path().is_some(),
            "whisper-cli.exe should be found in local whisper-cuda folder"
        );
        assert!(
            model_path().is_some(),
            "ggml-small.bin should be found in local desknotch-models folder"
        );
        assert!(is_available());
    }

    #[test]
    fn cleans_fillers_and_applies_domain_fixes() {
        assert_eq!(clean_fillers("hmm buka chrome"), "Buka chrome");
        assert_eq!(apply_fixes("Buka ngepad dan spotipai"), "Buka notepad dan spotify");
        assert_eq!(apply_fixes("buatkan tu duis"), "buatkan to do list");
    }

    #[test]
    fn resolves_selabar_q5_and_cpu_layout_in_custom_roots() {
        let base = std::env::temp_dir().join(format!("selabar-whisper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("whisper-cpu")).unwrap();
        std::fs::create_dir_all(base.join("models")).unwrap();
        std::fs::write(base.join("whisper-cpu").join("whisper-cli.exe"), b"stub").unwrap();
        std::fs::write(base.join("models").join("ggml-small-q5_1.bin"), b"stub").unwrap();

        let roots = vec![base.clone()];
        assert!(cli_path_in_roots(&roots, "whisper-cpu").is_some());
        assert!(cli_path_in_roots(&roots, "whisper-cuda").is_none());
        let resolved_model = model_path_in_roots(&roots).expect("q5_1 model should resolve");
        assert_eq!(
            resolved_model.file_name().and_then(|s| s.to_str()),
            Some("ggml-small-q5_1.bin")
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
