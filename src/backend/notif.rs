use crate::events::{NotifPayload, ToastAction, ToastInput, UiEvent, UiSender};
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct NotificationWatcher {
    running: Arc<AtomicBool>,
}

impl NotificationWatcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, ui: UiSender) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = Arc::clone(&self.running);

        std::thread::spawn(move || {
            let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
            if local_app_data.is_empty() {
                return;
            }
            let db_path = PathBuf::from(&local_app_data)
                .join("Microsoft")
                .join("Windows")
                .join("Notifications")
                .join("wpndatabase.db");
            if !db_path.exists() {
                return;
            }

            let mut seen: Option<HashSet<String>> = None;
            let mut seed_max_at: i64 = 0;

            while running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(800));

                let Ok(conn) = Connection::open_with_flags(
                    &db_path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
                ) else {
                    continue;
                };

                let mut handlers: HashMap<i64, String> = HashMap::new();
                if let Ok(mut stmt) = conn.prepare("SELECT RecordId, PrimaryId FROM NotificationHandler") {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    }) {
                        for item in rows.flatten() {
                            handlers.insert(item.0, item.1);
                        }
                    }
                }

                let Ok(mut query_stmt) = conn.prepare(
                    "SELECT Id, HandlerId, CAST(ArrivalTime AS TEXT), CAST(Payload AS TEXT) \
                     FROM Notification WHERE Type='toast' ORDER BY ArrivalTime DESC LIMIT 20",
                ) else {
                    continue;
                };

                struct RawToast {
                    id: i64,
                    handler_id: i64,
                    at2: String,
                    p2: String,
                }

                let toasts: Vec<RawToast> = match query_stmt.query_map([], |row| {
                    Ok(RawToast {
                        id: row.get::<_, i64>(0)?,
                        handler_id: row.get::<_, i64>(1)?,
                        at2: row.get::<_, String>(2)?,
                        p2: row.get::<_, String>(3)?,
                    })
                }) {
                    Ok(mapped) => mapped.flatten().collect(),
                    Err(_) => continue,
                };

                let current_keys: HashSet<String> =
                    toasts.iter().map(|t| format!("{}@{}", t.id, t.at2)).collect();

                let mut new_toasts: Vec<&RawToast> = Vec::new();
                match seen.as_mut() {
                    None => {
                        for t in &toasts {
                            if let Ok(v) = t.at2.trim().parse::<i64>() {
                                if v > seed_max_at {
                                    seed_max_at = v;
                                }
                            }
                        }
                        seen = Some(current_keys);
                    }
                    Some(seen_set) => {
                        for old in seen_set.difference(&current_keys) {
                            if let Some(old_id) = old.split('@').next() {
                                if !old_id.is_empty() {
                                    ui.send(UiEvent::NotifRemoved(old_id.to_string()));
                                }
                            }
                        }
                        for t in &toasts {
                            let key = format!("{}@{}", t.id, t.at2);
                            if !seen_set.contains(&key) {
                                let stamped = t.at2.trim().parse::<i64>().unwrap_or(0);
                                if stamped == 0 || stamped > seed_max_at {
                                    new_toasts.push(t);
                                }
                            }
                        }
                        *seen_set = current_keys;
                    }
                }

                if new_toasts.is_empty() {
                    continue;
                }
                new_toasts.sort_by(|a, b| a.at2.cmp(&b.at2));

                for t in new_toasts {
                    let handler_app = handlers
                        .get(&t.handler_id)
                        .cloned()
                        .unwrap_or_else(|| format!("handler:{}", t.handler_id));
                    if let Some(payload) = parse_toast_xml(&t.p2, t.id, &t.at2, &handler_app) {
                        ui.send(UiEvent::NotifArrived(payload));
                    }
                }
            }
        });
    }
}

pub fn normalize_logo_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let stripped = if trimmed
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:///"))
    {
        &trimmed[8..]
    } else if trimmed
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("file://"))
    {
        &trimmed[7..]
    } else {
        trimmed
    };
    percent_decode_utf8(stripped)
}

pub fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_profile(launch_str: &str) -> String {
    let segs: Vec<String> = launch_str
        .replace('\n', "|")
        .split('|')
        .map(|s| s.trim().to_string())
        .collect();
    if let Some(idx) = segs.iter().position(|s| s == "profile") {
        if let Some(next) = segs.get(idx + 1) {
            return next.clone();
        }
    }
    String::new()
}

fn url_host(u: &str) -> String {
    let after = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    after
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn wa_root_url(u: &str) -> bool {
    let host = url_host(u);
    if !host.contains("whatsapp") {
        return false;
    }
    let after = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let path_query = match after.find('/') {
        Some(idx) => &after[idx..],
        None => return true,
    };
    let path = path_query.split(['?', '#']).next().unwrap_or("");
    path.is_empty() || path == "/"
}

fn wa_jid_url(jid: &str) -> Option<String> {
    if !jid.contains("@s.whatsapp.net") {
        return None;
    }
    let phone: String = jid
        .split('@')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if phone.len() >= 8 {
        Some(format!("https://web.whatsapp.com/send?phone={}", phone))
    } else {
        None
    }
}

pub fn target_url_of(launch: &str, jid: &str, site: &str) -> String {
    let mut first_url: Option<String> = None;
    let mut first_wa_chat: Option<String> = None;
    for seg in launch
        .split(['|', '\n', '\r', '"', '\''])
        .map(|s| s.trim_matches(|c: char| c.is_control() || c == '"' || c as u32 == 39 || c == '}'))
    {
        if !(seg.starts_with("http://") || seg.starts_with("https://")) {
            continue;
        }
        if first_url.is_none() {
            first_url = Some(seg.to_string());
        }
        let low = seg.to_lowercase();
        if url_host(seg).contains("whatsapp")
            && !wa_root_url(seg)
            && (low.contains("/send?phone=")
                || low.contains("/send/?phone=")
                || low.contains("wa.me/"))
            && first_wa_chat.is_none()
        {
            first_wa_chat = Some(seg.to_string());
        }
    }
    // A WhatsApp origin-only launch (web.whatsapp.com) must not outrank the
    // sender chat: prefer an explicit chat link, then the sender JID, and
    // only fall back to the bare site.
    if let Some(chat) = first_wa_chat {
        return chat;
    }
    let first_is_wa_root = first_url.as_deref().map(wa_root_url).unwrap_or(false);
    if first_is_wa_root {
        if let Some(u) = wa_jid_url(jid) {
            return u;
        }
    }
    if let Some(u) = first_url {
        return u;
    }
    if let Some(u) = wa_jid_url(jid) {
        return u;
    }
    let clean_site = site
        .trim()
        .strip_prefix("via ")
        .or_else(|| site.trim().strip_prefix("Via "))
        .unwrap_or(site.trim())
        .trim();
    if !clean_site.is_empty() {
        let h = clean_site
            .split("://")
            .last()
            .unwrap_or(clean_site)
            .split('/')
            .next()
            .unwrap_or("")
            .trim();
        if !h.is_empty() && !h.contains(' ') {
            return format!("https://{}", h);
        }
    }
    String::new()
}
fn attr_val(tag: &str, attr: &str) -> String {
    for q in ['"', '\''] {
        let pat = format!("{}={}", attr, q);
        if let Some(pos) = tag.find(&pat) {
            let rest = &tag[pos + pat.len()..];
            if let Some(end) = rest.find(q) {
                return decode_entities(&rest[..end]);
            }
        }
    }
    String::new()
}

pub fn parse_toast_xml(xml: &str, id: i64, at: &str, raw_app: &str) -> Option<NotifPayload> {
    if xml.is_empty() {
        return None;
    }

    let mut text_parts = Vec::new();
    let mut attribution = String::new();
    let mut logo_path = String::new();
    let mut launch_str = String::new();
    let mut scenario = String::new();
    let mut audio_loop = false;
    let mut inputs: Vec<ToastInput> = Vec::new();
    let mut actions: Vec<ToastAction> = Vec::new();

    if let Some(ts) = xml.find("<toast") {
        if let Some(te) = xml[ts..].find('>') {
            let toast_tag = &xml[ts..ts + te + 1];
            launch_str = attr_val(toast_tag, "launch");
            scenario = attr_val(toast_tag, "scenario");
        }
    }
    if launch_str.is_empty() {
        if let Some(start) = xml.find("launch=\"") {
            let rest = &xml[start + 8..];
            if let Some(end) = rest.find('\"') {
                launch_str = decode_entities(&rest[..end]);
            }
        }
    }

    if let Some(au_s) = xml.find("<audio") {
        if let Some(au_e) = xml[au_s..].find('>') {
            let tag = &xml[au_s..au_s + au_e + 1];
            if attr_val(tag, "loop").eq_ignore_ascii_case("true") {
                audio_loop = true;
            }
        }
    }

    let mut inp_idx = 0;
    while let Some(open) = xml[inp_idx..].find("<input") {
        let s = inp_idx + open;
        if let Some(e) = xml[s..].find('>') {
            let tag = &xml[s..s + e + 1];
            let inp_id = attr_val(tag, "id");
            let placeholder = attr_val(tag, "placeHolderContent");
            if !inp_id.is_empty() {
                inputs.push(ToastInput {
                    id: inp_id,
                    placeholder,
                });
            }
            inp_idx = s + e + 1;
        } else {
            break;
        }
    }

    let mut act_idx = 0;
    while let Some(open) = xml[act_idx..].find("<action") {
        let s = act_idx + open;
        if xml[s..].starts_with("<actions") {
            act_idx = s + 8;
            continue;
        }
        if let Some(e) = xml[s..].find('>') {
            let tag = &xml[s..s + e + 1];
            let label = attr_val(tag, "content");
            let args = attr_val(tag, "arguments");
            let activation_type = attr_val(tag, "activationType");
            let placement = attr_val(tag, "placement");
            let mut input_id = attr_val(tag, "hint-inputId");
            if input_id.is_empty() {
                input_id = attr_val(tag, "inputId");
            }
            if !placement.eq_ignore_ascii_case("contextmenu")
                && (!label.is_empty() || !args.is_empty())
            {
                actions.push(ToastAction {
                    label,
                    args,
                    activation_type,
                    input_id,
                });
            }
            act_idx = s + e + 1;
        } else {
            break;
        }
    }

    let mut search_idx = 0;
    while let Some(open_tag) = xml[search_idx..].find("<text") {
        let tag_start = search_idx + open_tag;
        if let Some(tag_close) = xml[tag_start..].find('>') {
            let full_open = &xml[tag_start..tag_start + tag_close + 1];
            let is_attribution = full_open.contains("placement=\"attribution\"");

            if let Some(content_end) = xml[tag_start + tag_close + 1..].find("</text>") {
                let content_start = tag_start + tag_close + 1;
                let text_val = decode_entities(&xml[content_start..content_start + content_end]);
                if is_attribution {
                    attribution = text_val;
                } else if !text_val.trim().is_empty() {
                    text_parts.push(text_val);
                }
                search_idx = content_start + content_end + 7;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if let Some(img_start) = xml.find("<image") {
        let rest = &xml[img_start..];
        if let Some(src_start) = rest.find("src=\"") {
            let src_rest = &rest[src_start + 5..];
            if let Some(src_end) = src_rest.find('\"') {
                logo_path = normalize_logo_path(&src_rest[..src_end]);
            }
        }
    }

    let title = text_parts.first().cloned().unwrap_or_default();
    let body = if text_parts.len() > 1 {
        text_parts[1..].join("\n")
    } else {
        String::new()
    };
    if title.is_empty() && body.is_empty() {
        return None;
    }

    let mut site = String::new();
    let mut chat = String::new();

    for raw_seg in launch_str.split(['|', '\n', '\r', '"', '\'']) {
        let seg = raw_seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            let clean = seg
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            let host = clean
                .split('/')
                .next()
                .unwrap_or("")
                .trim_start_matches("www.");
            if !host.is_empty() && !host.contains(' ') {
                site = host.to_string();
            }
        }
        if seg.contains("@g.us") {
            chat = "group".to_string();
        } else if seg.contains("@lid") || seg.contains("n#") {
            chat = "chat".to_string();
        }
    }

    if site.is_empty() && !attribution.is_empty() {
        let attr_clean = attribution
            .trim()
            .strip_prefix("via ")
            .or_else(|| attribution.trim().strip_prefix("Via "))
            .unwrap_or(attribution.trim())
            .trim();
        site = attr_clean.to_string();
    }

    let mut app_label = raw_app.to_string();
    let lower_app = raw_app.to_lowercase();
    let is_browser = ["chrome", "zen", "firefox", "edge", "msedge", "brave", "opera", "vivaldi", "arc"]
        .iter()
        .any(|b| lower_app.contains(b));
    if is_browser && !site.is_empty() {
        let site_low = site.to_lowercase();
        if site_low.contains("whatsapp") {
            app_label = "WhatsApp".to_string();
        } else if site_low.contains("discord") {
            app_label = "Discord".to_string();
        } else if site_low.contains("telegram") {
            app_label = "Telegram".to_string();
        } else if site_low.contains("instagram") {
            app_label = "Instagram".to_string();
        } else {
            app_label = site.clone();
        }
    } else if lower_app.contains("discord") {
        app_label = "Discord".to_string();
    } else if lower_app.contains("whatsapp") {
        app_label = "WhatsApp".to_string();
    } else if lower_app.contains("zapfast") {
        app_label = "ZapFast".to_string();
    } else if lower_app.contains("telegram") {
        app_label = "Telegram".to_string();
    }

    let mut jid = String::new();
    for word in launch_str.split(|c: char| !c.is_alphanumeric() && c != '@' && c != '.') {
        if word.contains('@') && word.len() >= 8 {
            jid = word.to_string();
            break;
        }
    }

    let title_low = title.to_lowercase();
    let body_low = body.to_lowercase();
    let act_low: Vec<String> = actions.iter().map(|a| a.label.to_lowercase()).collect();
    let has_accept = act_low
        .iter()
        .any(|c| matches!(c.as_str(), "answer" | "accept" | "terima" | "jawab" | "join"));
    let has_decline = act_low
        .iter()
        .any(|c| matches!(c.as_str(), "decline" | "tolak" | "reject" | "dismiss" | "ignore"));
    let is_call = scenario.eq_ignore_ascii_case("incomingCall")
        || (audio_loop && !actions.is_empty())
        || (has_accept && has_decline)
        || [
            "incoming call",
            "panggilan masuk",
            "telepon suara",
            "telepon video",
            "telepon masuk",
            "panggilan telepon",
            "video call",
            "voice call",
            "sedang menelepon",
            "menelepon anda",
            "is calling",
            "panggilan suara",
            "panggilan video",
        ]
        .iter()
        .any(|kw| title_low.contains(kw) || body_low.contains(kw));

    Some(NotifPayload {
        id: id.to_string(),
        launch: launch_str,
        app: app_label,
        raw_app: raw_app.to_string(),
        site,
        chat,
        title,
        body,
        logo: logo_path,
        at: at.to_string(),
        jid,
        scenario,
        is_call,
        actions,
        inputs,
    })
}

pub fn decode_entities(input: &str) -> String {
    let mut s = input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    // Decode &#xHH; and &#DD;
    while let Some(pos) = s.find("&#x") {
        if let Some(end) = s[pos + 3..].find(';') {
            let hex = &s[pos + 3..pos + 3 + end];
            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                if let Some(ch) = char::from_u32(cp) {
                    s.replace_range(pos..pos + 4 + end, &ch.to_string());
                    continue;
                }
            }
        }
        break;
    }
    while let Some(pos) = s.find("&#") {
        if let Some(end) = s[pos + 2..].find(';') {
            let dec = &s[pos + 2..pos + 2 + end];
            if let Ok(cp) = dec.parse::<u32>() {
                if let Some(ch) = char::from_u32(cp) {
                    s.replace_range(pos..pos + 3 + end, &ch.to_string());
                    continue;
                }
            }
        }
        break;
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toast_call_scenario_and_actions() {
        let xml = r#"<toast scenario="incomingCall"><visual><binding template="ToastGeneric"><text>Budi Hartono</text><text>Panggilan WhatsApp masuk...</text></binding></visual><actions><action content="Terima" arguments="accept" activationType="background"/><action content="Tolak" arguments="decline" activationType="background"/></actions></toast>"#;
        let payload = parse_toast_xml(xml, 1, "08:30", "WhatsApp").expect("should parse");
        assert_eq!(payload.title, "Budi Hartono");
        assert_eq!(payload.body, "Panggilan WhatsApp masuk...");
        assert_eq!(payload.scenario, "incomingCall");
        assert!(payload.is_call);
        assert_eq!(payload.actions.len(), 2);
        assert_eq!(payload.actions[0].label, "Terima");
        assert_eq!(payload.actions[0].args, "accept");
        assert_eq!(payload.actions[1].label, "Tolak");
        assert_eq!(payload.actions[1].args, "decline");
        assert!(payload.inputs.is_empty());
    }

    #[test]
    fn wa_origin_plus_jid_prefers_sender_chat() {
        let launch = "program|zen|profile|C:/Users/x/AppData/Roaming/zen/Profiles/a|url|https://web.whatsapp.com/";
        let u = target_url_of(launch, "6281234567890@s.whatsapp.net", "web.whatsapp.com");
        assert_eq!(u, "https://web.whatsapp.com/send?phone=6281234567890");
    }

    #[test]
    fn newline_zen_launch_prefers_jid_or_extracts_origin() {
        let launch = "program\nzen\nprofile\nC:\\Users\\x\\zen\nwindowsTag\n12345\naction\n{\"action\":\"\",\"origin\":\"https://web.whatsapp.com\"}";
        assert_eq!(
            target_url_of(launch, "6281234567890@s.whatsapp.net", "web.whatsapp.com"),
            "https://web.whatsapp.com/send?phone=6281234567890"
        );
        assert_eq!(
            target_url_of(launch, "", "web.whatsapp.com"),
            "https://web.whatsapp.com"
        );
    }

    #[test]
    fn explicit_wa_chat_link_beats_origin_and_jid() {
        let launch = "url|https://web.whatsapp.com/|url|https://web.whatsapp.com/send?phone=6289998887776";
        let u = target_url_of(launch, "6281234567890@s.whatsapp.net", "");
        assert_eq!(u, "https://web.whatsapp.com/send?phone=6289998887776");
    }

    #[test]
    fn non_wa_explicit_url_is_preserved_with_path_and_query() {
        let launch = "url|https://mail.google.com/mail/u/0/#inbox/FMfcgzGrc";
        let u = target_url_of(launch, "6281234567890@s.whatsapp.net", "mail.google.com");
        assert_eq!(u, "https://mail.google.com/mail/u/0/#inbox/FMfcgzGrc");
    }

    #[test]
    fn group_jid_never_becomes_personal_phone() {
        let u = target_url_of("url|https://web.whatsapp.com/", "120363023456789012@g.us", "web.whatsapp.com");
        assert_eq!(u, "https://web.whatsapp.com/");
        let u2 = target_url_of("", "120363023456789012@g.us", "");
        assert_eq!(u2, "");
    }

    #[test]
    fn short_or_lid_jid_falls_back_to_site() {
        let u = target_url_of("url|https://web.whatsapp.com/", "12345@s.whatsapp.net", "web.whatsapp.com");
        assert_eq!(u, "https://web.whatsapp.com/");
        let u2 = target_url_of("", "abc123@lid", "web.whatsapp.com");
        assert_eq!(u2, "https://web.whatsapp.com");
    }

    #[test]
    fn parses_toast_quick_reply_input() {
        let xml = r#"<toast><visual><binding template="ToastGeneric"><text>Gian</text><text>Hai!</text></binding></visual><actions><input id="quickReply" type="text" placeHolderContent="Balas pesan..."/><action content="Kirim" arguments="reply" activationType="background"/></actions></toast>"#;
        let payload = parse_toast_xml(xml, 2, "08:31", "Discord").expect("should parse");
        assert_eq!(payload.title, "Gian");
        assert_eq!(payload.body, "Hai!");
        assert!(!payload.is_call);
        assert_eq!(payload.inputs.len(), 1);
        assert_eq!(payload.inputs[0].id, "quickReply");
        assert_eq!(payload.inputs[0].placeholder, "Balas pesan...");
        assert_eq!(payload.actions.len(), 1);
        assert_eq!(payload.actions[0].label, "Kirim");
    }

    #[test]
    fn skips_contextmenu_actions_but_keeps_call_buttons() {
        let zen_xml = r#"<toast launch="program&#xA;zen&#xA;action&#xA;{&quot;origin&quot;:&quot;https://web.whatsapp.com&quot;}"><visual><binding template="ToastImageAndText04"><text id="1">Budi</text><text id="2">Panggilan suara masuk</text><text id="3" placement="attribution">via web.whatsapp.com</text></binding></visual><actions><action content="Disable notifications from web.whatsapp.com" arguments="snooze" placement="contextmenu"/><action content="Notification settings" arguments="settings" placement="contextmenu"/></actions></toast>"#;
        let zen_payload =
            parse_toast_xml(zen_xml, 3, "12:00", "ZenToast-F0DC299D809B9700").expect("zen parses");
        assert!(zen_payload.is_call);
        assert!(zen_payload.actions.is_empty());

        let wa_xml = r#"<toast scenario="incomingCall"><visual><binding template="ToastGeneric"><text>Budi</text><text>Panggilan masuk</text></binding></visual><actions><action content="Terima" arguments="accept"/><action content="Settings" arguments="settings" placement="contextmenu"/></actions></toast>"#;
        let wa_payload =
            parse_toast_xml(wa_xml, 4, "12:01", "5319275A.WhatsAppDesktop").expect("wa parses");
        assert_eq!(wa_payload.actions.len(), 1);
        assert_eq!(wa_payload.actions[0].label, "Terima");
    }

    #[test]
    fn normalizes_zapfast_label_and_file_uri_logo() {
        let zf_xml = r#"<toast><visual><binding template="ToastGeneric"><image src="file:///C:/Users/Demo/My%20Avatar.jpg"/><text>Tim Proyek</text><text>Budi: Sticker</text></binding></visual></toast>"#;
        let payload = parse_toast_xml(zf_xml, 5, "19:00", "me.paolino.zapfast").expect("zf parses");
        assert_eq!(payload.app, "ZapFast");
        assert_eq!(payload.raw_app, "me.paolino.zapfast");
        assert_eq!(payload.logo, "C:/Users/Demo/My Avatar.jpg");
        assert_eq!(normalize_logo_path(""), "");
    }
}
