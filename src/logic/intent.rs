use chrono::{Duration as ChronoDur, Local, TimeZone};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub enum TimeRef {
    Relative { ms: u64 },
    Absolute { ts_ms: i64, ambiguous: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Theme { mode: String, label: String },
    Time,
    DiagnoseSystem,
    TodoList,
    TodoAdd { text: String },
    Timer { ms: u64 },
    ReminderAdd { text: String, at: TimeRef },
    Volume { level: Option<i64>, delta: Option<i64> },
    Media { action: String },
    YoutubePlay { query: String, newest: bool },
    Youtube { query: String },
    Whatsapp { contact: String, message: String },
    OpenTab { site: String, label: String, url: Option<String> },
    SearchFile { query: String, hint: Option<String> },
    OpenApp { app: String, label: String },
    Unknown { raw: String },
}

#[derive(Debug, Clone)]
pub enum ParsedVoiceAction {
    Single(Intent),
    Chain { first: Intent, rest: String },
}

pub struct AppDef {
    pub key: &'static str,
    pub label: &'static str,
    pub aliases: &'static [&'static str],
}

pub const APPS: &[AppDef] = &[
    AppDef { key: "chrome", label: "Chrome", aliases: &["chrome", "google chrome", "google", "krom", "krome", "crome"] },
    AppDef { key: "edge", label: "Edge", aliases: &["edge", "microsoft edge", "ms edge", "bing"] },
    AppDef { key: "firefox", label: "Firefox", aliases: &["firefox", "mozilla firefox", "mozilla", "mozila"] },
    AppDef { key: "notepad", label: "Notepad", aliases: &["notepad", "note pad", "noteped", "ngepad", "nottet", "nattet"] },
    AppDef { key: "word", label: "Word", aliases: &["word", "ms word", "microsoft word"] },
    AppDef { key: "excel", label: "Excel", aliases: &["excel", "ms excel", "microsoft excel", "ekcel", "exsel"] },
    AppDef { key: "powerpoint", label: "PowerPoint", aliases: &["powerpoint", "power point", "ppt", "microsoft powerpoint"] },
    AppDef { key: "spotify", label: "Spotify", aliases: &["spotify", "spotifai", "spotipai", "spotifay"] },
    AppDef { key: "youtube", label: "YouTube", aliases: &["youtube", "you tube", "yutub", "yutube", "yt"] },
    AppDef { key: "whatsapp", label: "WhatsApp", aliases: &["whatsapp", "whatsap", "watsap", "wa"] },
    AppDef { key: "telegram", label: "Telegram", aliases: &["telegram", "tele gram", "tg"] },
    AppDef { key: "discord", label: "Discord", aliases: &["discord", "diskor"] },
    AppDef { key: "steam", label: "Steam", aliases: &["steam", "stim"] },
    AppDef { key: "vscode", label: "VS Code", aliases: &["vscode", "vs code", "visual studio code", "vscod"] },
    AppDef { key: "explorer", label: "File Explorer", aliases: &["explorer", "file explorer", "windows explorer", "eksplorer", "exploler"] },
    AppDef { key: "kalkulator", label: "Kalkulator", aliases: &["kalkulator", "calculator", "kalku", "calc"] },
    AppDef { key: "pengaturan", label: "Pengaturan", aliases: &["pengaturan", "setting", "settings", "setelan"] },
    AppDef { key: "taskmgr", label: "Task Manager", aliases: &["task manager", "taskmgr", "task mgr", "manajer tugas"] },
    AppDef { key: "paint", label: "Paint", aliases: &["paint", "ms paint"] },
    AppDef { key: "canva", label: "Canva", aliases: &["canva", "kanva", "canvas"] },
    AppDef { key: "figma", label: "Figma", aliases: &["figma"] },
    AppDef { key: "notion", label: "Notion", aliases: &["notion", "nosion"] },
    AppDef { key: "obsidian", label: "Obsidian", aliases: &["obsidian", "obsi"] },
    AppDef { key: "zoom", label: "Zoom", aliases: &["zoom", "zoom meeting", "zum"] },
    AppDef { key: "teams", label: "Teams", aliases: &["teams", "ms teams", "microsoft teams"] },
    AppDef { key: "gmail", label: "Gmail", aliases: &["gmail", "g mail", "google mail"] },
    AppDef { key: "github", label: "GitHub", aliases: &["github", "git hub", "git"] },
];

pub fn web_url_for(site: &str) -> Option<&'static str> {
    match site.trim().to_lowercase().as_str() {
        "youtube" | "yt" => Some("https://www.youtube.com"),
        "whatsapp" | "wa" | "wa web" | "whatsapp web" => Some("https://web.whatsapp.com"),
        "gmail" => Some("https://mail.google.com"),
        "github" => Some("https://github.com"),
        "telegram" => Some("https://web.telegram.org"),
        "instagram" => Some("https://www.instagram.com"),
        "twitter" => Some("https://x.com"),
        "maps" => Some("https://maps.google.com"),
        "drive" => Some("https://drive.google.com"),
        "calendar" => Some("https://calendar.google.com"),
        "meet" | "gmeet" | "google meet" => Some("https://meet.google.com"),
        "zoom" => Some("https://zoom.us/join"),
        "teams" | "ms teams" => Some("https://teams.microsoft.com"),
        "chatgpt" => Some("https://chatgpt.com"),
        "claude" => Some("https://claude.ai"),
        _ => None,
    }
}

pub fn app_search_terms(key: &str) -> Vec<String> {
    let k = key.trim().to_lowercase();
    let mut terms: Vec<String> = match k.as_str() {
        "chrome" => vec!["google chrome", "chrome"],
        "edge" => vec!["microsoft edge", "edge"],
        "firefox" => vec!["firefox", "mozilla firefox"],
        "notepad" => vec!["notepad"],
        "word" => vec!["word", "microsoft word"],
        "excel" => vec!["excel", "microsoft excel"],
        "powerpoint" => vec!["powerpoint", "microsoft powerpoint"],
        "spotify" => vec!["spotify"],
        "youtube" => vec!["youtube"],
        "whatsapp" => vec!["whatsapp"],
        "telegram" => vec!["telegram"],
        "discord" => vec!["discord"],
        "steam" => vec!["steam"],
        "vscode" => vec!["visual studio code", "vscode", "code"],
        "explorer" => vec!["file explorer", "explorer"],
        "kalkulator" => vec!["calculator", "kalkulator"],
        "pengaturan" => vec!["settings", "pengaturan"],
        "taskmgr" => vec!["task manager"],
        "paint" => vec!["paint"],
        "canva" => vec!["canva"],
        "figma" => vec!["figma"],
        "notion" => vec!["notion"],
        "obsidian" => vec!["obsidian"],
        "zoom" => vec!["zoom"],
        "teams" => vec!["microsoft teams", "teams"],
        "gmail" => vec!["gmail"],
        "github" => vec!["github"],
        "zcode" => vec!["zcode", "z code", "dev.zcode.app"],
        "amd" => vec!["amd", "amd software", "radeon"],
        _ => vec![],
    }
    .into_iter()
    .map(str::to_string)
    .collect();
    if terms.is_empty() && !k.is_empty() {
        terms.push(k);
    }
    terms
}

#[derive(Clone)]
struct Tok {
    raw: String,
    low: String,
}

fn tokenize(input: &str) -> Vec<Tok> {
    let mut cleaned = String::with_capacity(input.len());
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            cleaned.push(c);
        } else {
            cleaned.push(' ');
        }
    }
    cleaned
        .split_whitespace()
        .map(|s| Tok {
            raw: s.to_string(),
            low: s.to_lowercase(),
        })
        .collect()
}

fn lev(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev.copy_from_slice(&cur);
    }
    prev[b.len()]
}

fn fuzzy(word: &str, target: &str, max_dist: usize) -> Option<usize> {
    if word == target {
        return Some(0);
    }
    let wl = word.chars().count();
    let tl = target.chars().count();
    if wl < 4 || tl < 4 {
        return None;
    }
    let mut cap = max_dist;
    if wl.min(tl) < 6 && cap > 1 {
        cap = 1;
    }
    if wl.abs_diff(tl) > cap {
        return None;
    }
    let d = lev(word, target);
    if d <= cap { Some(d) } else { None }
}

const APP_SKIP: &[&str] = &[
    "aplikasi", "aplikasinya", "app", "program", "yang", "namanya", "tolong", "coba", "ya", "dong",
    "deh", "saya", "aku", "gue", "gua", "gw", "sekarang", "dulu", "aja", "itu", "ini", "please",
    "prom", "prompt", "promp",
];

const PRONOUNS: &[&str] = &["saya", "aku", "gue", "gua", "gw"];
const WANT: &[&str] = &["mau", "ingin", "pengen", "pingin", "pengin", "hendak"];
const LEAD_FILLER: &[&str] = &[
    "tolong", "mohon", "minta", "coba", "eh", "halo", "hai", "hallo", "ya", "yah", "dong", "deh",
    "nih", "sih", "aja", "saja", "bantu", "bantuin", "segera", "please", "pls", "kok", "kan",
    "lah",
];
const ADD_VERBS: &[&str] = &[
    "buat", "buatkan", "bikin", "bikinin", "tambah", "tambahkan", "tambahin", "catat", "catatkan",
    "catet", "tulis", "tuliskan", "tulisin", "masukkan", "masukan", "masukin", "simpan",
    "simpanin", "daftarkan", "input", "inputkan", "note", "add",
];
const ADD_VERBS_STRONG: &[&str] = &[
    "buatkan", "catat", "catatkan", "catet", "tambahkan", "tambahin", "tambah", "tulis",
    "tuliskan", "tulisin", "masukkan", "masukan", "masukin", "simpan", "simpanin", "daftarkan",
    "input", "inputkan",
];
const PREPOSITIONS: &[&str] = &["ke", "di", "dalam", "untuk", "pada"];
const STRONG_CUES: &[&str] = &["apa", "apakah", "aja", "saja", "mana", "gimana", "bagaimana"];
const WEAK_CUES: &[&str] = &["lihat", "liat", "tampilkan", "tampil", "baca", "cek", "sebutkan", "show"];
const OPEN_VERBS: &[&str] = &[
    "buka", "bukain", "bukakan", "bukak", "jalanin", "jalankan", "launch", "open", "start",
    "nyalakan",
];
const SEARCH_VERBS: &[&str] = &["cari", "carikan", "cariin", "temukan", "temuin", "find", "search"];
const REMINDER_VERBS: &[&str] = &[
    "ingatkan", "ingetin", "ingat", "inget", "reminder", "pengingat", "alarm", "jadwalkan",
    "jadwalin",
];
const FILE_CUES: &[&str] = &["file", "dokumen", "document", "folder", "berkas"];
const EXTS: &[&str] = &[
    "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "jpg", "jpeg", "png", "txt", "mp4",
    "mp3", "zip", "csv",
];
const FILE_TOPIC: &[&str] = &[
    "file", "dokumen", "document", "folder", "berkas", "download", "downloads", "desktop", "pdf",
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "txt", "csv", "cv", "visa", "surat", "laporan",
    "invoice", "resume",
];
const QUERY_NOISE: &[&str] = &[
    "saya", "aku", "gue", "gua", "gw", "ku", "punya", "milik", "yang", "itu", "ini", "si", "file",
    "dokumen", "document", "folder", "berkas",
    // Filler/question words that are never part of a file name:
    // "cari file DENGAN KATA cv ADA APA SAJA" -> query must be just "cv".
    "dengan", "kata", "berkata", "bernama", "namanya", "nama", "ada", "apa", "apakah", "saja",
    "aja", "mana", "berapa", "banyak", "semua", "list", "daftar", "tolong", "coba", "dong",
    "deh", "nih", "sih", "ya", "yah", "untuk", "buat", "biar", "supaya", "agar", "dan", "lalu",
    "terus", "kemudian", "atau", "serta", "juga", "lagi", "dulu", "sekarang", "nanti", "besok",
    "lusa", "hari", "ini", "malam", "siang", "sore", "pagi",
];
const VERB_SKIP: &[&str] = &[
    "tadi", "tapi", "kata", "juga", "belum", "lagi", "nanti", "sekarang", "untuk", "dari", "kami",
    "kita", "kamu", "bisa", "harus", "ingin", "sama", "sana", "sini", "kapan", "kenapa", "siapa",
    "mana", "apa", "jam", "menit", "detik", "persen", "tolong", "coba", "yang", "dan", "dengan",
    "tentang", "kalau", "agar", "supaya", "sedang", "sudah", "dunia", "perang", "kabar",
];
const NONQUERY: &[&str] = &[
    "dulu", "dong", "deh", "sih", "aja", "saja", "ya", "yah", "nih", "kok", "lah", "lagi",
    "nanti", "sekarang", "sana", "situ", "gitu", "yang", "apa", "paling", "baru", "terbaru",
];
const TAB_SITE_BLOCK: &[&str] = &[
    "tab", "browser", "file", "dokumen", "pintu", "jendela", "aplikasi", "app",
];
const BROWSER_WORDS: &[&str] = &["chrome", "browser", "edge", "firefox", "brave", "opera"];

fn unit_ms(w: &str) -> u64 {
    match w {
        "detik" | "dtk" | "sekon" => 1_000,
        "menit" | "mnt" => 60_000,
        "jam" => 3_600_000,
        _ => 0,
    }
}

fn period_of(w: &str) -> Option<&'static str> {
    match w {
        "pagi" | "subuh" | "dini" => Some("am"),
        "siang" | "sore" | "petang" | "malam" | "malem" => Some("pm"),
        _ => None,
    }
}

fn file_hint(w: &str) -> Option<&'static str> {
    match w {
        "download" | "downloads" | "unduhan" => Some("download"),
        "dokumen" | "document" | "documents" => Some("documents"),
        "desktop" => Some("desktop"),
        _ => None,
    }
}

fn num_word(w: &str) -> Option<u32> {
    Some(match w {
        "nol" => 0,
        "satu" => 1,
        "dua" => 2,
        "tiga" => 3,
        "empat" => 4,
        "lima" => 5,
        "enam" => 6,
        "tujuh" => 7,
        "delapan" => 8,
        "sembilan" => 9,
        "sepuluh" => 10,
        "sebelas" => 11,
        "duabelas" => 12,
        _ => return None,
    })
}

fn clean_text(s: &str) -> String {
    let mut out = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let trim_chars: &[char] = &[' ', ',', ';', ':', '.', '!', '?', '-'];
    out = out.trim_matches(trim_chars).to_string();
    static TRAIL_RE: OnceLock<Regex> = OnceLock::new();
    let re = TRAIL_RE.get_or_init(|| {
        Regex::new(r"(?i)[\s,]+(?:ya|yah|dong|deh|nih|sih|aja|saja|donk|please|pls)$").unwrap()
    });
    for _ in 0..4 {
        let next = re.replace(&out, "").trim_end_matches(&[' ', ','][..]).to_string();
        if next == out {
            break;
        }
        out = next;
    }
    out.trim_matches(&[' ', ','][..]).to_string()
}

fn find_verb(low: &[String], verbs: &[&str], from: usize, window: usize) -> Option<(usize, String, usize)> {
    let end = if window > 0 {
        (from + window).min(low.len())
    } else {
        low.len()
    };
    for i in from..end {
        if let Some(&v) = verbs.iter().find(|&&v| v == low[i]) {
            return Some((i, v.to_string(), 0));
        }
    }
    for i in from..end {
        let mut best: Option<(usize, &str)> = None;
        for &v in verbs {
            if let Some(d) = fuzzy(&low[i], v, 2) {
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, v));
                }
            }
        }
        if let Some((d, v)) = best {
            return Some((i, v.to_string(), d));
        }
    }
    None
}

fn find_verb_guarded(low: &[String], verbs: &[&str], from: usize, window: usize) -> Option<(usize, String, usize)> {
    let hit = find_verb(low, verbs, from, window)?;
    if hit.2 > 0 && VERB_SKIP.contains(&low[hit.0].as_str()) {
        return None;
    }
    Some(hit)
}

fn find_app(low: &[String], from: usize) -> Option<(&'static str, &'static str, usize, usize, usize)> {
    let mut flat_aliases: Vec<(Vec<&str>, &AppDef)> = Vec::new();
    for app in APPS {
        for &al in app.aliases {
            flat_aliases.push((al.split_whitespace().collect(), app));
        }
    }
    flat_aliases.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for i in from..low.len() {
        if APP_SKIP.contains(&low[i].as_str()) {
            continue;
        }
        for (words, app) in &flat_aliases {
            if i + words.len() <= low.len()
                && (0..words.len()).all(|k| low[i + k] == words[k])
            {
                return Some((app.key, app.label, i, words.len(), 0));
            }
        }
    }

    for i in from..low.len() {
        let w = &low[i];
        if APP_SKIP.contains(&w.as_str()) || w.chars().count() < 4 {
            continue;
        }
        let mut best: Option<(&'static str, &'static str, usize)> = None;
        for (words, app) in &flat_aliases {
            if words.len() != 1 {
                continue;
            }
            if let Some(d) = fuzzy(w, words[0], 2) {
                if best.map(|b| d < b.2).unwrap_or(true) {
                    best = Some((app.key, app.label, d));
                }
            }
        }
        if let Some((k, l, d)) = best {
            return Some((k, l, i, 1, d));
        }
    }
    None
}

fn strip_lead(words: &[String]) -> &[String] {
    let mut i = 0;
    while i < words.len() && LEAD_FILLER.contains(&words[i].as_str()) {
        i += 1;
    }
    if i < words.len() && WANT.contains(&words[i].as_str()) {
        i += 1;
    }
    if i < words.len() && PRONOUNS.contains(&words[i].as_str()) {
        i += 1;
    }
    while i < words.len() && PREPOSITIONS.contains(&words[i].as_str()) {
        i += 1;
    }
    if i < words.len() && WANT.contains(&words[i].as_str()) {
        i += 1;
    }
    if i < words.len() && PRONOUNS.contains(&words[i].as_str()) {
        i += 1;
    }
    while i < words.len() && LEAD_FILLER.contains(&words[i].as_str()) {
        i += 1;
    }
    &words[i..]
}

fn finish_text(tokens: &[Tok], from: usize, to: usize, skip: &[bool]) -> String {
    let mut picks = Vec::new();
    for i in from..to.min(tokens.len()) {
        if skip.get(i).copied().unwrap_or(false) {
            continue;
        }
        picks.push(&tokens[i]);
    }
    let lows: Vec<String> = picks.iter().map(|t| t.low.clone()).collect();
    let keep = strip_lead(&lows);
    let start = picks.len().saturating_sub(keep.len());
    let raws: Vec<&str> = picks[start..].iter().map(|t| t.raw.as_str()).collect();
    clean_text(&raws.join(" "))
}

fn join_raw(tokens: &[Tok], from: usize, to: usize) -> String {
    let raws: Vec<&str> = tokens[from..to.min(tokens.len())]
        .iter()
        .map(|t| t.raw.as_str())
        .collect();
    clean_text(&raws.join(" "))
}

const TODO_NOUNS: &[(&[&str], bool)] = &[
    (&["to", "do", "list"], false),
    (&["todo", "list"], false),
    (&["to", "do"], false),
    (&["todolist"], false),
    (&["todos"], false),
    (&["todo"], false),
    (&["tudulis"], false),
    (&["tu", "duis"], false),
    (&["tu", "du"], false),
    (&["tu", "duh"], false),
    (&["tuh", "dulis"], false),
    (&["daftar", "tugas"], false),
    (&["daftar", "kerja"], false),
    (&["daftar", "todo"], false),
    (&["tugas"], false),
    (&["task"], false),
    (&["tasks"], false),
    (&["taks"], false),
    (&["todus"], false),
    (&["tuduh"], false),
    (&["list"], true),
    (&["lis"], true),
];

fn find_todo_noun(low: &[String]) -> Option<(usize, usize)> {
    for i in 0..low.len() {
        for &(words, needs_prep) in TODO_NOUNS {
            if i + words.len() > low.len() {
                continue;
            }
            if needs_prep {
                let prev = if i > 0 { low[i - 1].as_str() } else { "" };
                if !(PREPOSITIONS.contains(&prev)
                    || ADD_VERBS.contains(&prev)
                    || WEAK_CUES.contains(&prev))
                {
                    continue;
                }
            }
            if (0..words.len()).all(|k| low[i + k] == words[k]) {
                return Some((i, i + words.len()));
            }
        }
    }
    None
}

fn parse_duration(low: &[String]) -> Option<u64> {
    let mut total = 0u64;
    let mut found = false;
    let mut i = 0;
    while i < low.len() {
        if low[i] == "setengah" && low.get(i + 1).map(|s| s.as_str()) == Some("jam") {
            total += 30 * 60_000;
            found = true;
            i += 2;
            continue;
        }
        if low[i] == "seperempat" && low.get(i + 1).map(|s| s.as_str()) == Some("jam") {
            total += 15 * 60_000;
            found = true;
            i += 2;
            continue;
        }
        let unit = low.get(i + 1).map(|s| unit_ms(s)).unwrap_or(0);
        if unit > 0 && low[i].len() <= 4 && low[i].chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = low[i].parse::<u64>() {
                total += n * unit;
                found = true;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    if !found {
        for tok in low {
            if tok.len() <= 3 && tok.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = tok.parse::<u64>() {
                    if n > 0 && n <= 180 {
                        total = n * 60_000;
                        found = true;
                        break;
                    }
                }
            }
        }
    }
    if found { Some(total) } else { None }
}

fn parse_time_ref(low: &[String]) -> Option<(TimeRef, usize, usize, Vec<usize>)> {
    for i in 0..low.len() {
        let unit = low.get(i + 1).map(|s| unit_ms(s)).unwrap_or(0);
        if unit > 0 && low[i].len() <= 4 && low[i].chars().all(|c| c.is_ascii_digit()) {
            let j = i + 2;
            let prev_dalam = i > 0 && low[i - 1] == "dalam";
            let next_word = low.get(j).map(|s| s.as_str()).unwrap_or("");
            let next2 = low.get(j + 1).map(|s| s.as_str()).unwrap_or("");
            if prev_dalam
                || next_word == "lagi"
                || next_word == "kemudian"
                || (next_word == "dari" && next2 == "sekarang")
            {
                let start = if prev_dalam { i - 1 } else { i };
                let end = if next_word == "dari" { j + 2 } else { j + 1 };
                let n = low[i].parse::<u64>().unwrap_or(0);
                return Some((TimeRef::Relative { ms: n * unit }, start, end, Vec::new()));
            }
        }
        if low[i] == "setengah"
            && low.get(i + 1).map(|s| s.as_str()) == Some("jam")
            && matches!(low.get(i + 2).map(|s| s.as_str()), Some("lagi") | Some("kemudian"))
        {
            return Some((TimeRef::Relative { ms: 30 * 60_000 }, i, i + 3, Vec::new()));
        }
    }

    for i in 0..low.len() {
        if low[i] != "jam" && low[i] != "pukul" {
            continue;
        }
        let next = low.get(i + 1)?;
        let (mut hour, num_len) = if next.len() <= 2 && next.chars().all(|c| c.is_ascii_digit()) {
            (next.parse::<u32>().ok()?, 1)
        } else if next == "dua" && low.get(i + 2).map(|s| s.as_str()) == Some("belas") {
            (12, 2)
        } else if let Some(v) = num_word(next) {
            (v, 1)
        } else {
            continue;
        };
        if hour > 23 {
            continue;
        }
        let mut j = i + 1 + num_len;
        let p = low.get(j).and_then(|w| period_of(w));
        if let Some(period) = p {
            let pword = low[j].as_str();
            j += 1;
            if period == "pm" {
                if hour == 12 {
                    if pword == "malam" || pword == "malem" {
                        hour = 0;
                    }
                } else if hour < 12 {
                    hour += 12;
                }
            } else if period == "am" && hour == 12 {
                hour = 0;
            }
        }

        let mut day_offset = 0i64;
        let mut day_words = Vec::new();
        for d in 0..low.len() {
            if low[d] == "besok" {
                day_offset = 1;
                day_words.push(d);
            } else if low[d] == "lusa" {
                day_offset = 2;
                day_words.push(d);
            } else if low[d] == "nanti" {
                day_words.push(d);
            } else if low[d] == "hari" && low.get(d + 1).map(|s| s.as_str()) == Some("ini") {
                day_words.push(d);
                day_words.push(d + 1);
            }
        }

        let now = Local::now();
        let now_ms = now.timestamp_millis();
        let base_date = now.date_naive() + ChronoDur::days(day_offset);
        let mk_ts = |date: chrono::NaiveDate, hr: u32| -> i64 {
            let naive = date.and_hms_opt(hr, 0, 0).unwrap();
            Local
                .from_local_datetime(&naive)
                .single()
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(now_ms)
        };
        let mut target_ms = mk_ts(base_date, hour);
        let ambiguous = p.is_none() && (1..=11).contains(&hour);
        if target_ms < now_ms && ambiguous && hour + 12 <= 23 {
            target_ms = mk_ts(base_date, hour + 12);
        }
        if target_ms < now_ms {
            target_ms = mk_ts(base_date + ChronoDur::days(1), hour);
        }
        return Some((
            TimeRef::Absolute {
                ts_ms: target_ms,
                ambiguous,
            },
            i,
            j,
            day_words,
        ));
    }
    None
}

pub fn site_key(site_text: &str) -> Option<(&'static str, &'static str)> {
    let s = site_text.to_lowercase();
    for app in APPS {
        if app.aliases.iter().any(|&a| a == s) {
            return Some((app.key, app.label));
        }
    }
    None
}

fn tab_web_label(w: &str) -> Option<(&'static str, &'static str)> {
    Some(match w {
        "youtube" | "yutub" | "yt" => ("youtube", "YouTube"),
        "whatsapp" | "wa" | "wa web" | "whatsapp web" => ("whatsapp", "WhatsApp"),
        "gmail" => ("gmail", "Gmail"),
        "meet" | "gmeet" | "google meet" => ("meet", "Google Meet"),
        "zoom" => ("zoom", "Zoom"),
        "teams" | "ms teams" => ("teams", "Microsoft Teams"),
        "chatgpt" => ("chatgpt", "ChatGPT"),
        "claude" => ("claude", "Claude"),
        _ => return None,
    })
}

fn try_diagnose(low: &[String]) -> Option<Intent> {
    let flat = format!(" {} ", low.join(" "));
    let wants_metric = [
        " ram ", " cpu ", " memori ", " memory ", " proses ", " lambat ", " berat ", " lag ",
        " ngelag ", " nge-lag ", " lemot ",
    ]
    .iter()
    .any(|w| flat.contains(w));
    let wants_check = [
        " cek ", " lihat ", " liat ", " makan ", " boros ", " banyak ", " kenapa ", " kondisi ",
        " laptop ", " pc ", " status ", " pakai ", " pake ", " tinggi ",
    ]
    .iter()
    .any(|w| flat.contains(w));
    let explicit_sys = flat.contains(" cek ram ")
        || flat.contains(" cek cpu ")
        || flat.contains(" cek laptop ")
        || flat.contains(" makan ram ");
    if (wants_metric && wants_check) || explicit_sys {
        Some(Intent::DiagnoseSystem)
    } else {
        None
    }
}

fn try_theme(low: &[String]) -> Option<Intent> {
    let flat = format!(" {} ", low.join(" "));
    if !flat.contains(" tema ") && !flat.contains(" mode ") {
        return None;
    }
    if [" terang ", " cerah ", " putih ", " light "]
        .iter()
        .any(|w| flat.contains(w))
    {
        return Some(Intent::Theme {
            mode: "light".to_string(),
            label: "Tema Terang".to_string(),
        });
    }
    if [" gelap ", " hitam ", " dark "]
        .iter()
        .any(|w| flat.contains(w))
    {
        return Some(Intent::Theme {
            mode: "dark".to_string(),
            label: "Tema Gelap".to_string(),
        });
    }
    None
}

fn try_time(low: &[String]) -> Option<Intent> {
    let flat = format!(" {} ", low.join(" "));
    if !flat.contains(" jam ") && !flat.contains(" pukul ") {
        return None;
    }
    if flat.contains("jam berapa")
        || flat.contains("pukul berapa")
        || flat.contains("berapa jam")
        || flat.contains("berapa pukul")
    {
        return Some(Intent::Time);
    }
    None
}

fn try_todo_list(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let (n_start, n_end) = find_todo_noun(low)?;
    let mut strong = None;
    let mut weak = None;
    for i in 0..low.len() {
        if i >= n_start && i < n_end {
            continue;
        }
        if strong.is_none() && STRONG_CUES.contains(&low[i].as_str()) {
            strong = Some(i);
        }
        if weak.is_none() && WEAK_CUES.contains(&low[i].as_str()) {
            weak = Some(i);
        }
    }
    if strong.is_some() {
        return Some(Intent::TodoList);
    }
    if weak.is_none() && n_start > 0 {
        return None;
    }
    let add = find_verb_guarded(low, ADD_VERBS, 0, 4);
    if let Some(w_idx) = weak {
        if add.as_ref().map(|a| w_idx > a.0).unwrap_or(true) {
            return Some(Intent::TodoList);
        }
    }
    if add.is_none() {
        let after = finish_text(tokens, n_end, low.len(), &[]);
        if after.is_empty() {
            return Some(Intent::TodoList);
        }
        let rest = (n_end..low.len())
            .any(|k| !PRONOUNS.contains(&low[k].as_str()) && !LEAD_FILLER.contains(&low[k].as_str()));
        if !rest {
            return Some(Intent::TodoList);
        }
    }
    None
}

fn try_todo_add(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let noun = find_todo_noun(low);
    let add = find_verb_guarded(low, ADD_VERBS, 0, 4);
    let mut lead_has_content = false;
    if let Some((n_start, _)) = noun {
        for q in 0..n_start {
            let w = low[q].as_str();
            if ADD_VERBS.contains(&w) || LEAD_FILLER.contains(&w) || PREPOSITIONS.contains(&w) {
                continue;
            }
            lead_has_content = true;
            break;
        }
    }
    if add.is_none() && !lead_has_content {
        return None;
    }
    if low.iter().any(|w| OPEN_VERBS.contains(&w.as_str())) {
        return None;
    }
    if noun.is_none() {
        let a_idx = add.as_ref()?.0;
        if !ADD_VERBS_STRONG.contains(&low[a_idx].as_str()) {
            return None;
        }
    }
    if find_verb_guarded(low, REMINDER_VERBS, 0, 5).is_some() {
        return None;
    }
    if low.iter().any(|w| w == "timer" || w == "countdown") {
        return None;
    }

    let mut text = String::new();
    if lead_has_content {
        let n_start = noun.unwrap().0;
        let mut lf = 0;
        let mut lt = n_start;
        while lf < lt
            && (ADD_VERBS.contains(&low[lf].as_str()) || LEAD_FILLER.contains(&low[lf].as_str()))
        {
            lf += 1;
        }
        while lt > lf && (low[lt - 1] == "di" || low[lt - 1] == "ke") {
            lt -= 1;
        }
        if lt > lf {
            text = join_raw(tokens, lf, lt);
        }
    }
    if text.is_empty() {
        let from = noun.map(|x| x.1).unwrap_or_else(|| add.as_ref().unwrap().0 + 1);
        text = finish_text(tokens, from, low.len(), &[]);
    }
    if text.is_empty() {
        if let (Some(_), Some(a)) = (noun, add) {
            if ADD_VERBS_STRONG.contains(&low[a.0].as_str()) {
                return None;
            }
            return Some(Intent::TodoList);
        }
        return None;
    }
    if text.split_whitespace().count() == 1 && STRONG_CUES.contains(&text.to_lowercase().as_str()) {
        return None;
    }
    Some(Intent::TodoAdd { text })
}

fn try_timer(low: &[String]) -> Option<Intent> {
    let mut hit = false;
    for i in 0..low.len() {
        if low[i] == "timer" || low[i] == "countdown" || low[i] == "mundur" {
            hit = true;
            break;
        }
        if low[i] == "hitung" && low.get(i + 1).map(|s| s.as_str()) == Some("mundur") {
            hit = true;
            break;
        }
        if low[i] != "timur" && fuzzy(&low[i], "timer", 1).is_some() {
            hit = true;
            break;
        }
    }
    if !hit {
        return None;
    }
    let ms = parse_duration(low)?;
    Some(Intent::Timer { ms })
}

fn try_reminder(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let verb = find_verb_guarded(low, REMINDER_VERBS, 0, 5)?;
    let (at, start, end, day_words) = parse_time_ref(low)?;
    let mut skip = vec![false; low.len()];
    for i in start..end.min(low.len()) {
        skip[i] = true;
    }
    for d in day_words {
        if d < skip.len() {
            skip[d] = true;
        }
    }
    let text = finish_text(tokens, verb.0 + 1, low.len(), &skip);
    Some(Intent::ReminderAdd { text, at })
}

fn try_volume(low: &[String]) -> Option<Intent> {
    const UP: &[&str] = &["besarkan", "besarin", "naikkan", "naikin", "tinggikan", "keraskan", "keras"];
    const DOWN: &[&str] = &["kecilkan", "kecilin", "turunkan", "turunin", "pelankan", "rendahkan", "pelan"];

    let mut v_idx = None;
    for (i, w) in low.iter().enumerate() {
        if w == "volume" || w == "volum" || w == "mute" || w == "bisukan" || fuzzy(w, "volume", 1).is_some() {
            v_idx = Some(i);
            break;
        }
    }
    let mut dir = 0i64;
    for w in low {
        if UP.contains(&w.as_str()) {
            dir = 1;
            break;
        }
        if DOWN.contains(&w.as_str()) {
            dir = -1;
            break;
        }
    }
    let Some(vi) = v_idx else {
        if dir != 0 && low.iter().any(|w| w == "suara" || w == "audio") {
            return Some(Intent::Volume {
                level: None,
                delta: Some(dir * 10),
            });
        }
        return None;
    };
    if low[vi] == "mute" || low[vi] == "bisukan" {
        return Some(Intent::Volume {
            level: Some(0),
            delta: None,
        });
    }
    for w in &low[vi + 1..] {
        if w.len() <= 3 && w.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = w.parse::<i64>() {
                return Some(Intent::Volume {
                    level: Some(n.clamp(0, 100)),
                    delta: None,
                });
            }
        }
    }
    if low[vi + 1..].iter().any(|w| w == "maksimal" || w == "maks" || w == "penuh") {
        return Some(Intent::Volume {
            level: Some(100),
            delta: None,
        });
    }
    if dir != 0 {
        return Some(Intent::Volume {
            level: None,
            delta: Some(dir * 10),
        });
    }
    match low.get(vi + 1).map(|s| s.as_str()) {
        Some("naik") => Some(Intent::Volume {
            level: None,
            delta: Some(10),
        }),
        Some("turun") => Some(Intent::Volume {
            level: None,
            delta: Some(-10),
        }),
        _ => None,
    }
}

fn all_non_query(low: &[String], from: usize, to: usize) -> bool {
    (from..to.min(low.len())).all(|i| NONQUERY.contains(&low[i].as_str()))
}

fn putar_video_start(low: &[String]) -> Option<usize> {
    if low.len() >= 2 && low[0] == "putar" && low[1] == "video" {
        return Some(2);
    }
    if low.len() > 2 && LEAD_FILLER.contains(&low[0].as_str()) && low[1] == "putar" && low[2] == "video" {
        return Some(3);
    }
    None
}

fn try_youtube_play(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let n = low.len();
    if let Some(mut start) = putar_video_start(low) {
        if start < n {
            if low[start] == "yang" && start + 1 < n {
                start += 1;
            }
            let last = low[n - 1].as_str();
            if last == "baru" || last == "terbaru" {
                let mut k = n - 1;
                let mut strong = last == "terbaru";
                if k >= start + 1 && low[k - 1] == "paling" {
                    k -= 1;
                    strong = true;
                }
                if k >= start + 1 && low[k - 1] == "yang" {
                    k -= 1;
                }
                if strong && k > start && !all_non_query(low, start, k) {
                    let raw_q = join_raw(tokens, start, k);
                    static YP_RE: OnceLock<Regex> = OnceLock::new();
                    let re = YP_RE.get_or_init(|| {
                        Regex::new(r"(?i)\s+(?:di\s+(?:youtube|yt)|cari)\b").unwrap()
                    });
                    let query = re.replace_all(&raw_q, "").trim().to_string();
                    return Some(Intent::YoutubePlay {
                        query,
                        newest: true,
                    });
                }
            }
        }
    }

    // Broad YoutubePlay triggers:
    // 1) "cari ... dan/lalu play/putar"
    // 2) "putar lagu/musik/video <q> [di youtube/yt]" or "play <q> di youtube/yt"
    let has_terbaru = low.iter().any(|w| w == "terbaru")
        || low.windows(2).any(|w| w[0] == "paling" && w[1] == "baru");
    let has_yt = low.windows(2).any(|w| w[0] == "di" && (w[1] == "youtube" || w[1] == "yt"));
    let has_cari = (0..n.min(3)).any(|i| matches!(low[i].as_str(), "cari" | "carikan" | "cariin"));
    let has_chain_play = low
        .windows(2)
        .any(|w| matches!(w[0].as_str(), "dan" | "lalu" | "langsung") && matches!(w[1].as_str(), "play" | "putar"));

    let mut q_start: Option<usize> = None;
    let mut q_end = n;

    if has_cari && has_chain_play {
        let v = (0..n.min(3))
            .find(|&i| matches!(low[i].as_str(), "cari" | "carikan" | "cariin"))
            .unwrap_or(0);
        let mut s = v + 1;
        if s < n && matches!(low[s].as_str(), "video" | "vidio" | "klip") {
            s += 1;
        }
        q_start = Some(s);
    } else if let Some(pv) = (0..n.min(3))
        .find(|&i| matches!(low[i].as_str(), "putar" | "putarkan" | "putarin" | "play" | "setel"))
    {
        let next = low.get(pv + 1).map(|s| s.as_str()).unwrap_or("");
        let is_media_noun = matches!(next, "lagu" | "musik" | "video" | "vidio");
        if has_yt || (is_media_noun && n > pv + 2) {
            let mut s = pv + 1;
            if matches!(low.get(s).map(|x| x.as_str()), Some("video") | Some("vidio")) {
                s += 1;
            }
            q_start = Some(s);
        }
    }

    let s = q_start?;
    while q_end > s {
        if q_end >= s + 2
            && matches!(low[q_end - 2].as_str(), "dan" | "lalu" | "langsung")
            && matches!(low[q_end - 1].as_str(), "play" | "putar")
        {
            q_end -= 2;
            continue;
        }
        if q_end >= s + 2
            && low[q_end - 2] == "di"
            && (low[q_end - 1] == "youtube" || low[q_end - 1] == "yt")
        {
            q_end -= 2;
            continue;
        }
        if matches!(low[q_end - 1].as_str(), "play" | "putar" | "terbaru") {
            q_end -= 1;
            continue;
        }
        if q_end >= s + 2 && low[q_end - 2] == "paling" && low[q_end - 1] == "baru" {
            q_end -= 2;
            continue;
        }
        break;
    }
    if q_end <= s || all_non_query(low, s, q_end) {
        return None;
    }
    let query = join_raw(tokens, s, q_end);
    if query.is_empty()
        || matches!(
            query.to_lowercase().as_str(),
            "lagu" | "musik" | "video" | "media"
        )
    {
        return None;
    }
    Some(Intent::YoutubePlay {
        query,
        newest: has_terbaru,
    })
}

fn try_youtube(tokens: &[Tok], low: &[String], last_site: Option<&str>) -> Option<Intent> {
    let n = low.len();
    if let Some(mut start) = putar_video_start(low) {
        if start < n {
            let mut end = n;
            if end >= start + 2 && low[end - 2] == "di" && (low[end - 1] == "youtube" || low[end - 1] == "yt") {
                end -= 2;
            }
            if low[start] == "yang" && start + 1 < end {
                start += 1;
            }
            if end > start && !all_non_query(low, start, end) {
                return Some(Intent::Youtube {
                    query: join_raw(tokens, start, end),
                });
            }
        }
    }
    let v = (0..n.min(3)).find(|&i| matches!(low[i].as_str(), "cari" | "carikan" | "cariin" | "search"))?;
    if low.get(v + 1).map(|s| s.as_str()) == Some("di")
        && matches!(low.get(v + 2).map(|s| s.as_str()), Some("youtube") | Some("yt"))
    {
        if n > v + 3 && !all_non_query(low, v + 3, n) {
            return Some(Intent::Youtube {
                query: join_raw(tokens, v + 3, n),
            });
        }
        return None;
    }
    if n >= v + 4
        && low[n - 2] == "di"
        && (low[n - 1] == "youtube" || low[n - 1] == "yt")
        && !all_non_query(low, v + 1, n - 2)
    {
        return Some(Intent::Youtube {
            query: join_raw(tokens, v + 1, n - 2),
        });
    }
    if last_site == Some("youtube.com") && n > v + 1 && !all_non_query(low, v + 1, n) {
        return Some(Intent::Youtube {
            query: join_raw(tokens, v + 1, n),
        });
    }
    None
}

fn try_whatsapp(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let n = low.len();
    let v = if low.first().map(|s| s.as_str()) == Some("kirim") {
        0
    } else if n > 2 && LEAD_FILLER.contains(&low[0].as_str()) && low[1] == "kirim" {
        1
    } else {
        return None;
    };
    let mut i = v + 1;
    if matches!(low.get(i).map(|s| s.as_str()), Some("pesan") | Some("wa")) {
        i += 1;
    }
    if matches!(low.get(i).map(|s| s.as_str()), Some("di") | Some("lewat"))
        && matches!(low.get(i + 1).map(|s| s.as_str()), Some("whatsapp") | Some("wa"))
    {
        i += 2;
    }
    if !matches!(low.get(i).map(|s| s.as_str()), Some("ke") | Some("untuk")) {
        return None;
    }
    i += 1;
    if i >= n {
        return None;
    }
    let mark = (i..n.saturating_sub(1))
        .find(|&k| low[k] == "di" && (low[k + 1] == "whatsapp" || low[k + 1] == "wa"));
    let (contact_to, mut msg_from) = match mark {
        Some(m) if m == i => return None,
        Some(m) => (m, m + 2),
        None => (i + 1, i + 1),
    };
    if matches!(low.get(msg_from).map(|s| s.as_str()), Some("pesan") | Some("yaitu") | Some("bilang")) {
        msg_from += 1;
    }
    if contact_to > n || msg_from >= n {
        return None;
    }
    let contact = join_raw(tokens, i, contact_to);
    let message = join_raw(tokens, msg_from, n);
    if contact.is_empty()
        || message.is_empty()
        || matches!(contact.to_lowercase().as_str(), "whatsapp" | "wa" | "aplikasi")
    {
        return None;
    }
    Some(Intent::Whatsapp { contact, message })
}

fn try_open_tab(tokens: &[Tok], low: &[String], raw: &str) -> Option<Intent> {
    static DOM_RE: OnceLock<Regex> = OnceLock::new();
    let dom_re = DOM_RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:https?://)?([a-z0-9._-]+\.(?:com|org|net|io|id|tv|co)(?:/[^\s]*)?)$")
            .unwrap()
    });
    let raw_trim = raw.trim();
    if let Some(cap) = dom_re.captures(raw_trim) {
        let dom = &cap[1];
        let base = dom.split(['.', '/']).next().unwrap_or(dom).to_lowercase();
        let (site, label) = tab_web_label(&base)
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .unwrap_or_else(|| (base.clone(), base));
        let url = if raw_trim.to_lowercase().starts_with("http") {
            raw_trim.to_string()
        } else {
            format!("https://{}", dom)
        };
        return Some(Intent::OpenTab {
            site,
            label,
            url: Some(url),
        });
    }

    let n = low.len();
    let verb = find_verb_guarded(low, OPEN_VERBS, 0, 4)?;
    let mut start = verb.0 + 1;
    if start >= n {
        return None;
    }
    let is_tab = low[start] == "tab";
    if is_tab {
        start += 1;
    }
    if start >= n {
        return None;
    }
    let mut end = n;
    let mut saw_browser = false;
    if end >= start + 3 && low[end - 3] == "di" && low[end - 2] == "google" && low[end - 1] == "chrome" {
        end -= 3;
        saw_browser = true;
    } else if end >= start + 2 && low[end - 2] == "di" && BROWSER_WORDS.contains(&low[end - 1].as_str()) {
        end -= 2;
        saw_browser = true;
    }
    if end <= start || TAB_SITE_BLOCK.contains(&low[start].as_str()) {
        return None;
    }
    let site = join_raw(tokens, start, end);
    if site.is_empty() {
        return None;
    }
    let site_low = site.to_lowercase();
    let first_word = low[start].as_str();
    let is_web_only = tab_web_label(first_word).is_some()
        || tab_web_label(&site_low).is_some()
        || (end - start >= 2 && matches!(low[end - 1].as_str(), "com" | "org" | "net" | "id" | "io" | "tv" | "co"));

    if !is_tab && !saw_browser && !is_web_only {
        return None;
    }
    let (key, label) = if let Some((k, l)) = tab_web_label(&site_low) {
        (k.to_string(), l.to_string())
    } else if let Some((k, l)) = site_key(&site) {
        (k.to_string(), l.to_string())
    } else if let Some((k, l)) = tab_web_label(first_word) {
        (k.to_string(), l.to_string())
    } else {
        (site_low, site)
    };
    Some(Intent::OpenTab {
        site: key,
        label,
        url: None,
    })
}

fn try_search(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    let verb = find_verb_guarded(low, SEARCH_VERBS, 0, 4)?;
    let explicit = low[verb.0] == "carikan" || low[verb.0] == "cariin";

    let mut hint: Option<String> = None;
    let mut keep = Vec::new();
    let mut i = verb.0 + 1;
    while i < low.len() {
        // "taruh/masukkan ... di shelf notch" is the DESTINATION of the
        // file, not part of its name. Everything from the placement verb
        // on belongs to the follow-up action, never to the query.
        if matches!(low[i].as_str(), "taruh" | "taroh" | "simpan" | "masukin" | "masukkan" | "pindah" | "pindahkan" | "copy" | "salin") {
            break;
        }
        if low[i] == "di" || low[i] == "ke" {
            let mut j = i + 1;
            if matches!(low.get(j).map(|s| s.as_str()), Some("folder") | Some("dalam")) {
                j += 1;
            }
            if matches!(low.get(j).map(|s| s.as_str()), Some("shelf") | Some("notch") | Some("rak")) {
                // shelf destination: stop the query here
                break;
            }
            if low[i] == "di" {
                if let Some(h) = low.get(j).and_then(|w| file_hint(w)) {
                    hint = Some(h.to_string());
                    i = j + 1;
                    continue;
                }
            }
        }
        keep.push(i);
        i += 1;
    }

    let mut words: Vec<(String, String)> = Vec::new();
    for idx in keep {
        let mut w = low[idx].clone();
        let mut raw = tokens[idx].raw.clone();
        if w.len() >= 5 && w.ends_with("nya") {
            let base = &w[..w.len() - 3];
            if base.len() >= 3 || base == "cv" {
                w = base.to_string();
                raw.truncate(raw.len() - 3);
            }
        }
        words.push((w, raw));
    }

    if words.is_empty() && hint.is_none() {
        return None;
    }
    let mut has_cue = false;
    let mut has_topic = false;
    // "cari file cv DAN taruh di shelf": once a file cue/topic is present,
    // a conjunction starts the follow-up clause, not the file name.
    let mut seen_file_word = false;
    let mut cut = words.len();
    for (k, (lw, _)) in words.iter().enumerate() {
        if FILE_CUES.contains(&lw.as_str()) || FILE_TOPIC.contains(&lw.as_str()) || EXTS.contains(&lw.as_str()) {
            seen_file_word = true;
        }
        if seen_file_word && matches!(lw.as_str(), "dan" | "lalu" | "terus" | "kemudian" | "buat" | "untuk" | "biar" | "supaya" | "abis" | "habis") {
            cut = k;
            break;
        }
    }
    words.truncate(cut);
    if words.is_empty() && hint.is_none() {
        return None;
    }
    let mut fallback = String::new();
    for (lw, rw) in &words {
        if FILE_CUES.contains(&lw.as_str()) {
            has_cue = true;
            if fallback.is_empty() {
                fallback = rw.clone();
            }
        }
        if EXTS.contains(&lw.as_str()) || FILE_TOPIC.contains(&lw.as_str()) {
            has_topic = true;
        }
    }
    if !explicit && !has_cue && !has_topic && hint.is_none() {
        return None;
    }

    let mut from = 0;
    while from < words.len()
        && (FILE_CUES.contains(&words[from].0.as_str())
            || matches!(words[from].0.as_str(), "yang" | "namanya" | "bernama" | "nama"))
    {
        from += 1;
    }
    let mut picked: Vec<String> = words[from..]
        .iter()
        .filter(|(lw, _)| !QUERY_NOISE.contains(&lw.as_str()))
        .map(|(_, rw)| rw.clone())
        .collect();

    if picked.is_empty() {
        if !explicit && hint.is_none() {
            return None;
        }
        picked.push(if fallback.is_empty() {
            "file".to_string()
        } else {
            fallback
        });
    }
    let query = clean_text(&picked.join(" "));
    if query.is_empty() {
        return None;
    }
    Some(Intent::SearchFile { query, hint })
}

fn try_open_app(tokens: &[Tok], low: &[String]) -> Option<Intent> {
    if let Some(verb) = find_verb_guarded(low, OPEN_VERBS, 0, 4) {
        if let Some((key, label, _, _, _)) = find_app(low, verb.0 + 1) {
            return Some(Intent::OpenApp {
                app: key.to_string(),
                label: label.to_string(),
            });
        }
        return None;
    }
    if let Some((key, label, idx, len, dist)) = find_app(low, 0) {
        if dist == 0 && idx == 0 && finish_text(tokens, idx + len, low.len(), &[]).is_empty() {
            return Some(Intent::OpenApp {
                app: key.to_string(),
                label: label.to_string(),
            });
        }
    }
    None
}

pub fn parse_single(text: &str, last_site: Option<&str>) -> Intent {
    let raw = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let tokens = tokenize(&raw);
    let low: Vec<String> = tokens.iter().map(|t| t.low.clone()).collect();
    if low.is_empty() {
        return Intent::Unknown { raw };
    }

    if let Some(i) = try_theme(&low) {
        return i;
    }
    if let Some(i) = try_time(&low) {
        return i;
    }
    if let Some(i) = try_diagnose(&low) {
        return i;
    }
    if let Some(i) = try_todo_list(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_todo_add(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_timer(&low) {
        return i;
    }
    if let Some(i) = try_reminder(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_volume(&low) {
        return i;
    }
    if let Some(i) = try_youtube_play(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_youtube(&tokens, &low, last_site) {
        return i;
    }
    if let Some(i) = try_whatsapp(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_open_tab(&tokens, &low, &raw) {
        return i;
    }
    if let Some(i) = try_search(&tokens, &low) {
        return i;
    }
    if let Some(i) = try_open_app(&tokens, &low) {
        return i;
    }

    // Post-intent generic app catch-all ("buka zcode", "jalankan amd")
    static GENERIC_OPEN_RE: OnceLock<Regex> = OnceLock::new();
    let go = GENERIC_OPEN_RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*(?:buka\s+aplikasi|buka\s+apk|bukakan|bukain|jalankan|buka)\s+([a-zA-Z0-9._-]+(?:\s+[a-zA-Z0-9._-]+)?)\s*$")
            .unwrap()
    });
    let stripped = raw.trim_end_matches(&['.', ',', ';', '!', '?'][..]);
    if let Some(cap) = go.captures(stripped) {
        let name = cap[1].trim();
        let low_name = name.to_lowercase();
        if !matches!(
            low_name.as_str(),
            "pintu" | "jendela" | "mata" | "browser" | "file" | "dokumen" | "tab"
        ) {
            return Intent::OpenApp {
                app: low_name,
                label: name.to_string(),
            };
        }
    }

    Intent::Unknown { raw }
}

pub fn parse_local_action(text: &str) -> Option<Intent> {
    let t = text
        .trim()
        .to_lowercase()
        .trim_end_matches(&['.', '!', '?', ','][..])
        .to_string();
    static MEDIA_RE: OnceLock<Regex> = OnceLock::new();
    static VOL_LVL_RE: OnceLock<Regex> = OnceLock::new();
    static VOL_DIR_RE: OnceLock<Regex> = OnceLock::new();

    let mre = MEDIA_RE.get_or_init(|| {
        Regex::new(r"^(?:putar|jeda|pause|play|lanjut|next|stop|sebelumnya|prev(?:ious)?|lagu\s+berikutnya|lagu\s+sebelumnya)(?:\s+(?:lagu|musik|lagi|ya|dong|saja|video|media|spotify))?$")
            .unwrap()
    });
    if mre.is_match(&t) {
        let action = if t.contains("lanjut") || t.contains("next") || t.contains("berikutnya") {
            "next"
        } else if t.contains("sebelumnya") || t.contains("prev") {
            "previous"
        } else {
            "play-pause"
        };
        return Some(Intent::Media {
            action: action.to_string(),
        });
    }

    let vre = VOL_LVL_RE.get_or_init(|| {
        Regex::new(r"^(?:(?:naikkan|turunkan|kecilkan|besarkan|set|atur|ubah)\s+)?(?:volume|suara)(?:\s+(?:jadi|ke|sampai))?\s*(\d{1,3})\s*(?:%|persen)?$")
            .unwrap()
    });
    if let Some(cap) = vre.captures(&t) {
        if let Ok(v) = cap[1].parse::<i64>() {
            return Some(Intent::Volume {
                level: Some(v.clamp(0, 100)),
                delta: None,
            });
        }
    }

    let vdre = VOL_DIR_RE
        .get_or_init(|| Regex::new(r"^(?:volume|suara)\s+(naik|turun|kecil|besar)$").unwrap());
    if let Some(cap) = vdre.captures(&t) {
        let delta = if &cap[1] == "naik" || &cap[1] == "besar" {
            10
        } else {
            -10
        };
        return Some(Intent::Volume {
            level: None,
            delta: Some(delta),
        });
    }
    None
}

pub fn parse_voice_or_chain(text: &str, last_site: Option<&str>) -> ParsedVoiceAction {
    static CHAIN_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHAIN_RE
        .get_or_init(|| Regex::new(r"(?i)[,;]?[\s]*\b(?:lalu|kemudian|terus|dan\s+lalu)\b\s+").unwrap());
    if let Some(m) = re.find(text) {
        let head = text[..m.start()].trim();
        let tail = text[m.end()..].trim();
        if !head.is_empty() && !tail.is_empty() {
            let first = parse_single(head, last_site);
            if !matches!(first, Intent::Unknown { .. }) {
                return ParsedVoiceAction::Chain {
                    first,
                    rest: tail.to_string(),
                };
            }
        }
    }
    // "cari file cv dan taruh di shelf": a bare "dan" also chains when the
    // tail clearly starts a follow-up ACTION (placement/typing verbs).
    static DAN_RE: OnceLock<Regex> = OnceLock::new();
    let dre = DAN_RE.get_or_init(|| Regex::new(r"(?i)\s+dan\s+").unwrap());
    if let Some(m) = dre.find(text) {
        let head = text[..m.start()].trim();
        let tail = text[m.end()..].trim();
        let tail_low = tail.to_lowercase();
        let action_tail = ["taruh", "taroh", "simpan", "masukin", "masukkan", "pindah", "pindahkan", "salin", "copy", "hapus", "kirim", "buka", "ketik", "tulis", "paste"]
            .iter()
            .any(|v| tail_low.starts_with(v));
        if action_tail && !head.is_empty() && !tail.is_empty() {
            let first = parse_single(head, last_site);
            if !matches!(first, Intent::Unknown { .. }) {
                return ParsedVoiceAction::Chain {
                    first,
                    rest: tail.to_string(),
                };
            }
        }
    }
    ParsedVoiceAction::Single(parse_single(text, last_site))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_core_intents() {
        assert_eq!(
            parse_single("ganti ke tema gelap", None),
            Intent::Theme { mode: "dark".into(), label: "Tema Gelap".into() }
        );
        assert_eq!(parse_single("sekarang jam berapa", None), Intent::Time);
        assert_eq!(parse_single("lihat to do list apa saja", None), Intent::TodoList);
        assert_eq!(
            parse_single("ngedit dan belanja di to do list", None),
            Intent::TodoAdd { text: "ngedit dan belanja".into() }
        );
        assert_eq!(parse_single("timer 10 menit", None), Intent::Timer { ms: 600_000 });
        assert!(matches!(
            parse_single("ingatkan 15 menit lagi minum air", None),
            Intent::ReminderAdd { at: TimeRef::Relative { ms: 900_000 }, .. }
        ));
        assert_eq!(
            parse_single("volume 65 persen", None),
            Intent::Volume { level: Some(65), delta: None }
        );
        assert_eq!(
            parse_single("putar video windah basudara paling baru", None),
            Intent::YoutubePlay { query: "windah basudara".into(), newest: true }
        );
        assert_eq!(
            parse_single("cari video windah paling hits dan play", None),
            Intent::YoutubePlay { query: "windah paling hits".into(), newest: false }
        );
        assert_eq!(
            parse_single("putar lagu santai di youtube", None),
            Intent::YoutubePlay { query: "lagu santai".into(), newest: false }
        );
        assert_eq!(
            parse_single("coba cek laptop gw yang makan ram banyak", None),
            Intent::DiagnoseSystem
        );
        assert_eq!(
            parse_single("cari lagu hindia di youtube", None),
            Intent::Youtube { query: "lagu hindia".into() }
        );
        assert_eq!(
            parse_single("kirim wa ke budi halo apa kabar", None),
            Intent::Whatsapp { contact: "budi".into(), message: "halo apa kabar".into() }
        );
        assert!(matches!(
            parse_single("buka youtube", None),
            Intent::OpenTab { site, .. } if site == "youtube"
        ));
        assert!(matches!(
            parse_single("buka wa web", None),
            Intent::OpenTab { site, .. } if site == "whatsapp"
        ));
        assert_eq!(
            parse_single("carikan si visanya di download", None),
            Intent::SearchFile { query: "visa".into(), hint: Some("download".into()) }
        );
        assert!(matches!(
            parse_single("buka chrome", None),
            Intent::OpenApp { app, .. } if app == "chrome"
        ));
    }

    #[test]
    fn file_query_excludes_placement_clause() {
        // The user's exact failing case: query must be just the file name.
        assert_eq!(
            parse_single("cari file cv dan taruh di shelf notch", None),
            Intent::SearchFile { query: "cv".into(), hint: None }
        );
        assert_eq!(
            parse_single("cari file cv taruh di shelf", None),
            Intent::SearchFile { query: "cv".into(), hint: None }
        );
        assert_eq!(
            parse_single("carikan dokumen visa di download lalu kirim ke wa", None),
            Intent::SearchFile { query: "visa".into(), hint: Some("download".into()) }
        );
    }

    #[test]
    fn file_query_strips_question_filler() {
        // The user's exact failing sentence: only "cv" is the file name.
        assert_eq!(
            parse_single("coba cari file dengan kata cv ada apa saja", None),
            Intent::SearchFile { query: "cv".into(), hint: None }
        );
        assert_eq!(
            parse_single("cari file cv ada apa saja", None),
            Intent::SearchFile { query: "cv".into(), hint: None }
        );
        assert_eq!(
            parse_single("carikan dokumen cv yang ada di download", None),
            Intent::SearchFile { query: "cv".into(), hint: Some("download".into()) }
        );
    }

    #[test]
    fn dan_chain_splits_placement_action() {
        match parse_voice_or_chain("cari file cv dan taruh di shelf notch", None) {
            ParsedVoiceAction::Chain { first, rest } => {
                assert!(matches!(first, Intent::SearchFile { query, .. } if query == "cv"));
                assert!(rest.to_lowercase().starts_with("taruh"));
            }
            other => panic!("expected chain, got {:?}", other),
        }
        // A plain conjunction inside a youtube query must NOT split.
        assert!(matches!(
            parse_voice_or_chain("cari di youtube resep nasi goreng dan ayam", None),
            ParsedVoiceAction::Single(Intent::Youtube { .. })
        ));
    }

    #[test]
    fn splits_chains() {
        assert!(matches!(
            parse_voice_or_chain("buka chrome lalu prompt halo dunia", None),
            ParsedVoiceAction::Chain { first: Intent::OpenApp { .. }, .. }
        ));
    }
}
