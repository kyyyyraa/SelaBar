use crate::backend::rates::get_usd_idr;
use crate::backend::store::StoreManager;
use crate::backend::system::diagnose_system;
use crate::backend::BackendServices;
use crate::events::{ScheduleProposal, UiSender};
use crate::logic::executor;
use crate::logic::router::clean_markdown;
use crate::state::{CalendarEvent, ReminderItem};
use chrono::{Datelike, Duration as ChronoDur, Local, NaiveDate, TimeZone};
use regex::Regex;
use std::sync::{atomic::AtomicBool, Arc, OnceLock};

pub const ASK_CHIPS: &[&str] = &[
    "Apa agenda hari ini?",
    "Ringkas notifikasi",
    "Kenapa laptop lambat?",
    "Kurs dolar hari ini?",
];

pub fn strip_tool_markup(t: &str) -> String {
    let mut x = t.to_string();
    for (open, close) in [
        ("<minimax>", "</minimax>"),
        ("<invoke", "</invoke>"),
        ("<parameter", "</parameter>"),
    ] {
        for _ in 0..20 {
            let low = x.to_ascii_lowercase();
            let Some(a) = low.find(open) else { break };
            if let Some(rel_b) = low[a..].find(close) {
                let b = a + rel_b + close.len();
                x = format!("{}{}", &x[..a], &x[b..]);
            } else {
                x.truncate(a);
                break;
            }
        }
    }
    x.replace("</invoke>", "")
        .replace("</parameter>", "")
        .replace("</minimax>", "")
        .replace("[object Object]", "")
}

pub fn clean_answer(t: &str) -> String {
    let s = strip_tool_markup(t);
    clean_markdown(&s)
}

pub fn format_tanggal_id(d: NaiveDate) -> String {
    let weekday = match d.weekday() {
        chrono::Weekday::Mon => "Senin",
        chrono::Weekday::Tue => "Selasa",
        chrono::Weekday::Wed => "Rabu",
        chrono::Weekday::Thu => "Kamis",
        chrono::Weekday::Fri => "Jumat",
        chrono::Weekday::Sat => "Sabtu",
        chrono::Weekday::Sun => "Minggu",
    };
    let month = month_name_id(d.month());
    format!("{}, {} {} {}", weekday, d.day(), month, d.year())
}

pub fn month_name_id(m: u32) -> &'static str {
    match m {
        1 => "Januari",
        2 => "Februari",
        3 => "Maret",
        4 => "April",
        5 => "Mei",
        6 => "Juni",
        7 => "Juli",
        8 => "Agustus",
        9 => "September",
        10 => "Oktober",
        11 => "November",
        12 => "Desember",
        _ => "",
    }
}

pub fn parse_schedule(q: &str) -> Option<ScheduleProposal> {
    let s = q.to_lowercase();
    let is_sched = [
        "jadwal",
        "buat jadwal",
        "meeting",
        "rapat",
        "agenda",
        "janji",
        "konsultasi",
        "presentasi",
    ]
    .iter()
    .any(|kw| s.contains(kw));
    if !is_sched {
        return None;
    }

    static TIME1_RE: OnceLock<Regex> = OnceLock::new();
    static TIME2_RE: OnceLock<Regex> = OnceLock::new();
    let re1 = TIME1_RE.get_or_init(|| Regex::new(r"(?:jam|pukul)\s*(\d{1,2})(?:[:.](\d{2}))?").unwrap());
    let re2 = TIME2_RE.get_or_init(|| Regex::new(r"\b(\d{1,2}):(\d{2})\b").unwrap());

    let (hh, mm): (u32, String) = if let Some(cap) = re1.captures(&s) {
        let h = cap[1].parse::<u32>().ok()?;
        let m = cap.get(2).map(|x| x.as_str().to_string()).unwrap_or_else(|| "00".to_string());
        (h, m)
    } else if let Some(cap) = re2.captures(&s) {
        (cap[1].parse::<u32>().ok()?, cap[2].to_string())
    } else {
        return None;
    };
    if hh > 23 {
        return None;
    }

    let today = Local::now().date_naive();
    let mut target_d = today;

    static DATE_NUM_RE: OnceLock<Regex> = OnceLock::new();
    static DATE_MON_RE: OnceLock<Regex> = OnceLock::new();
    let dn = DATE_NUM_RE.get_or_init(|| Regex::new(r"(?:tanggal|tgl)\s*(\d{1,2})(?:\s*(\d{1,2}))?").unwrap());
    let dm = DATE_MON_RE.get_or_init(|| {
        Regex::new(r"\b(\d{1,2})\s+(januari|februari|maret|april|mei|juni|juli|agustus|september|oktober|november|desember)\b")
            .unwrap()
    });

    if let Some(cap) = dn.captures(&s) {
        let day = cap[1].parse::<u32>().unwrap_or(today.day());
        let mon = cap
            .get(2)
            .and_then(|x| x.as_str().parse::<u32>().ok())
            .unwrap_or(today.month());
        if let Some(mut d) = NaiveDate::from_ymd_opt(today.year(), mon.clamp(1, 12), day.clamp(1, 31)) {
            if d < today {
                let (ny, nm) = if mon == 12 { (today.year() + 1, 1) } else { (today.year(), mon + 1) };
                if let Some(d2) = NaiveDate::from_ymd_opt(ny, nm, day) {
                    d = d2;
                }
            }
            target_d = d;
        }
    } else if let Some(cap) = dm.captures(&s) {
        let day = cap[1].parse::<u32>().unwrap_or(today.day());
        let mon = match &cap[2] {
            "januari" => 1,
            "februari" => 2,
            "maret" => 3,
            "april" => 4,
            "mei" => 5,
            "juni" => 6,
            "juli" => 7,
            "agustus" => 8,
            "september" => 9,
            "oktober" => 10,
            "november" => 11,
            "desember" => 12,
            _ => today.month(),
        };
        if let Some(mut d) = NaiveDate::from_ymd_opt(today.year(), mon, day.clamp(1, 31)) {
            if d < today {
                if let Some(d2) = NaiveDate::from_ymd_opt(today.year() + 1, mon, day) {
                    d = d2;
                }
            }
            target_d = d;
        }
    } else if s.contains("lusa") {
        target_d = today + ChronoDur::days(2);
    } else if s.contains("besok") {
        target_d = today + ChronoDur::days(1);
    }

    static STRIP_VERBS_RE: OnceLock<Regex> = OnceLock::new();
    static STRIP_TIME_RE: OnceLock<Regex> = OnceLock::new();
    static STRIP_WORDS_RE: OnceLock<Regex> = OnceLock::new();
    static STRIP_LINK_PHRASE_RE: OnceLock<Regex> = OnceLock::new();
    let sv = STRIP_VERBS_RE.get_or_init(|| Regex::new(r"(?i)\b(?:buat|bikin|buatkan|jadwalkan|schedule|tambah)\b").unwrap());
    let st = STRIP_TIME_RE.get_or_init(|| Regex::new(r"(?i)(?:jam|pukul)\s*\d{1,2}(?:[:.]\d{0,2})?|\b\d{1,2}:\d{2}\b|(?:tanggal|tgl)\s*\d{1,2}(?:\s*\d{1,2})?").unwrap());
    let sw = STRIP_WORDS_RE.get_or_init(|| Regex::new(r"(?i)\b(?:besok|lusa|hari ini|nanti|malam ini|sore|pagi|siang|januari|februari|maret|april|mei|juni|juli|agustus|september|oktober|november|desember)\b").unwrap());
    let sl = STRIP_LINK_PHRASE_RE.get_or_init(|| Regex::new(r"(?i)\b(?:berikut\s+linknya|dengan\s+link(?:nya)?|linknya|ini\s+linknya|link)\b").unwrap());

    let extracted_link = crate::backend::system::detect_meeting_link(q)
        .map(|l| l.url)
        .unwrap_or_default();
    let mut title = if !extracted_link.is_empty() {
        q.replace(&extracted_link, " ")
    } else {
        q.to_string()
    };
    title = sl.replace_all(&title, " ").to_string();
    title = st.replace_all(&title, " ").to_string();
    title = dm.replace_all(&title, " ").to_string();
    title = sw.replace_all(&title, " ").to_string();
    title = sv.replace_all(&title, " ").to_string();
    let cleaned_title: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let final_title = if cleaned_title.is_empty() {
        "Agenda".to_string()
    } else {
        cleaned_title.chars().take(60).collect()
    };

    Some(ScheduleProposal {
        title: final_title,
        at_iso: format!("{}T{:02}:{}", target_d.format("%Y-%m-%d"), hh, mm),
        note: extracted_link,
    })
}

pub async fn slow_answer() -> String {
    let d = diagnose_system().await;
    if d.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let r = d.get("reason").and_then(|v| v.as_str()).unwrap_or("tidak diketahui");
        return format!("Gagal membaca kondisi laptop: {}.", r);
    }
    let cpu = d.get("cpu_pct").and_then(|v| v.as_f64()).map(|n| format!("{}%", n.round() as i64)).unwrap_or_else(|| "tidak terbaca".into());
    let ram = match (d.get("ram_used_gb").and_then(|v| v.as_f64()), d.get("ram_total_gb").and_then(|v| v.as_f64())) {
        (Some(u), Some(t)) => format!("{} / {} GB", u, t),
        _ => "tidak terbaca".into(),
    };
    let disk = d.get("disk_c_free_gb").and_then(|v| v.as_f64()).map(|n| format!("{} GB bebas", n)).unwrap_or_else(|| "tidak terbaca".into());

    let mut lines = vec![
        "Kondisi laptop sekarang:".to_string(),
        format!("- CPU: {}", cpu),
        format!("- RAM: {}", ram),
        format!("- Disk C: {}", disk),
    ];
    if let Some(procs) = d.get("top_processes").and_then(|v| v.as_array()) {
        let top: Vec<String> = procs
            .iter()
            .take(5)
            .filter_map(|p| {
                let name = p.get("name").and_then(|v| v.as_str())?;
                let mb = p.get("mem_mb").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;
                Some(format!("{} ({} MB)", name, mb))
            })
            .collect();
        if !top.is_empty() {
            lines.push(format!("- Proses teratas: {}", top.join(", ")));
        }
    }
    if let Some(notes) = d.get("notes").and_then(|v| v.as_array()) {
        let ns: Vec<&str> = notes.iter().filter_map(|v| v.as_str()).collect();
        if !ns.is_empty() {
            lines.push(String::new());
            lines.push(format!("Catatan: {}", ns.join("; ")));
        }
    }
    lines.join("\n")
}

fn fmt_rupiah_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (idx, ch) in s.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    if n < 0 {
        out.push('-');
    }
    out.chars().rev().collect()
}

pub(crate) async fn rate_answer() -> String {
    let r = get_usd_idr().await;
    if r.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let reason = r.get("reason").and_then(|v| v.as_str()).unwrap_or("tidak diketahui");
        return format!("Gagal mengambil kurs: {}.", reason);
    }
    let rate = r.get("rate").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64;
    let source = r.get("source").and_then(|v| v.as_str()).unwrap_or("-");
    let when = r
        .get("fetched_at_ms")
        .and_then(|v| v.as_i64())
        .and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map(|dt| dt.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_else(|| "tidak diketahui".to_string());
    format!(
        "Kurs USD/IDR: {} rupiah per 1 dolar.\nSumber: {} (diambil {}).",
        fmt_rupiah_int(rate),
        source,
        when
    )
}

pub async fn run_ask_query(svc: BackendServices, run_id: u64, q: String) {
    executor::run_ask_harness(&svc, run_id, &q).await;
}

pub async fn run_ask_query_cancellable(
    svc: BackendServices,
    run_id: u64,
    q: String,
    cancel: Option<Arc<AtomicBool>>,
) {
    executor::run_ask_harness_cancellable(&svc, run_id, &q, cancel).await;
}

pub async fn run_ask_query_with_attachments(
    svc: BackendServices,
    run_id: u64,
    q: String,
    attachments: Vec<crate::backend::clipboard::PastedAttachment>,
    cancel: Option<Arc<AtomicBool>>,
) {
    executor::run_ask_harness_with_attachments(&svc, run_id, &q, &attachments, cancel).await;
}

pub fn accept_proposal_to_calendar(store: &StoreManager, ui: &UiSender, p: &ScheduleProposal) {
    let mut cal: Vec<CalendarEvent> = serde_json::from_value(store.get("calendarEvents")).unwrap_or_default();
    let (date, time) = p.at_iso.split_once('T').unwrap_or((&p.at_iso, "09:00"));
    let ms = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| {
            let hm: Vec<&str> = time.split(':').collect();
            let hh = hm.first()?.parse::<u32>().ok()?;
            let mm = hm.get(1)?.parse::<u32>().ok()?;
            let naive = d.and_hms_opt(hh, mm, 0)?;
            Local.from_local_datetime(&naive).single().map(|dt| dt.timestamp_millis())
        })
        .unwrap_or_else(crate::state::now_unix_ms);

    // The model sometimes truncates the meeting URL out of the title.
    // Keep it in the note so the Join banner can still find it later.
    let mut note = p.note.clone();
    if note.trim().is_empty() {
        if let Some(link) = crate::backend::system::detect_meeting_link(&p.title) {
            note = link.url;
        }
    } else if let Some(link) = crate::backend::system::detect_meeting_link(&note) {
        // Normalize to the full URL the Join banner can parse.
        note = link.url;
    }
    // Follow-up "masukin linknya <url>" after creating the same event must
    // UPDATE that event, not create a duplicate. Match by title + start time
    // (within 2 minutes), preferring the most recent one.
    let title_low = p.title.trim().to_lowercase();
    let existing = cal
        .iter()
        .enumerate()
        .filter(|(_, ev)| ev.title.trim().to_lowercase() == title_low && (ev.at - ms).abs() <= 120_000)
        .max_by_key(|(_, ev)| ev.at)
        .map(|(i, _)| i);
    let id = if let Some(idx) = existing {
        let ev = &mut cal[idx];
        if !note.trim().is_empty() {
            ev.note = note.clone();
        }
        ev.id.clone()
    } else {
        let id = format!("cal{}", crate::state::now_unix_ms());
        cal.push(CalendarEvent {
            id: id.clone(),
            title: p.title.clone(),
            at: ms,
            date: date.to_string(),
            time: time.to_string(),
            end_time: String::new(),
            end_at: 0,
            note,
        });
        id
    };
    if cal.len() > 200 {
        let drop = cal.len() - 200;
        cal.drain(..drop);
    }
    if let Ok(v) = serde_json::to_value(&cal) {
        store.set(Some(ui), "calendarEvents", v);
    }

    let mut rems: Vec<ReminderItem> = serde_json::from_value(store.get("reminders")).unwrap_or_default();
    if let Some(r) = rems.iter_mut().find(|r| r.id == id) {
        r.at = ms;
        r.message = p.title.clone();
    } else {
        rems.push(ReminderItem {
            id,
            at: ms,
            message: p.title.clone(),
        });
    }
    if let Ok(v) = serde_json::to_value(&rems) {
        store.set(Some(ui), "reminders", v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_followup_updates_same_event_no_duplicate() {
        // Simulate the store-level upsert: same title + same start time must
        // update the existing event note, not push a second event.
        let mut cal: Vec<CalendarEvent> = vec![CalendarEvent {
            id: "cal1".into(),
            title: "Kelas Sosio".into(),
            at: 1_000_000,
            date: "2026-11-19".into(),
            time: "12:00".into(),
            end_time: String::new(),
            end_at: 0,
            note: String::new(),
        }];
        let p = ScheduleProposal {
            title: "Kelas Sosio".into(),
            at_iso: "1970-01-01T00:16".into(), // not used directly here
            note: "https://meet.google.com/hya-ermm-afg".into(),
        };
        let ms = 1_000_000i64;
        let title_low = p.title.trim().to_lowercase();
        let existing = cal.iter().enumerate()
            .filter(|(_, ev)| ev.title.trim().to_lowercase() == title_low && (ev.at - ms).abs() <= 120_000)
            .max_by_key(|(_, ev)| ev.at)
            .map(|(i, _)| i);
        assert_eq!(existing, Some(0));
        if let Some(idx) = existing { cal[idx].note = p.note.clone(); }
        assert_eq!(cal.len(), 1);
        assert_eq!(cal[0].note, "https://meet.google.com/hya-ermm-afg");
        assert!(crate::backend::system::detect_meeting_link(&cal[0].note).is_some());
    }

    #[test]
    fn truncated_meet_title_keeps_link_in_note() {
        // The model truncated the URL mid-code; detection correctly refuses
        // the partial link, but a FULL link in the title must be recovered
        // into the note when the note is empty.
        let partial = "rapat berikut linknya https://meet.google.com/hya-erm";
        assert!(crate::backend::system::detect_meeting_link(partial).is_none());

        let full = "rapat himpunan https://meet.google.com/hya-ermd-afg";
        let note = String::new();
        let recovered = if note.trim().is_empty() {
            crate::backend::system::detect_meeting_link(full).map(|l| l.url)
        } else {
            None
        };
        assert_eq!(recovered.as_deref(), Some("https://meet.google.com/hya-ermd-afg"));
    }

    #[test]
    fn reminder_message_with_zoom_link_detected() {
        let msg = "Weekly Sync https://us05web.zoom.us/j/98765432100?pwd=Ab12";
        let link = crate::backend::system::detect_meeting_link(msg);
        assert!(link.is_some());
    }

    #[test]
    fn parses_round_and_exact_hours() {
        let p1 = parse_schedule("buat jadwal rapat besok jam 7").unwrap();
        assert!(p1.at_iso.ends_with("T07:00"));
        assert!(p1.title.to_lowercase().contains("rapat"));

        let p2 = parse_schedule("meeting klien jam 19.30").unwrap();
        assert!(p2.at_iso.ends_with("T19:30"));

        let p3 = parse_schedule(
            "jadwalkan rapat himpunan hari ini jam 19:45 berikut linknya https://meet.google.com/hya-ermn-afg",
        )
        .unwrap();
        assert_eq!(p3.title, "rapat himpunan");
        assert_eq!(p3.note, "https://meet.google.com/hya-ermn-afg");
        assert!(p3.at_iso.ends_with("T19:45"));
    }

    #[test]
    fn strip_tool_markup_is_safe_on_multibyte_unicode() {
        let s = "İstanbul <invoke name=\"weather\"><parameter name=\"location\">İzmir</parameter></invoke> cerah.";
        let out = strip_tool_markup(s);
        assert_eq!(out, "İstanbul  cerah.");
    }
}
