use crate::backend::win;
use crate::events::{
    HwndHandle, InstalledApp, LiveUsage, MeetingLink, MeetingProvider, NotifPayload,
};
use base64::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn silent(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn lock_workstation() -> bool {
    unsafe { windows::Win32::System::Shutdown::LockWorkStation().is_ok() }
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn open_uri_via_shell(uri: &str) -> bool {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file = to_wide(uri);
    let verb = to_wide("open");
    let init_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let res = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if init_ok {
        unsafe {
            CoUninitialize();
        }
    }
    res > 32
}

pub fn open_url_async(url: &str) {
    let u = url.trim().to_string();
    if !u.is_empty() {
        std::thread::spawn(move || {
            let _ = open_url(&u);
        });
    }
}

pub fn open_path_async(path: &str) {
    let p = path.trim().to_string();
    if !p.is_empty() {
        std::thread::spawn(move || {
            let _ = open_path(&p);
        });
    }
}

pub fn reveal_path_async(path: &str) {
    let p = path.trim().to_string();
    if !p.is_empty() {
        std::thread::spawn(move || {
            let _ = reveal_path(&p);
        });
    }
}

pub fn is_safe_spotify_uri(uri: &str) -> bool {
    if !uri.starts_with("spotify:") || uri.len() > 256 {
        return false;
    }
    uri.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                ':' | '/' | '?' | '=' | '&' | '-' | '_' | '.' | '%' | '!' | '~' | '*' | '(' | ')'
            )
    })
}

pub fn open_spotify_uri(uri: &str) -> bool {
    if is_safe_spotify_uri(uri) {
        open_uri_via_shell(uri)
    } else {
        false
    }
}

pub fn open_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    let low = trimmed.to_ascii_lowercase();
    if low.starts_with("https://")
        || low.starts_with("http://")
        || low.starts_with("mailto:")
        || is_safe_protocol_uri(trimmed)
    {
        open_uri_via_shell(trimmed)
    } else {
        false
    }
}

pub fn open_path(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() {
        return false;
    }
    open_uri_via_shell(p)
}

pub fn reveal_path(path: &str) -> bool {
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{ILCreateFromPathW, SHOpenFolderAndSelectItems};

    let p = path.trim().replace('/', "\\");
    if p.is_empty() {
        return false;
    }
    let wide = to_wide(&p);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let pidl = ILCreateFromPathW(windows::core::PCWSTR(wide.as_ptr()));
        if !pidl.is_null() {
            let ok = SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
            CoTaskMemFree(Some(pidl as *const _));
            if ok {
                return true;
            }
        }
    }
    if let Some(parent) = Path::new(&p).parent() {
        return open_uri_via_shell(&parent.to_string_lossy());
    }
    false
}

fn extract_url_host(raw: &str) -> String {
    let u = raw.trim();
    let after_scheme = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let host_part = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host = host_part
        .split(':')
        .next()
        .unwrap_or(host_part)
        .to_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

pub(crate) fn is_gecko_switch_to_tab_label(text_low: &str) -> bool {
    [
        "switch to tab",
        "beralih ke tab",
        "pindah ke tab",
        "alihkan ke tab",
    ]
    .iter()
    .any(|m| text_low.contains(m))
}

fn should_spawn_root_tab(is_gecko: bool, tabs_seen: usize, gecko_checked: bool) -> bool {
    if is_gecko {
        tabs_seen > 0 && gecko_checked
    } else {
        tabs_seen > 0
    }
}

fn parse_url_tab_info(raw: &str) -> (bool, Vec<String>, Vec<String>) {
    let u = raw.trim();
    let after_scheme = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let (host_part, path_and_query) = match after_scheme.find('/') {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, ""),
    };
    let host = host_part
        .split(':')
        .next()
        .unwrap_or(host_part)
        .to_lowercase();
    let is_pure_root = (path_and_query.is_empty()
        || path_and_query == "/"
        || path_and_query == "/#")
        && !path_and_query.contains('?');

    let h_clean = host.strip_prefix("www.").unwrap_or(&host);
    let mut domain_kws = Vec::new();
    let mut specific_kws = Vec::new();

    if h_clean.contains("youtube") || h_clean == "youtu.be" {
        domain_kws.push("youtube".to_string());
        if let Some(pos) = path_and_query.find("v=") {
            let vid: String = path_and_query[pos + 2..]
                .split(['&', '#', '/'])
                .next()
                .unwrap_or("")
                .chars()
                .take(16)
                .collect();
            if vid.len() >= 8 {
                specific_kws.push(vid.to_lowercase());
            }
        }
    } else if h_clean.contains("whatsapp") {
        domain_kws.push("whatsapp".to_string());
    } else if h_clean.contains("discord") {
        domain_kws.push("discord".to_string());
    } else if h_clean.contains("mail.google") || h_clean == "gmail.com" {
        domain_kws.extend(["gmail", "inbox", "mail"].map(str::to_string));
    } else if h_clean.contains("meet.google") {
        domain_kws.extend(["meet.google.com", "google meet"].map(str::to_string));
        let seg = path_and_query
            .trim_start_matches('/')
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if seg.len() >= 9 {
            specific_kws.push(seg.to_lowercase());
        }
    } else if h_clean.contains("zoom.us") {
        domain_kws.push("zoom".to_string());
        let seg = path_and_query
            .trim_start_matches('/')
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("");
        if seg.len() >= 6 {
            specific_kws.push(seg.to_lowercase());
        }
    } else if h_clean.contains("teams.microsoft") || h_clean.contains("teams.live") {
        domain_kws.push("teams".to_string());
    } else if h_clean.contains("github") {
        domain_kws.push("github".to_string());
    } else if h_clean.contains("telegram") {
        domain_kws.push("telegram".to_string());
    } else if h_clean.contains("instagram") {
        domain_kws.push("instagram".to_string());
    } else if h_clean == "x.com" || h_clean.contains("twitter") {
        domain_kws.extend(["x.com", "twitter"].map(str::to_string));
    } else if h_clean.contains("maps.google") {
        domain_kws.push("maps".to_string());
    } else if h_clean.contains("drive.google") {
        domain_kws.push("drive".to_string());
    } else if h_clean.contains("calendar.google") {
        domain_kws.push("calendar".to_string());
    } else if h_clean.contains("gemini.google") {
        domain_kws.extend(["gemini", "google gemini"].map(str::to_string));
    } else if h_clean.contains("aistudio.google") {
        domain_kws.extend(["ai studio", "aistudio"].map(str::to_string));
    } else if h_clean.contains("notebooklm.google") {
        domain_kws.push("notebooklm".to_string());
    } else if h_clean.contains("docs.google") {
        domain_kws.extend(["google docs", "docs"].map(str::to_string));
    } else if h_clean.contains("sheets.google") {
        domain_kws.extend(["google sheets", "sheets"].map(str::to_string));
    } else if h_clean.contains("slides.google") {
        domain_kws.extend(["google slides", "slides"].map(str::to_string));
    } else if h_clean.contains("claude.ai") {
        domain_kws.push("claude".to_string());
    } else if h_clean == "chatgpt.com" || h_clean.contains("chat.openai") {
        domain_kws.push("chatgpt".to_string());
    } else if h_clean.contains("notion.so") || h_clean.contains("notion.site") {
        domain_kws.push("notion".to_string());
    } else {
        let parts: Vec<&str> = h_clean.split('.').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 3
            && matches!(
                parts[parts.len() - 2],
                "google" | "microsoft" | "live" | "yahoo" | "amazon" | "co" | "ac" | "go" | "or" | "com"
            )
        {
            domain_kws.push(parts[0].to_string());
        } else if parts.len() >= 2 {
            domain_kws.push(parts[parts.len() - 2].to_string());
        } else if !h_clean.is_empty() {
            domain_kws.push(h_clean.to_string());
        }
    }
    (is_pure_root, domain_kws, specific_kws)
}

pub(crate) const BROWSER_STEMS: [&str; 9] = [
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "arc",
    "zen",
    "zenbrowser",
];

pub(crate) struct WindowEnumItem {
    pub(crate) hwnd: isize,
    pub(crate) title: String,
    pub(crate) stem: String,
    pub(crate) exe_path: String,
}

fn process_stem_and_path(pid: u32) -> (String, String) {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return (String::new(), String::new());
    }
    let mut stem = String::new();
    let mut exe_path = String::new();
    unsafe {
        if let Ok(hp) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut pbuf = [0u16; 1024];
            let mut plen = pbuf.len() as u32;
            if QueryFullProcessImageNameW(
                hp,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(pbuf.as_mut_ptr()),
                &mut plen,
            )
            .is_ok()
            {
                let full = String::from_utf16_lossy(&pbuf[..plen as usize]);
                stem = Path::new(&full)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                exe_path = full;
            }
            let _ = windows::Win32::Foundation::CloseHandle(hp);
        }
    }
    (stem, exe_path)
}

fn foreground_is_process_stem(needle: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return false;
    }
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let (stem, _) = process_stem_and_path(pid);
        !stem.is_empty() && (stem == n || stem.contains(&n))
    }
}

pub(crate) unsafe fn enum_visible_windows() -> Vec<WindowEnumItem> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut tbuf = [0u16; 512];
        let tlen = GetWindowTextW(hwnd, &mut tbuf);
        if tlen <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&tbuf[..tlen as usize]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let (stem, exe_path) = process_stem_and_path(pid);
        let vec = &mut *(lparam.0 as *mut Vec<WindowEnumItem>);
        vec.push(WindowEnumItem {
            hwnd: hwnd.0 as isize,
            title,
            stem,
            exe_path,
        });
        BOOL(1)
    }

    let mut items = Vec::new();
    let _ = EnumWindows(Some(cb), LPARAM(&mut items as *mut Vec<WindowEnumItem> as isize));
    items
}

pub(crate) fn make_uia_i4(val: i32) -> windows::Win32::System::Variant::VARIANT {
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
    let mut var = VARIANT::default();
    var.Anonymous.Anonymous = core::mem::ManuallyDrop::new(VARIANT_0_0 {
        vt: VT_I4,
        wReserved1: 0,
        wReserved2: 0,
        wReserved3: 0,
        Anonymous: VARIANT_0_0_0 { lVal: val },
    });
    var
}

fn strip_browser_title_suffix(raw: &str) -> String {
    let mut s = raw.trim().to_lowercase();
    for suf in [
        "— zen browser",
        "- zen browser",
        "— zen",
        "- zen",
        "— mozilla firefox",
        "- mozilla firefox",
        "- google chrome",
        "— google chrome",
        "- microsoft\u{200b} edge",
        "- microsoft edge",
        "- brave",
    ] {
        if let Some(prefix) = s.strip_suffix(suf) {
            s = prefix.trim().to_string();
            break;
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) unsafe fn find_active_browser_doc_for_hwnd(
    uia: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    hwnd: isize,
    wr: windows::Win32::Foundation::RECT,
    prefer_kw: Option<&str>,
) -> Option<(
    windows::Win32::UI::Accessibility::IUIAutomationElement,
    windows::Win32::Foundation::RECT,
)> {
    use windows::Win32::UI::Accessibility::{
        IUIAutomationLegacyIAccessiblePattern, TreeScope_Descendants, UIA_ControlTypePropertyId,
        UIA_DocumentControlTypeId, UIA_LegacyIAccessiblePatternId,
    };

    let active_title = if hwnd != 0 {
        strip_browser_title_suffix(&window_title(hwnd))
    } else {
        String::new()
    };
    let active_words: Vec<&str> = active_title
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .collect();

    let v_doc = make_uia_i4(UIA_DocumentControlTypeId.0);
    let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v_doc).ok()?;
    let docs = root.FindAll(TreeScope_Descendants, &cond).ok()?;
    let n = docs.Length().unwrap_or(0).min(32);
    let mut best: Option<(
        i64,
        windows::Win32::UI::Accessibility::IUIAutomationElement,
        windows::Win32::Foundation::RECT,
    )> = None;
    for i in 0..n {
        let Ok(doc) = docs.GetElement(i) else {
            continue;
        };
        let Ok(br) = doc.CurrentBoundingRectangle() else {
            continue;
        };
        let bw = br.right - br.left;
        let bh = br.bottom - br.top;
        let overlaps =
            br.right > wr.left && br.left < wr.right && br.bottom > wr.top && br.top < wr.bottom;
        if bw < 300 || bh < 200 || !overlaps {
            continue;
        }
        let offscreen = doc.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
        let mut doc_name = doc
            .CurrentName()
            .map(|s| s.to_string().to_lowercase())
            .unwrap_or_default();
        let mut doc_url = String::new();
        if let Ok(acc) =
            doc.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                UIA_LegacyIAccessiblePatternId,
            )
        {
            if doc_name.trim().is_empty() {
                doc_name = acc
                    .CurrentName()
                    .map(|s| s.to_string().to_lowercase())
                    .unwrap_or_default();
            }
            doc_url = acc
                .CurrentValue()
                .map(|s| s.to_string().to_lowercase())
                .unwrap_or_default();
        }
        let doc_clean = strip_browser_title_suffix(&doc_name);

        // Gecko (Zen/Firefox) keeps inactive background tabs in the UIA tree as
        // offscreen Document nodes with valid bounding rects. Strongly prefer
        // visible (on-screen) Documents matching the browser's active window title.
        let onscreen_bonus: i64 = if !offscreen { 4_000_000_000 } else { 0 };
        let title_bonus: i64 = if !active_title.is_empty() && !doc_clean.is_empty() {
            if doc_clean.contains(&active_title)
                || (doc_clean.len() >= 3 && active_title.contains(&doc_clean))
            {
                2_000_000_000
            } else if !active_words.is_empty()
                && active_words.iter().filter(|w| doc_clean.contains(**w)).count() * 2
                    >= active_words.len()
            {
                1_500_000_000
            } else {
                0
            }
        } else {
            0
        };
        let kw_bonus: i64 = match prefer_kw {
            Some(kw) if !kw.is_empty() && (doc_clean.contains(kw) || doc_url.contains(kw)) => {
                1_000_000_000
            }
            _ => 0,
        };
        let area = bw as i64 * bh as i64 + onscreen_bonus + title_bonus + kw_bonus;
        if best.as_ref().map_or(true, |b| area > b.0) {
            best = Some((area, doc, br));
        }
    }
    best.map(|(_, d, r)| (d, r))
}

pub(crate) unsafe fn mouse_click_screen(x: i32, y: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

    let _ = SetCursorPos(x, y);
    std::thread::sleep(Duration::from_millis(20));
    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

pub(crate) unsafe fn mouse_click_screen_restore(x: i32, y: i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};
    let mut saved = POINT::default();
    let has_saved = GetCursorPos(&mut saved).is_ok();
    mouse_click_screen(x, y);
    std::thread::sleep(Duration::from_millis(15));
    if has_saved {
        let _ = SetCursorPos(saved.x, saved.y);
    }
}

pub(crate) unsafe fn direct_click_in_window(hwnd: isize, screen_x: i32, screen_y: i32) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    if hwnd != 0 && GetForegroundWindow().0 as isize != hwnd {
        win::focus_hwnd(hwnd);
        std::thread::sleep(Duration::from_millis(45));
    }
    mouse_click_screen_restore(screen_x, screen_y);
}

/// Hints Windows to trim inactive working-set pages (e.g. after finishing Live Voice/Whisper).
pub fn trim_process_working_set() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetProcessWorkingSetSize(
            hProcess: *mut std::ffi::c_void,
            dwMinimumWorkingSetSize: usize,
            dwMaximumWorkingSetSize: usize,
        ) -> i32;
    }
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub(crate) unsafe fn send_unicode_text(text: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        VIRTUAL_KEY,
    };

    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    if !inputs.is_empty() {
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn clean_wa_sender(raw: &str) -> String {
    let mut s = raw
        .trim()
        .replace(['\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}'], "");
    if let Some(rest) = s.strip_prefix("~ ") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix('~') {
        s = rest.trim_start().to_string();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn normalize_browser_token(hint: &str) -> String {
    let tail = hint.split('!').next().unwrap_or(hint);
    let seg = tail.split('\\').last().unwrap_or(tail);
    let mut tok = seg
        .strip_suffix(".exe")
        .or_else(|| seg.strip_suffix(".EXE"))
        .unwrap_or(seg)
        .trim()
        .to_lowercase();
    if tok.contains("zen") {
        tok = "zen".to_string();
    } else if tok.contains("chrome") {
        tok = "chrome".to_string();
    } else if tok.contains("edge") {
        tok = "msedge".to_string();
    } else if tok.contains("firefox") {
        tok = "firefox".to_string();
    } else if tok.contains("brave") {
        tok = "brave".to_string();
    } else if tok.contains("opera") {
        tok = "opera".to_string();
    } else if tok.contains("vivaldi") {
        tok = "vivaldi".to_string();
    } else if tok.contains("arc") {
        tok = "arc".to_string();
    }
    tok
}

fn parse_guid(raw: &str) -> Option<windows::core::GUID> {
    let hex: String = raw
        .trim()
        .trim_matches(['{', '}'])
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if hex.len() != 32 {
        return None;
    }
    let d1 = u32::from_str_radix(&hex[0..8], 16).ok()?;
    let d2 = u16::from_str_radix(&hex[8..12], 16).ok()?;
    let d3 = u16::from_str_radix(&hex[12..16], 16).ok()?;
    let mut d4 = [0u8; 8];
    for i in 0..8 {
        d4[i] = u8::from_str_radix(&hex[16 + i * 2..18 + i * 2], 16).ok()?;
    }
    Some(windows::core::GUID::from_values(d1, d2, d3, d4))
}

fn read_custom_activator_clsid(raw_app: &str) -> Option<(String, windows::core::GUID)> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CLASSES_ROOT,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };

    let candidate = raw_app.trim();
    if candidate.is_empty() {
        return None;
    }

    unsafe fn query_sz(hkey: HKEY, name: &str) -> Option<String> {
        let wn = to_wide(name);
        let mut kind = 0u32;
        let mut bytes = 0u32;
        if RegQueryValueExW(hkey, wn.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut bytes) != 0
            || bytes < 4
        {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize / 2) + 2];
        if RegQueryValueExW(
            hkey,
            wn.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut bytes,
        ) == 0
            && (kind == REG_SZ || kind == 2)
        {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        } else {
            None
        }
    }

    for (root, base_subkey) in [
        (HKEY_CLASSES_ROOT, "AppUserModelId"),
        (HKEY_CURRENT_USER, r"Software\Classes\AppUserModelId"),
        (HKEY_LOCAL_MACHINE, r"Software\Classes\AppUserModelId"),
    ] {
        let direct_path = to_wide(&format!(r"{}\{}", base_subkey, candidate));
        let mut hk: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, direct_path.as_ptr(), 0, KEY_READ, &mut hk) } == 0
            && !hk.is_null()
        {
            let val = unsafe { query_sz(hk, "CustomActivator") };
            unsafe { RegCloseKey(hk) };
            if let Some(gs) = val {
                if let Some(guid) = parse_guid(&gs) {
                    return Some((candidate.to_string(), guid));
                }
            }
        }

        // Case-insensitive / prefix scan under AppUserModelId
        let base_w = to_wide(base_subkey);
        let mut base_hk: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, base_w.as_ptr(), 0, KEY_READ, &mut base_hk) } == 0
            && !base_hk.is_null()
        {
            let cand_low = candidate.to_lowercase();
            let mut idx = 0u32;
            loop {
                let mut nbuf = [0u16; 512];
                let mut nlen = nbuf.len() as u32;
                let st = unsafe {
                    RegEnumKeyExW(
                        base_hk,
                        idx,
                        nbuf.as_mut_ptr(),
                        &mut nlen,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if st != 0 {
                    break;
                }
                idx += 1;
                let sub = String::from_utf16_lossy(&nbuf[..nlen as usize]);
                let sub_low = sub.to_lowercase();
                if sub_low == cand_low
                    || sub_low.contains(&cand_low)
                    || (cand_low.len() >= 5 && sub_low.starts_with(&cand_low))
                {
                    let mut sh: HKEY = std::ptr::null_mut();
                    if unsafe { RegOpenKeyExW(base_hk, nbuf.as_ptr(), 0, KEY_READ, &mut sh) } == 0
                        && !sh.is_null()
                    {
                        let val = unsafe { query_sz(sh, "CustomActivator") };
                        unsafe { RegCloseKey(sh) };
                        if let Some(gs) = val {
                            if let Some(guid) = parse_guid(&gs) {
                                unsafe { RegCloseKey(base_hk) };
                                return Some((sub, guid));
                            }
                        }
                    }
                }
            }
            unsafe { RegCloseKey(base_hk) };
        }
    }
    None
}

pub fn activate_toast_com(
    raw_app: &str,
    invoked_args: &str,
    user_input: Option<(&str, &str)>,
) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Notifications::{
        INotificationActivationCallback, NOTIFICATION_USER_INPUT_DATA,
    };
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    let Some((aumid, clsid)) = read_custom_activator_clsid(raw_app) else {
        return false;
    };
    let args_s = invoked_args.to_string();
    let input_pair = user_input.map(|(k, v)| (k.to_string(), v.to_string()));

    std::thread::spawn(move || unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let invoke_with = |coinit| -> Option<bool> {
            let init_ok = CoInitializeEx(None, coinit).is_ok();
            let cb_res = CoCreateInstance::<_, INotificationActivationCallback>(
                &clsid,
                None,
                CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER,
            );
            let res = match cb_res {
                Ok(cb) => {
                    let w_app = to_wide(&aumid);
                    let w_args = to_wide(&args_s);
                    let act = if let Some((ref k, ref v)) = input_pair {
                        let wk = to_wide(k);
                        let wv = to_wide(v);
                        let item = NOTIFICATION_USER_INPUT_DATA {
                            Key: PCWSTR(wk.as_ptr()),
                            Value: PCWSTR(wv.as_ptr()),
                        };
                        cb.Activate(PCWSTR(w_app.as_ptr()), PCWSTR(w_args.as_ptr()), &[item])
                    } else {
                        cb.Activate(PCWSTR(w_app.as_ptr()), PCWSTR(w_args.as_ptr()), &[])
                    };
                    Some(act.is_ok())
                }
                Err(_) => None,
            };
            if init_ok {
                CoUninitialize();
            }
            res
        };

        // Try MTA + LocalServer first (packaged/MSIX apps like WhatsApp Desktop),
        // then fall back to STA + InProcServer (browsers like Zen/Firefox).
        invoke_with(COINIT_MULTITHREADED)
            .or_else(|| invoke_with(COINIT_APARTMENTTHREADED))
            .unwrap_or(false)
    })
    .join()
    .unwrap_or(false)
}

fn focus_or_open_url_blocking(
    full_url: &str,
    preferred_tok: &str,
    profile: &str,
    extra_keywords: &[String],
    reuse_domain: bool,
) -> (bool, bool, Option<isize>, bool) {
    focus_or_open_url_blocking_ext(full_url, preferred_tok, profile, extra_keywords, reuse_domain, false)
}

fn focus_or_open_url_blocking_ext(
    full_url: &str,
    preferred_tok: &str,
    profile: &str,
    extra_keywords: &[String],
    reuse_domain: bool,
    strict_browser: bool,
) -> (bool, bool, Option<isize>, bool) {
    let (_is_pure_root, domain_kws, specific_kws) = parse_url_tab_info(full_url);
    let mut search_kws: Vec<String> = if reuse_domain {
        let mut k = domain_kws;
        for ex in extra_keywords {
            let low = ex.trim().to_lowercase();
            if low.len() >= 3 && !k.contains(&low) {
                k.push(low);
            }
        }
        k
    } else {
        specific_kws
    };

    let windows_list = unsafe { enum_visible_windows() };
    let mut ordered_browsers: Vec<WindowEnumItem> = Vec::new();
    for bw in windows_list
        .iter()
        .filter(|w| BROWSER_STEMS.contains(&w.stem.as_str()))
    {
        if !preferred_tok.is_empty() && (bw.stem == preferred_tok || bw.stem.contains(preferred_tok))
        {
            ordered_browsers.push(WindowEnumItem {
                hwnd: bw.hwnd,
                title: bw.title.clone(),
                stem: bw.stem.clone(),
                exe_path: bw.exe_path.clone(),
            });
        }
    }
    if !strict_browser || preferred_tok.is_empty() {
        for bw in windows_list
            .iter()
            .filter(|w| BROWSER_STEMS.contains(&w.stem.as_str()))
        {
            if !ordered_browsers.iter().any(|o| o.hwnd == bw.hwnd) {
                ordered_browsers.push(WindowEnumItem {
                    hwnd: bw.hwnd,
                    title: bw.title.clone(),
                    stem: bw.stem.clone(),
                    exe_path: bw.exe_path.clone(),
                });
            }
        }
    }

    if !ordered_browsers.is_empty() && !search_kws.is_empty() {
        let bws_for_thread: Vec<(isize, String, String)> = ordered_browsers
            .iter()
            .map(|b| (b.hwnd, b.stem.clone(), b.title.clone()))
            .collect();
        let kws_clone = search_kws.clone();
        let reused_hwnd: Option<isize> = std::thread::spawn(move || {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED,
            };
            use windows::Win32::UI::Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationInvokePattern,
                IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern,
                TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
                UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId, UIA_ListItemControlTypeId,
                UIA_SelectionItemPatternId, UIA_TabItemControlTypeId, UIA_TreeItemControlTypeId,
            };

            unsafe fn scan_pass(
                uia: &IUIAutomation,
                conds: &[IUIAutomationCondition],
                bws: &[(isize, String, String)],
                kws: &[String],
            ) -> Option<isize> {
                for &(hwnd, _, _) in bws {
                    let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) else {
                        continue;
                    };
                    for cond in conds {
                        let Ok(tabs) = root.FindAll(TreeScope_Descendants, cond) else {
                            continue;
                        };
                        let count = tabs.Length().unwrap_or(0).min(400);
                        for i in 0..count {
                            let Ok(tab) = tabs.GetElement(i) else {
                                continue;
                            };
                            let name = tab
                                .CurrentName()
                                .map(|s| s.to_string().to_lowercase())
                                .unwrap_or_default();
                            if name.is_empty()
                                || !kws.iter().any(|kw| name.contains(kw.as_str()))
                            {
                                continue;
                            }
                            if let Ok(sel) =
                                tab.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                                    UIA_SelectionItemPatternId,
                                )
                            {
                                let _ = sel.Select();
                            } else if let Ok(inv) =
                                tab.GetCurrentPatternAs::<IUIAutomationInvokePattern>(
                                    UIA_InvokePatternId,
                                )
                            {
                                let _ = inv.Invoke();
                            } else if let Ok(acc) =
                                tab.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                                    UIA_LegacyIAccessiblePatternId,
                                )
                            {
                                let _ = acc.DoDefaultAction();
                            }
                            win::focus_hwnd(hwnd);
                            return Some(hwnd);
                        }
                    }
                }
                None
            }

            unsafe {
                let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
                let mut matched = None;
                if let Ok(uia) =
                    CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                {
                    let mut conds = Vec::new();
                    for ct in [
                        UIA_TabItemControlTypeId.0,
                        UIA_ButtonControlTypeId.0,
                        UIA_TreeItemControlTypeId.0,
                        UIA_ListItemControlTypeId.0,
                    ] {
                        let v = make_uia_i4(ct);
                        if let Ok(c) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) {
                            conds.push(c);
                        }
                    }
                    matched = scan_pass(&uia, &conds, &bws_for_thread, &kws_clone);
                    if matched.is_none() && reuse_domain {
                        // Wake lazy background browser UIA tree and retry once
                        for (hwnd, _, _) in bws_for_thread.iter().take(2) {
                            win::focus_hwnd(*hwnd);
                        }
                        std::thread::sleep(Duration::from_millis(240));
                        matched = scan_pass(&uia, &conds, &bws_for_thread, &kws_clone);
                    }
                }
                if init_ok {
                    CoUninitialize();
                }
                matched
            }
        })
        .join()
        .unwrap_or(None);

        if let Some(hwnd) = reused_hwnd {
            return (true, true, Some(hwnd), false);
        }

        for bw in &ordered_browsers {
            let tlow = bw.title.to_lowercase();
            if search_kws.iter().any(|kw| tlow.contains(kw.as_str())) {
                win::focus_hwnd(bw.hwnd);
                return (true, true, Some(bw.hwnd), false);
            }
        }
    }
    search_kws.clear();

    // Open new URL in preferred browser if specified, otherwise the currently active open browser!
    let target_browser = if !preferred_tok.is_empty() {
        ordered_browsers
            .iter()
            .find(|w| w.stem == preferred_tok || w.stem.contains(preferred_tok))
            .or_else(|| if strict_browser { None } else { ordered_browsers.first() })
    } else {
        ordered_browsers.first()
    };

    if let Some(bw) = target_browser {
        if unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsIconic(
                windows::Win32::Foundation::HWND(bw.hwnd as *mut _),
            )
            .as_bool()
        } {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                    windows::Win32::Foundation::HWND(bw.hwnd as *mut _),
                    windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                );
            }
            std::thread::sleep(Duration::from_millis(240));
        }
        win::focus_hwnd(bw.hwnd);
        if !bw.exe_path.is_empty() {
            let mut cmd = Command::new(&bw.exe_path);
            // -profile is a Gecko (zen/firefox) remoting flag; chromium
            // browsers use --profile-directory and must not receive it.
            if !profile.is_empty() && matches!(bw.stem.as_str(), "zen" | "zenbrowser" | "firefox") {
                cmd.args(["-profile", profile]);
            }
            cmd.arg(full_url);
            if silent(&mut cmd).spawn().is_ok() {
                return (true, false, Some(bw.hwnd), true);
            }
        }
    } else if !preferred_tok.is_empty() {
        if let Some(hwnd) = launch_specific_browser_with_url(preferred_tok, full_url) {
            return (true, false, Some(hwnd), true);
        }
    }

    let ok = open_uri_via_shell(full_url);
    let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize };
    (ok, false, if fg != 0 { Some(fg) } else { None }, true)
}

fn find_browser_exe_for_token(tok: &str) -> Option<String> {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_string());
    let pf86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let candidates: Vec<String> = match tok {
        "chrome" => vec![
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", pf),
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", pf86),
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", local),
        ],
        "msedge" => vec![
            format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", pf86),
            format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", pf),
        ],
        "zen" | "zenbrowser" => vec![
            format!("{}\\Zen Browser\\zen.exe", pf),
            format!("{}\\Zen Browser\\zen.exe", local),
            format!("{}\\Twilight\\zen.exe", pf),
        ],
        "firefox" => vec![
            format!("{}\\Mozilla Firefox\\firefox.exe", pf),
            format!("{}\\Mozilla Firefox\\firefox.exe", pf86),
        ],
        "brave" => vec![
            format!("{}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", pf),
            format!("{}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe", local),
        ],
        _ => Vec::new(),
    };
    candidates
        .into_iter()
        .find(|p| !p.is_empty() && std::path::Path::new(p).is_file())
}

fn launch_specific_browser_with_url(tok: &str, full_url: &str) -> Option<isize> {
    if let Some(exe) = find_browser_exe_for_token(tok) {
        let mut cmd = Command::new(&exe);
        cmd.arg(full_url);
        if silent(&mut cmd).spawn().is_ok() {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(220));
                let wins = unsafe { enum_visible_windows() };
                if let Some(w) = wins.iter().find(|w| w.stem == tok || w.stem.contains(tok)) {
                    win::focus_hwnd(w.hwnd);
                    return Some(w.hwnd);
                }
            }
        }
    }
    let display_name = match tok {
        "chrome" => "Google Chrome",
        "msedge" => "Microsoft Edge",
        "zen" | "zenbrowser" => "Zen",
        "firefox" => "Firefox",
        "brave" => "Brave",
        _ => tok,
    };
    if launch_app(tok, display_name) {
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(250));
            let wins = unsafe { enum_visible_windows() };
            if let Some(w) = wins.iter().find(|w| w.stem == tok || w.stem.contains(tok)) {
                win::focus_hwnd(w.hwnd);
                std::thread::sleep(Duration::from_millis(200));
                let _ = navigate_tab_to_url_blocking(w.hwnd, full_url);
                return Some(w.hwnd);
            }
        }
    }
    None
}

fn window_title(hwnd: isize) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(
            windows::Win32::Foundation::HWND(hwnd as *mut _),
            &mut buf,
        )
    };
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_lowercase()
}

/// Uses Gecko's (`zen` / `firefox`) built-in `% <host>` address-bar Switch-to-Tab
/// search to locate and focus an existing tab even when vertical/collapsed/renamed
/// tabs hide the site name from UIA `TabItem` enumeration.
/// Returns `(switched, checked_dropdown)`.
fn try_gecko_switch_to_tab_blocking(
    hwnd: isize,
    site_host: &str,
    domain_kws: &[String],
) -> (bool, bool) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationInvokePattern,
        IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern,
        TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
        UIA_DataItemControlTypeId, UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId,
        UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId, UIA_ListItemControlTypeId,
        UIA_MenuItemControlTypeId, UIA_SelectionItemPatternId, UIA_TextControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, IsIconic, ShowWindow, SW_RESTORE,
    };

    if hwnd == 0 || site_host.trim().is_empty() {
        return (false, false);
    }
    let host_owned = site_host.to_lowercase();
    let kws_owned: Vec<String> = domain_kws.iter().map(|k| k.to_lowercase()).collect();

    std::thread::spawn(move || unsafe {
        if IsIconic(HWND(hwnd as *mut _)).as_bool() {
            let _ = ShowWindow(HWND(hwnd as *mut _), SW_RESTORE);
            std::thread::sleep(Duration::from_millis(200));
        }
        win::focus_hwnd(hwnd);
        std::thread::sleep(Duration::from_millis(120));
        if GetForegroundWindow().0 as isize != hwnd {
            return (false, false);
        }

        let mut wr = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut wr).is_err() {
            return (false, false);
        }
        let wt_before = window_title(hwnd);
        if win::send_keys("ctrl+l").is_err() {
            return (false, false);
        }
        std::thread::sleep(Duration::from_millis(90));
        send_unicode_text(&format!("% {}", host_owned));
        std::thread::sleep(Duration::from_millis(280));

        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut switched = false;
        let mut checked = false;

        if let Ok(uia) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        {
            if let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) {
                checked = true;
                struct PopupCand {
                    top: i32,
                    bottom: i32,
                    text: String,
                    el: Option<windows::Win32::UI::Accessibility::IUIAutomationElement>,
                    cx: i32,
                    cy: i32,
                    has_br: bool,
                }
                let mut rows: Vec<PopupCand> = Vec::new();
                for ct in [
                    UIA_ListItemControlTypeId.0,
                    UIA_MenuItemControlTypeId.0,
                    UIA_DataItemControlTypeId.0,
                    UIA_GroupControlTypeId.0,
                    UIA_HyperlinkControlTypeId.0,
                    UIA_ButtonControlTypeId.0,
                    UIA_TextControlTypeId.0,
                ] {
                    let v = make_uia_i4(ct);
                    let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                        continue;
                    };
                    let Ok(items) = root.FindAll(TreeScope_Descendants, &cond) else {
                        continue;
                    };
                    let n = items.Length().unwrap_or(0).min(240);
                    for i in 0..n {
                        let Ok(el) = items.GetElement(i) else {
                            continue;
                        };
                        if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                            continue;
                        }
                        let Ok(br) = el.CurrentBoundingRectangle() else {
                            continue;
                        };
                        if br.top < wr.top + 30
                            || br.top > wr.top + 580
                            || br.right <= br.left + 8
                            || br.bottom <= br.top + 8
                        {
                            continue;
                        }
                        let name = el
                            .CurrentName()
                            .map(|s| s.to_string().to_lowercase())
                            .unwrap_or_default();
                        let help = el
                            .CurrentHelpText()
                            .map(|s| s.to_string().to_lowercase())
                            .unwrap_or_default();
                        let (acc_name, acc_desc) = el
                            .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                                UIA_LegacyIAccessiblePatternId,
                            )
                            .ok()
                            .map(|acc| {
                                (
                                    acc.CurrentName()
                                        .map(|s| s.to_string().to_lowercase())
                                        .unwrap_or_default(),
                                    acc.CurrentDescription()
                                        .map(|s| s.to_string().to_lowercase())
                                        .unwrap_or_default(),
                                )
                            })
                            .unwrap_or_default();
                        let combined = format!("{} {} {} {}", name, help, acc_name, acc_desc);
                        if combined.trim().is_empty() {
                            continue;
                        }
                        rows.push(PopupCand {
                            top: br.top,
                            bottom: br.bottom,
                            text: combined,
                            el: Some(el),
                            cx: ((br.left + br.right) / 2).clamp(wr.left + 4, wr.right - 4),
                            cy: ((br.top + br.bottom) / 2).clamp(wr.top + 4, wr.bottom - 4),
                            has_br: br.left >= wr.left - 12 && br.right <= wr.right + 12,
                        });
                    }
                }

                // Prefer Switch-to-Tab element whose own text or neighboring row elements match site_host / domain_kws.
                let mut chosen_idx: Option<usize> = None;
                for (idx, r) in rows.iter().enumerate() {
                    if !is_gecko_switch_to_tab_label(&r.text) {
                        continue;
                    }
                    let direct_match = r.text.contains(&host_owned)
                        || kws_owned.iter().any(|k| r.text.contains(k.as_str()));
                    if direct_match {
                        chosen_idx = Some(idx);
                        break;
                    }
                    let band_match = rows.iter().enumerate().any(|(j, other)| {
                        j != idx
                            && (other.top - r.top).abs() <= 44
                            && (other.bottom - r.bottom).abs() <= 44
                            && (other.text.contains(&host_owned)
                                || kws_owned.iter().any(|k| other.text.contains(k.as_str())))
                    });
                    if band_match && chosen_idx.is_none() {
                        chosen_idx = Some(idx);
                    }
                }

                if let Some(idx) = chosen_idx {
                    if let Some(el) = rows[idx].el.take() {
                        let mut acted = false;
                        if let Ok(inv) =
                            el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                        {
                            acted = inv.Invoke().is_ok();
                        }
                        if !acted {
                            if let Ok(sel) = el
                                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                                    UIA_SelectionItemPatternId,
                                )
                            {
                                acted = sel.Select().is_ok();
                            }
                        }
                        if !acted {
                            if let Ok(acc) = el
                                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                                    UIA_LegacyIAccessiblePatternId,
                                )
                            {
                                acted = acc.DoDefaultAction().is_ok();
                            }
                        }
                        if !acted && rows[idx].has_br {
                            mouse_click_screen_restore(rows[idx].cx, rows[idx].cy);
                        }
                        for _ in 0..5 {
                            std::thread::sleep(Duration::from_millis(90));
                            let wt = window_title(hwnd);
                            if kws_owned.iter().any(|kw| wt.contains(kw.as_str()))
                                || (!wt.is_empty() && wt != wt_before)
                            {
                                switched = true;
                                break;
                            }
                        }
                        if !switched && rows[idx].has_br {
                            mouse_click_screen_restore(rows[idx].cx, rows[idx].cy);
                            for _ in 0..5 {
                                std::thread::sleep(Duration::from_millis(90));
                                let wt = window_title(hwnd);
                                if kws_owned.iter().any(|kw| wt.contains(kw.as_str()))
                                    || (!wt.is_empty() && wt != wt_before)
                                {
                                    switched = true;
                                    break;
                                }
                            }
                        }
                        if switched {
                            crate::backend::logfile::append(&format!(
                                "[notif-tab] gecko-switch ok host='{}' title='{}'",
                                host_owned,
                                window_title(hwnd)
                            ));
                        }
                    }
                }
            }
        }

        if !switched {
            let _ = win::send_keys("esc");
            std::thread::sleep(Duration::from_millis(50));
            let _ = win::send_keys("esc");
        }
        if init_ok {
            CoUninitialize();
        }
        (switched, checked)
    })
    .join()
    .unwrap_or((false, false))
}

/// Selects the verified site tab in the source browser and returns
/// `(Some(hwnd), tabs_seen, gecko_checked)` when found/selected, or
/// `(None, tabs_seen, gecko_checked)` when no matching tab could be verified.
fn select_site_tab_blocking(
    browsers: &[(isize, String, String)],
    domain_kws: &[String],
    site_host: &str,
) -> (Option<isize>, usize, bool) {
    if browsers.is_empty() || domain_kws.is_empty() {
        return (None, 0, false);
    }
    let host_low = site_host.trim().to_lowercase();
    // Fast path: a window whose active tab title already matches the target site.
    for &(hwnd, _, ref title) in browsers {
        let tl = title.to_lowercase();
        if domain_kws.iter().any(|kw| tl.contains(kw.as_str()))
            || (!host_low.is_empty() && tl.contains(&host_low))
        {
            win::focus_hwnd(hwnd);
            crate::backend::logfile::append(&format!(
                "[notif-tab] fast-path matched host='{}' hwnd={} title='{}'",
                host_low, hwnd, tl
            ));
            return (Some(hwnd), 1, false);
        }
    }
    let bws: Vec<(isize, String, String)> = browsers.to_vec();
    let kws: Vec<String> = domain_kws.to_vec();
    let host_clone = host_low.clone();
    std::thread::spawn(move || {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        };
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationInvokePattern,
            IUIAutomationLegacyIAccessiblePattern, IUIAutomationScrollItemPattern,
            IUIAutomationSelectionItemPattern, TreeScope_Descendants, UIA_ButtonControlTypeId,
            UIA_ControlTypePropertyId, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
            UIA_ListItemControlTypeId, UIA_ScrollItemPatternId, UIA_SelectionItemPatternId,
            UIA_TabItemControlTypeId, UIA_TreeItemControlTypeId,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

        unsafe fn verify_tab_switched(
            hwnd: isize,
            sel_pat: &Option<IUIAutomationSelectionItemPattern>,
            kws: &[String],
            name: &str,
            wt_before: &str,
        ) -> bool {
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(90));
                let wt = window_title(hwnd);
                let sel_now = sel_pat
                    .as_ref()
                    .and_then(|s| s.CurrentIsSelected().ok())
                    .map(|b| b.as_bool())
                    .unwrap_or(false);
                if sel_now
                    || kws.iter().any(|kw| wt.contains(kw.as_str()))
                    || (!wt.is_empty() && wt != wt_before)
                    || (!name.is_empty() && wt.contains(name))
                {
                    return true;
                }
            }
            false
        }

        unsafe {
            let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let mut matched: Option<isize> = None;
            let mut total_tabs_seen: usize = 0;
            let mut gecko_checked = false;
            if let Ok(uia) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            {
                let mut conds: Vec<(i32, _)> = Vec::new();
                for ct in [
                    UIA_TabItemControlTypeId.0,
                    UIA_TreeItemControlTypeId.0,
                    UIA_ListItemControlTypeId.0,
                    UIA_ButtonControlTypeId.0,
                ] {
                    let v = make_uia_i4(ct);
                    if let Ok(c) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) {
                        conds.push((ct, c));
                    }
                }
                for attempt in 0..2 {
                    total_tabs_seen = 0;
                    for &(hwnd, _, _) in &bws {
                        let is_iconic =
                            windows::Win32::UI::WindowsAndMessaging::IsIconic(HWND(hwnd as *mut _))
                                .as_bool();
                        if attempt == 1 || is_iconic {
                            // Restore minimized browser or wake a lazy background browser UIA tree (Zen/Firefox).
                            if is_iconic {
                                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                                    HWND(hwnd as *mut _),
                                    windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                                );
                            }
                            win::focus_hwnd(hwnd);
                            std::thread::sleep(Duration::from_millis(240));
                            let tl = window_title(hwnd);
                            if kws.iter().any(|kw| tl.contains(kw.as_str()))
                                || (!host_clone.is_empty() && tl.contains(&host_clone))
                            {
                                matched = Some(hwnd);
                                total_tabs_seen = total_tabs_seen.max(1);
                                break;
                            }
                        }
                        let mut wr = RECT::default();
                        let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
                        let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) else {
                            continue;
                        };
                        for (ct, cond) in &conds {
                            let Ok(tabs) = root.FindAll(TreeScope_Descendants, cond) else {
                                continue;
                            };
                            let count = tabs.Length().unwrap_or(0).min(400);
                            for i in 0..count {
                                let Ok(tab) = tabs.GetElement(i) else {
                                    continue;
                                };
                                // Non-TabItem fallback controls (Zen vertical sidebar / pinned buttons)
                                // must live inside the browser chrome strip, never inside web page content.
                                let br_opt = tab.CurrentBoundingRectangle().ok();
                                if *ct != UIA_TabItemControlTypeId.0 {
                                    if let Some(br) = br_opt {
                                        let win_w = (wr.right - wr.left).max(400);
                                        let in_top_chrome = br.top <= wr.top + 220;
                                        let in_side_rail = br.right
                                            <= wr.left + ((win_w as f32 * 0.34).max(460.0) as i32)
                                            && br.top <= wr.bottom + 12;
                                        if !in_top_chrome && !in_side_rail {
                                            continue;
                                        }
                                    } else {
                                        continue;
                                    }
                                }
                                let name = tab
                                    .CurrentName()
                                    .map(|s| s.to_string().to_lowercase())
                                    .unwrap_or_default();
                                let help = tab
                                    .CurrentHelpText()
                                    .map(|s| s.to_string().to_lowercase())
                                    .unwrap_or_default();
                                let acc_pat = tab
                                    .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                                        UIA_LegacyIAccessiblePatternId,
                                    )
                                    .ok();
                                let (acc_name, acc_desc, acc_val) = acc_pat
                                    .as_ref()
                                    .map(|acc| {
                                        (
                                            acc.CurrentName()
                                                .map(|s| s.to_string().to_lowercase())
                                                .unwrap_or_default(),
                                            acc.CurrentDescription()
                                                .map(|s| s.to_string().to_lowercase())
                                                .unwrap_or_default(),
                                            acc.CurrentValue()
                                                .map(|s| s.to_string().to_lowercase())
                                                .unwrap_or_default(),
                                        )
                                    })
                                    .unwrap_or_default();

                                if name.is_empty()
                                    && help.is_empty()
                                    && acc_name.is_empty()
                                    && acc_desc.is_empty()
                                    && acc_val.is_empty()
                                {
                                    continue;
                                }
                                // Only count real UIA TabItem controls with a non-empty name
                                // so toolbar buttons do not inflate total_tabs_seen.
                                if *ct == UIA_TabItemControlTypeId.0 && !name.is_empty() {
                                    total_tabs_seen += 1;
                                }
                                let combined = format!(
                                    "{} {} {} {} {}",
                                    name, help, acc_name, acc_desc, acc_val
                                );
                                let hit = kws.iter().any(|kw| combined.contains(kw.as_str()))
                                    || (!host_clone.is_empty() && combined.contains(&host_clone));
                                if !hit {
                                    continue;
                                }

                                let wt_before = window_title(hwnd);
                                let sel_pat = tab
                                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                                        UIA_SelectionItemPatternId,
                                    )
                                    .ok();
                                if let Some(ref sel) = sel_pat {
                                    if sel.CurrentIsSelected().map(|b| b.as_bool()).unwrap_or(false)
                                    {
                                        win::focus_hwnd(hwnd);
                                        matched = Some(hwnd);
                                        break;
                                    }
                                    win::focus_hwnd(hwnd);
                                    if sel.Select().is_ok()
                                        && verify_tab_switched(
                                            hwnd, &sel_pat, &kws, &name, &wt_before,
                                        )
                                    {
                                        win::focus_hwnd(hwnd);
                                        matched = Some(hwnd);
                                        break;
                                    }
                                }

                                if let Ok(inv) = tab
                                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(
                                        UIA_InvokePatternId,
                                    )
                                {
                                    win::focus_hwnd(hwnd);
                                    if inv.Invoke().is_ok()
                                        && verify_tab_switched(
                                            hwnd, &sel_pat, &kws, &name, &wt_before,
                                        )
                                    {
                                        win::focus_hwnd(hwnd);
                                        matched = Some(hwnd);
                                        break;
                                    }
                                }

                                if let Some(ref acc) = acc_pat {
                                    win::focus_hwnd(hwnd);
                                    if acc.DoDefaultAction().is_ok()
                                        && verify_tab_switched(
                                            hwnd, &sel_pat, &kws, &name, &wt_before,
                                        )
                                    {
                                        win::focus_hwnd(hwnd);
                                        matched = Some(hwnd);
                                        break;
                                    }
                                }

                                // Real click fallback (needed for Zen Essentials / pinned icons
                                // where UIA Select/Invoke succeeds or is absent without switching).
                                if let Ok(scr) = tab
                                    .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(
                                        UIA_ScrollItemPatternId,
                                    )
                                {
                                    let _ = scr.ScrollIntoView();
                                    std::thread::sleep(Duration::from_millis(60));
                                }
                                let br_click = tab.CurrentBoundingRectangle().ok().or(br_opt);
                                if let Some(br) = br_click {
                                    let bw = br.right - br.left;
                                    let bh = br.bottom - br.top;
                                    if bw >= 8
                                        && bh >= 8
                                        && br.left >= wr.left - 12
                                        && br.right <= wr.right + 12
                                        && br.top >= wr.top - 12
                                        && br.bottom <= wr.bottom + 12
                                    {
                                        win::focus_hwnd(hwnd);
                                        std::thread::sleep(Duration::from_millis(80));
                                        if GetForegroundWindow().0 as isize == hwnd {
                                            let cx = ((br.left + br.right) / 2)
                                                .clamp(wr.left + 4, wr.right - 4);
                                            let cy = ((br.top + br.bottom) / 2)
                                                .clamp(wr.top + 4, wr.bottom - 4);
                                            mouse_click_screen_restore(cx, cy);
                                            if verify_tab_switched(
                                                hwnd, &sel_pat, &kws, &name, &wt_before,
                                            ) {
                                                matched = Some(hwnd);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if matched.is_some() {
                                break;
                            }
                        }
                        if matched.is_some() {
                            break;
                        }
                    }
                    if matched.is_some() {
                        break;
                    }
                }
            }
            if matched.is_none() && !host_clone.is_empty() {
                for &(hwnd, ref stem, _) in &bws {
                    if matches!(stem.as_str(), "zen" | "zenbrowser" | "firefox") {
                        let (sw, chk) = try_gecko_switch_to_tab_blocking(hwnd, &host_clone, &kws);
                        if chk {
                            gecko_checked = true;
                        }
                        if sw {
                            matched = Some(hwnd);
                            total_tabs_seen = total_tabs_seen.max(1);
                            break;
                        }
                    }
                }
            }
            if init_ok {
                CoUninitialize();
            }
            crate::backend::logfile::append(&format!(
                "[notif-tab] host='{}' matched={:?} tabs_seen={} gecko_checked={}",
                host_clone, matched, total_tabs_seen, gecko_checked
            ));
            (matched, total_tabs_seen, gecko_checked)
        }
    })
    .join()
    .unwrap_or((None, 0, false))
}

/// Types the destination URL into the address bar of the verified tab.
/// Ctrl+L focuses the omnibox in every supported browser; Enter is only
/// sent while that exact window is foreground. Never touches a chat input.
pub(crate) fn navigate_tab_to_url_blocking(hwnd: isize, url: &str) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, ShowWindow, SW_RESTORE};
    if hwnd == 0 || url.is_empty() {
        return false;
    }
    if unsafe { IsIconic(HWND(hwnd as *mut _)).as_bool() } {
        unsafe {
            let _ = ShowWindow(HWND(hwnd as *mut _), SW_RESTORE);
        }
        std::thread::sleep(Duration::from_millis(220));
    }
    win::focus_hwnd(hwnd);
    std::thread::sleep(Duration::from_millis(140));
    unsafe {
        if GetForegroundWindow().0 as isize != hwnd {
            return false;
        }
        if win::send_keys("ctrl+l").is_err() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(90));
        send_unicode_text(url);
        std::thread::sleep(Duration::from_millis(70));
        if GetForegroundWindow().0 as isize != hwnd {
            return false;
        }
        win::send_keys("enter").is_ok()
    }
}

fn strip_wa_count_suffix(s: &str) -> String {
    static CNT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = CNT_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\s*\(\d+(?:\s+(?:pesan|messages?|unread|baru|new))?\)\s*$").unwrap()
    });
    re.replace(s.trim(), "").trim().to_string()
}

pub fn wa_sender_variants(raw_sender: &str) -> Vec<String> {
    let cleaned = strip_wa_count_suffix(&clean_wa_sender(raw_sender));
    let mut out: Vec<String> = Vec::new();
    let mut push_unique = |cand: &str| {
        let t = strip_wa_count_suffix(&clean_wa_sender(cand));
        if !t.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
    };

    if let Some((before, after)) = cleaned.split_once(" @ ") {
        // Group notification: "fikar @ Koordinasi PDD X Acara" -> try group first.
        push_unique(after);
        push_unique(before);
    } else if let Some((before, _after)) = cleaned.split_once(": ") {
        // Group notification: "Koordinasi PDD X Acara: fikar" -> try group first.
        push_unique(before);
    }
    push_unique(&cleaned);
    out
}

fn wa_row_matches_sender(cleaned_row_name: &str, sender_low: &str) -> bool {
    let low = cleaned_row_name.to_lowercase();
    if low == sender_low {
        return true;
    }
    if let Some(rest) = low.strip_prefix(sender_low) {
        return rest
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || matches!(c, '\n' | '\r' | '\t' | ',' | ':' | '|'));
    }
    false
}

fn is_wa_chrome_text_ext(text_low: &str, variants_low: &[String], is_row: bool) -> bool {
    if is_row {
        const ROW_HEADER_PREFIXES: &[&str] = &[
            "unduh whatsapp",
            "dapatkan whatsapp",
            "get whatsapp",
            "whatsapp untuk",
            "whatsapp for",
            "cari atau mulai",
            "search or start",
            "tanya meta ai",
            "ask meta ai",
        ];
        if ROW_HEADER_PREFIXES.iter().any(|p| text_low.starts_with(p)) {
            return true;
        }
        let variant_has_wa = variants_low.iter().any(|v| v.contains("whatsapp"));
        return !variant_has_wa && text_low == "whatsapp";
    }
    const CHROME_PHRASES: &[&str] = &[
        "meta ai",
        "tanya meta",
        "ask meta",
        "unduh whatsapp",
        "dapatkan whatsapp",
        "get whatsapp",
        "whatsapp untuk",
        "whatsapp for",
        "untuk windows",
        "for windows",
        "cari atau mulai",
        "search or start",
        "mulai obrolan",
        "cari pesan",
        "belum dibaca",
        "favorit",
        "diarsipkan",
        "archived",
        "pesan berbintang",
        "saluran",
        "komunitas",
    ];
    if CHROME_PHRASES.iter().any(|p| text_low.contains(p)) {
        return true;
    }
    let variant_has_wa = variants_low.iter().any(|v| v.contains("whatsapp"));
    if !variant_has_wa && (text_low == "whatsapp" || text_low.contains("whatsapp")) {
        return true;
    }
    false
}

#[cfg(test)]
fn is_wa_chrome_text(text_low: &str, variants_low: &[String]) -> bool {
    is_wa_chrome_text_ext(text_low, variants_low, false)
}

unsafe fn find_verified_wa_doc(
    uia: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    hwnd: isize,
    wr: windows::Win32::Foundation::RECT,
) -> Option<(
    windows::Win32::UI::Accessibility::IUIAutomationElement,
    windows::Win32::Foundation::RECT,
)> {
    use windows::Win32::UI::Accessibility::{
        IUIAutomationValuePattern, TreeScope_Descendants, UIA_ControlTypePropertyId,
        UIA_DocumentControlTypeId, UIA_ValuePatternId,
    };

    let title_wa = window_title(hwnd).contains("whatsapp");
    let v_doc = make_uia_i4(UIA_DocumentControlTypeId.0);
    let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v_doc).ok()?;
    let docs = root.FindAll(TreeScope_Descendants, &cond).ok()?;
    let n = docs.Length().unwrap_or(0).min(24);
    let mut best_wa: Option<(
        i64,
        windows::Win32::UI::Accessibility::IUIAutomationElement,
        windows::Win32::Foundation::RECT,
    )> = None;
    let mut best_any: Option<(
        i64,
        windows::Win32::UI::Accessibility::IUIAutomationElement,
        windows::Win32::Foundation::RECT,
    )> = None;
    for i in 0..n {
        let Ok(doc) = docs.GetElement(i) else {
            continue;
        };
        if doc.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
            continue;
        }
        let Ok(br) = doc.CurrentBoundingRectangle() else {
            continue;
        };
        let bw = br.right - br.left;
        let bh = br.bottom - br.top;
        let overlaps =
            br.right > wr.left && br.left < wr.right && br.bottom > wr.top && br.top < wr.bottom;
        if bw < 300 || bh < 200 || !overlaps {
            continue;
        }
        let doc_name = doc
            .CurrentName()
            .map(|s| s.to_string().to_lowercase())
            .unwrap_or_default();
        let doc_val = doc
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()
            .and_then(|vp| vp.CurrentValue().ok())
            .map(|s| s.to_string().to_lowercase())
            .unwrap_or_default();
        let area = bw as i64 * bh as i64;
        if doc_name.contains("whatsapp") || doc_val.contains("whatsapp") {
            if best_wa.as_ref().map_or(true, |b| area > b.0) {
                best_wa = Some((area, doc, br));
            }
        } else if title_wa && best_any.as_ref().map_or(true, |b| area > b.0) {
            best_any = Some((area, doc, br));
        }
    }
    best_wa.or(best_any).map(|(_, d, r)| (d, r))
}

unsafe fn is_active_tab_whatsapp_uia(
    uia: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    hwnd: isize,
    wr: windows::Win32::Foundation::RECT,
) -> bool {
    if window_title(hwnd).contains("whatsapp") {
        return true;
    }
    find_verified_wa_doc(uia, root, hwnd, wr).is_some()
}

/// Picks the sender chat in the WhatsApp sidebar using the verified active
/// WhatsApp Web `Document` UIA subtree only, checking visible top contacts
/// first across 2 attempts before falling back to the WA search-box.
fn try_open_whatsapp_chat_blocking(hwnd: isize, raw_sender: &str, delay_ms: u64) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants,
        UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_DataItemControlTypeId,
        UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId,
        UIA_ListItemControlTypeId, UIA_PaneControlTypeId, UIA_TextControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    let variants = wa_sender_variants(raw_sender);
    if variants.is_empty() || hwnd == 0 {
        return false;
    }
    let variants_low: Vec<String> = variants.iter().map(|v| v.to_lowercase()).collect();
    let mut search_variants_low = variants_low.clone();
    for tok in variants[0].split_whitespace() {
        let t_low = tok.to_lowercase();
        if t_low.chars().count() >= 4 && !search_variants_low.contains(&t_low) {
            search_variants_low.push(t_low);
        }
    }
    let raw_sender_owned = raw_sender.to_string();

    std::thread::sleep(Duration::from_millis(delay_ms));
    win::focus_hwnd(hwnd);

    std::thread::spawn(move || unsafe {
        if GetForegroundWindow().0 as isize != hwnd {
            return false;
        }
        let mut wr = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut wr).is_err() {
            return false;
        }

        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut opened = false;

        #[allow(clippy::too_many_arguments)]
        unsafe fn scan_wa_sidebar(
            uia: &IUIAutomation,
            search_root: &IUIAutomationElement,
            variants_low: &[String],
            sidebar_min_x: i32,
            sidebar_max_x: i32,
            min_y: i32,
            max_y: i32,
            search_mode: bool,
        ) -> Option<(i32, i32)> {
            // (score, var_idx, top, cx, cy)
            let mut best: Option<(i32, usize, i32, i32, i32)> = None;
            for (ct, is_row) in [
                (UIA_ListItemControlTypeId.0, true),
                (UIA_DataItemControlTypeId.0, true),
                (UIA_GroupControlTypeId.0, true),
                (UIA_ButtonControlTypeId.0, true),
                (UIA_PaneControlTypeId.0, true),
                (UIA_TextControlTypeId.0, false),
                (UIA_HyperlinkControlTypeId.0, false),
            ] {
                let v = make_uia_i4(ct);
                let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                    continue;
                };
                let Ok(items) = search_root.FindAll(TreeScope_Descendants, &cond) else {
                    continue;
                };
                let n = items.Length().unwrap_or(0).min(600);
                for i in 0..n {
                    let Ok(el) = items.GetElement(i) else {
                        continue;
                    };
                    if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                        continue;
                    }
                    let name = el.CurrentName().map(|s| s.to_string()).unwrap_or_default();
                    let n_clean = clean_wa_sender(&name);
                    if n_clean.is_empty() {
                        continue;
                    }
                    let n_low = n_clean.to_lowercase();
                    if is_wa_chrome_text_ext(&n_low, variants_low, is_row) {
                        continue;
                    }
                    if search_mode
                        && !is_row
                        && (n_low.starts_with("cari ")
                            || n_low.starts_with("search ")
                            || n_low.starts_with("tanya "))
                    {
                        continue;
                    }
                    let mut matched_tier = 0u8;
                    let mut matched_var = usize::MAX;
                    for (vi, v_low) in variants_low.iter().enumerate() {
                        let tier = if n_low == *v_low {
                            3
                        } else if is_row && wa_row_matches_sender(&n_clean, v_low) {
                            2
                        } else if !is_row && n_low.starts_with(v_low.as_str()) {
                            2
                        } else if v_low.len() >= 3 && n_low.contains(v_low.as_str()) {
                            1
                        } else {
                            0
                        };
                        if tier > matched_tier {
                            matched_tier = tier;
                            matched_var = vi;
                            if tier == 3 && vi == 0 {
                                break;
                            }
                        }
                    }
                    if matched_tier == 0 {
                        continue;
                    }
                    let Ok(br) = el.CurrentBoundingRectangle() else {
                        continue;
                    };
                    let bw = br.right - br.left;
                    let bh = br.bottom - br.top;
                    if is_row {
                        if bw < 120 || bh < 32 || bh > 210 {
                            continue;
                        }
                    } else if bw < 8 || bh < 8 || bh > 96 {
                        continue;
                    }
                    let eff_min_y = if search_mode && !is_row {
                        min_y + 12
                    } else {
                        min_y
                    };
                    let score = (matched_tier as i32) * 2 + if is_row { 1 } else { 0 };
                    if br.left >= sidebar_min_x
                        && br.right <= sidebar_max_x
                        && br.top >= eff_min_y
                        && br.bottom <= max_y
                    {
                        let cx = if is_row {
                            (br.left + (((bw as f32 * 0.38).clamp(48.0, 180.0)) as i32))
                                .min(br.right - 20)
                        } else {
                            ((br.left + br.right) / 2)
                                .clamp(sidebar_min_x + 12, sidebar_max_x - 20)
                        };
                        let cy = (br.top + br.bottom) / 2;
                        let better = match best {
                            None => true,
                            Some((bs, bvi, btop, _, _)) => {
                                (score > bs)
                                    || (score == bs && matched_var < bvi)
                                    || (score == bs && matched_var == bvi && br.top < btop)
                            }
                        };
                        if better {
                            best = Some((score, matched_var, br.top, cx, cy));
                        }
                    }
                }
            }
            best.map(|(_, _, _, cx, cy)| (cx, cy))
        }

        if let Ok(uia) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        {
            for attempt in 0..3 {
                if attempt == 1 {
                    std::thread::sleep(Duration::from_millis(320));
                }
                let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) else {
                    continue;
                };
                if !is_active_tab_whatsapp_uia(&uia, &root, hwnd, wr) {
                    crate::backend::logfile::append(&format!(
                        "[wa-chat] aborted: active tab is not WhatsApp (title='{}')",
                        window_title(hwnd)
                    ));
                    break;
                }
                let Some((search_root, doc_r)) = find_verified_wa_doc(&uia, &root, hwnd, wr)
                else {
                    crate::backend::logfile::append(&format!(
                        "[wa-chat] aborted: no verified WhatsApp document (title='{}')",
                        window_title(hwnd)
                    ));
                    continue;
                };
                let doc_r_opt = Some(doc_r);
                let (sidebar_min_x, sidebar_max_x, content_min_y, content_max_y) = {
                    let doc_w = (doc_r.right - doc_r.left).max(400);
                    (
                        doc_r.left + 32,
                        doc_r.left + (doc_w as f32 * 0.56) as i32,
                        doc_r.top + 40,
                        doc_r.bottom - 12,
                    )
                };

                if attempt < 2 {
                    if let Some((cx, cy)) = scan_wa_sidebar(
                        &uia,
                        &search_root,
                        &variants_low,
                        sidebar_min_x,
                        sidebar_max_x,
                        content_min_y,
                        content_max_y,
                        false,
                    ) {
                        if GetForegroundWindow().0 as isize == hwnd {
                            mouse_click_screen_restore(cx, cy);
                            opened = true;
                            crate::backend::logfile::append(&format!(
                                "[wa-chat] pass1(try={}) matched sender='{}' at ({},{})",
                                attempt, raw_sender_owned, cx, cy
                            ));
                            break;
                        }
                    }
                    continue;
                }

                // Pass 2 (only after 2 full Pass-1 attempts missed): WA search-box fallback inside web Document
                if let Some(doc_r) = doc_r_opt {
                    let v_edit = make_uia_i4(UIA_EditControlTypeId.0);
                    if let Ok(cond) =
                        uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v_edit)
                    {
                        if let Ok(edits) = search_root.FindAll(TreeScope_Descendants, &cond) {
                            let n = edits.Length().unwrap_or(0).min(20);
                            for i in 0..n {
                                let Ok(ed) = edits.GetElement(i) else {
                                    continue;
                                };
                                let nm = ed
                                    .CurrentName()
                                    .map(|s| s.to_string().to_lowercase())
                                    .unwrap_or_default();
                                let is_wa_search = ["cari", "search", "obrolan", "chat"]
                                    .iter()
                                    .any(|k| nm.contains(k));
                                if !is_wa_search {
                                    continue;
                                }
                                let Ok(br) = ed.CurrentBoundingRectangle() else {
                                    continue;
                                };
                                let bw = br.right - br.left;
                                let bh = br.bottom - br.top;
                                if bw >= 80
                                    && bh >= 16
                                    && br.left >= sidebar_min_x
                                    && br.right <= sidebar_max_x
                                    && br.top >= doc_r.top + 25
                                    && br.top <= doc_r.top + 260
                                {
                                    let search_min_x = (br.left - 24).max(doc_r.left + 32);
                                    if GetForegroundWindow().0 as isize == hwnd {
                                        let _ = ed.SetFocus();
                                        let ecx = (br.left + br.right) / 2;
                                        let ecy = (br.top + br.bottom) / 2;
                                        mouse_click_screen_restore(ecx, ecy);
                                        std::thread::sleep(Duration::from_millis(80));
                                        let _ = win::send_keys("ctrl+a");
                                        std::thread::sleep(Duration::from_millis(30));
                                        send_unicode_text(&variants[0]);
                                        for wait_ms in [420u64, 380, 380] {
                                            std::thread::sleep(Duration::from_millis(wait_ms));
                                            if let Some((cx, cy)) = scan_wa_sidebar(
                                                &uia,
                                                &search_root,
                                                &search_variants_low,
                                                search_min_x,
                                                sidebar_max_x,
                                                br.bottom + 12,
                                                content_max_y,
                                                true,
                                            ) {
                                                if GetForegroundWindow().0 as isize == hwnd {
                                                    mouse_click_screen_restore(cx, cy);
                                                    opened = true;
                                                    crate::backend::logfile::append(&format!(
                                                        "[wa-chat] pass2 matched q='{}' at ({},{})",
                                                        variants[0], cx, cy
                                                    ));
                                                    break;
                                                }
                                            }
                                        }
                                        if !opened {
                                            crate::backend::logfile::append(&format!(
                                                "[wa-chat] pass2 miss for sender='{}'",
                                                variants[0]
                                            ));
                                            let _ = win::send_keys("esc");
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        if init_ok {
            CoUninitialize();
        }
        opened
    })
    .join()
    .unwrap_or(false)
}
pub async fn open_url_smart(raw_url: &str) -> Value {
    let url_str = raw_url.trim().to_string();
    if url_str.is_empty() {
        return json!({ "ok": false, "reason": "empty" });
    }
    let full_url = if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
        format!("https://{}", url_str)
    } else {
        url_str
    };

    tokio::task::spawn_blocking(move || {
        let (ok, reused, _, _) = focus_or_open_url_blocking(&full_url, "", "", &[], true);
        json!({ "ok": ok, "reused": reused })
    })
    .await
    .unwrap_or_else(|_| json!({ "ok": false }))
}

/// Opens a URL in the currently active running browser (e.g. Zen, Chrome, Edge)
/// reusing an exact matching tab if open, otherwise opening a new tab inside that browser.
pub async fn open_url_in_browser(
    raw_url: &str,
    preferred_browser: Option<&str>,
    reuse_domain: bool,
    navigate_existing: bool,
) -> Value {
    let url_str = raw_url.trim().to_string();
    if url_str.is_empty() {
        return json!({ "ok": false, "reason": "empty" });
    }
    let full_url = if !url_str.starts_with("http://") && !url_str.starts_with("https://") {
        format!("https://{}", url_str)
    } else {
        url_str
    };
    let pref_raw = preferred_browser.unwrap_or("").trim().to_string();
    tokio::task::spawn_blocking(move || {
        let explicit_tok = if pref_raw.is_empty() {
            String::new()
        } else {
            normalize_browser_token(&pref_raw)
        };
        let strict_browser = !explicit_tok.is_empty();
        let target_tok = if !explicit_tok.is_empty() {
            explicit_tok
        } else {
            let windows_list = unsafe { enum_visible_windows() };
            windows_list
                .iter()
                .find(|w| BROWSER_STEMS.contains(&w.stem.as_str()))
                .map(|w| w.stem.clone())
                .unwrap_or_default()
        };
        let (ok, reused, hwnd_opt, _) = focus_or_open_url_blocking_ext(
            &full_url,
            &target_tok,
            "",
            &[],
            reuse_domain,
            strict_browser,
        );
        let border_hwnd = hwnd_opt.or_else(|| {
            let wins = unsafe { enum_visible_windows() };
            wins.iter()
                .find(|w| {
                    (!target_tok.is_empty() && (w.stem == target_tok || w.stem.contains(&target_tok)))
                        || (target_tok.is_empty() && BROWSER_STEMS.contains(&w.stem.as_str()))
                })
                .map(|w| w.hwnd)
        });
        if let Some(h) = border_hwnd {
            if h != 0 {
                if unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsIconic(
                        windows::Win32::Foundation::HWND(h as *mut _),
                    )
                    .as_bool()
                } {
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                            windows::Win32::Foundation::HWND(h as *mut _),
                            windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                        );
                    }
                    win::focus_hwnd(h);
                    std::thread::sleep(Duration::from_millis(240));
                }
            }
        }
        let (is_pure_root, _dom_kws, specific_kws) = parse_url_tab_info(&full_url);
        let mut did_navigate = false;
        if reused && navigate_existing && (!is_pure_root || !specific_kws.is_empty()) {
            if let Some(h) = border_hwnd {
                did_navigate = navigate_tab_to_url_blocking(h, &full_url);
            }
        }
        json!({
            "ok": ok,
            "reused": reused,
            "navigated": did_navigate,
            "browser": target_tok,
            "hwnd": border_hwnd,
        })
    })
    .await
    .unwrap_or_else(|_| json!({ "ok": false }))
}

pub async fn open_url_in_active_browser(raw_url: &str, reuse_domain: bool) -> Value {
    open_url_in_browser(raw_url, None, reuse_domain, false).await
}

pub async fn focus_window(hint: &str) -> Value {
    let h = hint.trim().to_lowercase();
    if h.is_empty() {
        return json!({ "ok": false, "reason": "empty" });
    }
    let ok = tokio::task::spawn_blocking(move || {
        let list = unsafe { enum_visible_windows() };
        if let Some(w) = list
            .iter()
            .find(|w| w.title.to_lowercase().contains(&h) || w.stem.contains(&h))
        {
            return win::focus_hwnd(w.hwnd);
        }
        false
    })
    .await
    .unwrap_or(false);
    json!({ "ok": ok })
}

pub async fn focus_media_app(app_id: &str) -> Value {
    let raw = app_id.trim();
    if raw.is_empty() {
        return json!({ "ok": false, "reason": "empty" });
    }
    let tail = raw.split('!').next().unwrap_or(raw);
    let tok = tail
        .strip_suffix(".exe")
        .or_else(|| tail.strip_suffix(".EXE"))
        .unwrap_or(tail)
        .trim()
        .to_lowercase();
    if tok.is_empty() {
        return json!({ "ok": false, "reason": "empty" });
    }

    let ok = tokio::task::spawn_blocking(move || {
        let list = unsafe { enum_visible_windows() };
        if let Some(w) = list.iter().find(|w| w.stem == tok || w.stem.contains(&tok)) {
            return win::focus_hwnd(w.hwnd);
        }
        if let Some(w) = list.iter().find(|w| w.title.to_lowercase().contains(&tok)) {
            return win::focus_hwnd(w.hwnd);
        }
        false
    })
    .await
    .unwrap_or(false);
    json!({ "ok": ok })
}

pub fn is_safe_protocol_uri(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty()
        || t.chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>'))
    {
        return false;
    }
    let Some((scheme_raw, rest)) = t.split_once(':') else {
        return false;
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return false;
    }
    if matches!(
        scheme.as_str(),
        "http"
            | "https"
            | "file"
            | "javascript"
            | "vbscript"
            | "data"
            | "cmd"
            | "powershell"
            | "ms-msdt"
            | "search-ms"
            | "ms-appinstaller"
            | "ms-officecmd"
            | "mk"
            | "res"
            | "its"
            | "mhtml"
    ) {
        return false;
    }
    rest.starts_with("//") || scheme == "ms-settings"
}

pub fn extract_app_focus_tokens(raw_app: &str, title: &str) -> Vec<String> {
    const BOILERPLATE: &[&str] = &[
        "electron",
        "app",
        "com",
        "org",
        "net",
        "dev",
        "io",
        "windows",
        "microsoft",
        "systemtoast",
        "desktop",
        "client",
        "win32",
        "handler",
    ];
    const GENERIC_TITLES: &[&str] = &[
        "task completed",
        "new reward available",
        "your confirmation is needed",
        "plan awaiting approval",
    ];
    let mut out = Vec::new();
    let mut push_tok = |s: &str| {
        let low = s.trim().to_lowercase();
        if low.len() >= 2 && !out.iter().any(|x| x == &low) {
            out.push(low);
        }
    };

    for part in raw_app
        .split(['!', '\\', '/', '.', '_', '-', ' ', '\t'])
        .map(|p| {
            p.strip_suffix(".exe")
                .or_else(|| p.strip_suffix(".EXE"))
                .unwrap_or(p)
                .trim()
                .to_lowercase()
        })
    {
        if part.len() < 2 || BOILERPLATE.contains(&part.as_str()) {
            continue;
        }
        push_tok(&part);
        if part == "visualstudiocode" {
            push_tok("code");
        } else if part == "nvapp" {
            push_tok("nvidia");
        }
    }

    let first_seg = title
        .split(['—', '-', '|'])
        .next()
        .unwrap_or(title)
        .trim();
    let title_low = first_seg.to_lowercase();
    if title_low.len() >= 2
        && !GENERIC_TITLES.iter().any(|g| title_low == *g || title_low.starts_with(g))
    {
        push_tok(&title_low);
    }
    out
}

pub async fn notif_open_rich(
    raw_app: &str,
    launch: &str,
    url: &str,
    profile: &str,
    sender_title: &str,
) -> Value {
    let raw_a = raw_app.trim().to_string();
    let launch_s = launch.trim().to_string();
    // Browser web-push toasts (Zen/Firefox/Chrome...): do NOT replay the toast
    // through its COM activator. Gecko's notificationserver re-launches the
    // browser with the toast `launchUrl` whenever the original alert is no
    // longer live, which opens a NEW bare https://web.whatsapp.com tab every
    // click (and WhatsApp then shows the red "open in another window" error in
    // the existing tab). Route through the existing tab + UI Automation
    // sidebar click instead (the path that reliably opened the sender chat).
    let is_browser_toast = {
        let tok = normalize_browser_token(&raw_a);
        !tok.is_empty() && BROWSER_STEMS.contains(&tok.as_str())
    };
    if !raw_a.is_empty() && !launch_s.is_empty() && !is_browser_toast {
        let tok = normalize_browser_token(&raw_a);
        let (_, domain_kws, _) = parse_url_tab_info(url);
        let ra = raw_a.clone();
        let ln = launch_s.clone();
        let activated = tokio::task::spawn_blocking(move || {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let fg_before = unsafe { GetForegroundWindow().0 as isize };
            let target_hwnds: Vec<isize> = if !tok.is_empty() {
                unsafe { enum_visible_windows() }
                    .into_iter()
                    .filter(|w| w.stem == tok || w.stem.contains(&tok))
                    .map(|w| w.hwnd)
                    .collect()
            } else {
                Vec::new()
            };
            if !activate_toast_com(&ra, &ln, None) {
                crate::backend::logfile::append(&format!(
                    "[notif] COM activate_toast_com returned false raw_app='{}'",
                    ra
                ));
                return false;
            }
            for _ in 0..8 {
                std::thread::sleep(Duration::from_millis(50));
                let fg_after = unsafe { GetForegroundWindow().0 as isize };
                let switched_focus = fg_after != 0
                    && fg_after != fg_before
                    && (target_hwnds.is_empty() || target_hwnds.contains(&fg_after));
                let title_ok = !target_hwnds.is_empty()
                    && target_hwnds.contains(&fg_after)
                    && (domain_kws.is_empty()
                        || domain_kws.iter().any(|k| window_title(fg_after).contains(k)));
                if switched_focus || title_ok {
                    return true;
                }
            }
            crate::backend::logfile::append(&format!(
                "[notif] COM returned S_OK but target did not focus/switch (raw_app='{}', domains={:?}); falling back to UIA",
                ra, domain_kws
            ));
            false
        })
        .await
        .unwrap_or(false);
        if activated {
            return json!({ "ok": true, "outcome": "activated", "com": true });
        }
        if is_safe_protocol_uri(&launch_s) {
            let ln = launch_s.clone();
            let ok = tokio::task::spawn_blocking(move || open_uri_via_shell(&ln))
                .await
                .unwrap_or(false);
            if ok {
                return json!({ "ok": true, "outcome": "opened", "protocol": true });
            }
        }
    }
    notif_open_chat(&raw_a, url, profile, sender_title).await
}

pub async fn notif_send_reply(
    raw_app: &str,
    args: &str,
    input_id: &str,
    reply_text: &str,
) -> bool {
    let ra = raw_app.trim().to_string();
    let ar = args.to_string();
    let iid = input_id.to_string();
    let txt = reply_text.to_string();
    if ra.is_empty() || iid.is_empty() || txt.trim().is_empty() {
        return false;
    }
    tokio::task::spawn_blocking(move || activate_toast_com(&ra, &ar, Some((&iid, &txt))))
        .await
        .unwrap_or(false)
}

pub(crate) fn is_zapfast_hint(raw_app: &str) -> bool {
    raw_app.to_lowercase().contains("zapfast")
}

pub(crate) fn clean_zapfast_chat_query(raw_sender: &str) -> String {
    let cleaned = clean_wa_sender(raw_sender);
    let low = cleaned.to_lowercase();
    if low.is_empty()
        || matches!(
            low.as_str(),
            "zapfast" | "whatsapp" | "pesan baru" | "new message" | "notification"
        )
    {
        return String::new();
    }
    cleaned
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZapFastPane {
    LeftSidebar,
    ChatHeader,
    ChatComposerZone,
    Other,
}

pub(crate) fn zapfast_is_composer_name(name_lower: &str) -> bool {
    let s = name_lower.trim();
    if s.is_empty() {
        return false;
    }
    // Search boxes often say "Cari pesan" / "Search messages"; never misclassify those as composer.
    if ["cari", "search", "filter", "temukan", "find"]
        .iter()
        .any(|k| s.contains(k))
    {
        return false;
    }
    const COMPOSER_PHRASES: &[&str] = &[
        "ketik pesan",
        "ketik sebuah pesan",
        "tulis pesan",
        "kirim pesan",
        "type a message",
        "write a message",
        "send a message",
        "type message",
        "message input",
        "chat input",
        "balas",
        "reply",
        "compose",
    ];
    if COMPOSER_PHRASES.iter().any(|p| s.contains(p)) {
        return true;
    }
    matches!(s, "pesan" | "message")
}

pub(crate) fn zapfast_is_search_name(name_lower: &str) -> bool {
    let s = name_lower.trim();
    if s.is_empty() || zapfast_is_composer_name(s) {
        return false;
    }
    const SEARCH_CUES: &[&str] = &[
        "cari",
        "search",
        "filter",
        "temukan",
        "find",
        "quick switcher",
        "jump to",
        "loncat ke",
        "mulai obrolan",
        "start chat",
        "new chat",
    ];
    SEARCH_CUES.iter().any(|c| s.contains(c))
}

pub(crate) fn zapfast_pane_role(
    el_rect: windows::Win32::Foundation::RECT,
    win_rect: windows::Win32::Foundation::RECT,
) -> ZapFastPane {
    let win_w = (win_rect.right - win_rect.left).max(1);
    let win_h = (win_rect.bottom - win_rect.top).max(1);
    let sidebar_max_x = win_rect.left + (win_w as f32 * 0.44) as i32;
    let right_pane_min_x = win_rect.left + (win_w as f32 * 0.28) as i32;
    let composer_min_y = win_rect.top + (win_h as f32 * 0.55) as i32;

    if el_rect.right <= sidebar_max_x {
        ZapFastPane::LeftSidebar
    } else if el_rect.left >= right_pane_min_x
        && el_rect.bottom >= win_rect.bottom - 170
        && el_rect.top >= composer_min_y
    {
        ZapFastPane::ChatComposerZone
    } else if el_rect.left >= right_pane_min_x && el_rect.top <= win_rect.top + 145 {
        ZapFastPane::ChatHeader
    } else {
        ZapFastPane::Other
    }
}

pub(crate) fn zapfast_text_matches_contact(text: &str, variants_low: &[String]) -> bool {
    let cleaned = clean_wa_sender(text);
    if cleaned.is_empty() {
        return false;
    }
    let low = cleaned.to_lowercase();
    if is_wa_chrome_text_ext(&low, variants_low, false) {
        return false;
    }
    for v_low in variants_low {
        if v_low.is_empty() {
            continue;
        }
        if low == *v_low || wa_row_matches_sender(&cleaned, v_low) {
            return true;
        }
        if v_low.chars().count() >= 4 && low.contains(v_low.as_str()) {
            return true;
        }
    }
    false
}

fn rects_overlap_or_near(
    a: windows::Win32::Foundation::RECT,
    b: windows::Win32::Foundation::RECT,
    pad: i32,
) -> bool {
    a.right >= b.left - pad
        && a.left <= b.right + pad
        && a.bottom >= b.top - pad
        && a.top <= b.bottom + pad
}

fn focus_zapfast_window_only(hint: &str) -> bool {
    let mut brought = false;
    let list = unsafe { enum_visible_windows() };
    if let Some(w) = list.iter().find(|w| {
        w.stem == "zapfast"
            || w.stem.contains("zapfast")
            || w.title.to_lowercase().contains("zapfast")
    }) {
        brought = win::focus_hwnd(w.hwnd);
    }
    if !brought {
        brought = launch_app(hint, "zapfast");
    }
    if !brought {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let exe = std::path::PathBuf::from(local)
                .join("Programs")
                .join("ZapFast")
                .join("zapfast.exe");
            if exe.exists() {
                brought = open_uri_via_shell(&exe.to_string_lossy());
            }
        }
    }
    brought
}

fn open_zapfast_chat_blocking(hint: &str, sender_title: &str) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants,
        UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_DataItemControlTypeId,
        UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId,
        UIA_ListItemControlTypeId, UIA_PaneControlTypeId, UIA_TextControlTypeId,
        UIA_TreeItemControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    let brought = focus_zapfast_window_only(hint);
    let mut focused = false;
    for _ in 0..30 {
        if foreground_is_process_stem("zapfast") {
            focused = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(60));
    }
    if !focused {
        crate::backend::logfile::append(
            "[notif-zapfast] window did not focus in time; skipping keystrokes",
        );
        return brought;
    }

    let query = clean_zapfast_chat_query(sender_title);
    if query.is_empty() {
        return true;
    }
    let variants = wa_sender_variants(&query);
    if variants.is_empty() {
        return true;
    }
    let variants_low: Vec<String> = variants.iter().map(|v| v.to_lowercase()).collect();

    std::thread::sleep(Duration::from_millis(120));
    let hwnd = unsafe { GetForegroundWindow().0 as isize };
    if hwnd == 0 || !foreground_is_process_stem("zapfast") {
        return true;
    }
    let mut wr = RECT::default();
    if unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut wr) }.is_err() {
        return true;
    }

    unsafe {
        unsafe fn scan_zapfast_sidebar(
            uia: &IUIAutomation,
            root: &IUIAutomationElement,
            wr: RECT,
            variants_low: &[String],
            min_y: i32,
        ) -> Option<(i32, i32)> {
            let mut best: Option<(i32, i32, i32, i32)> = None;
            for (ct, is_row) in [
                (UIA_ListItemControlTypeId.0, true),
                (UIA_DataItemControlTypeId.0, true),
                (UIA_TreeItemControlTypeId.0, true),
                (UIA_ButtonControlTypeId.0, true),
                (UIA_GroupControlTypeId.0, true),
                (UIA_PaneControlTypeId.0, true),
                (UIA_TextControlTypeId.0, false),
                (UIA_HyperlinkControlTypeId.0, false),
            ] {
                let v = make_uia_i4(ct);
                let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                    continue;
                };
                let Ok(items) = root.FindAll(TreeScope_Descendants, &cond) else {
                    continue;
                };
                let n = items.Length().unwrap_or(0).min(500);
                for i in 0..n {
                    let Ok(el) = items.GetElement(i) else {
                        continue;
                    };
                    if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                        continue;
                    }
                    let Ok(br) = el.CurrentBoundingRectangle() else {
                        continue;
                    };
                    if zapfast_pane_role(br, wr) != ZapFastPane::LeftSidebar {
                        continue;
                    }
                    let bw = br.right - br.left;
                    let bh = br.bottom - br.top;
                    if br.top < min_y || br.bottom > wr.bottom - 8 {
                        continue;
                    }
                    if is_row {
                        if bw < 90 || bh < 24 || bh > 190 {
                            continue;
                        }
                    } else if bw < 8 || bh < 8 || bh > 96 {
                        continue;
                    }
                    let name = el.CurrentName().map(|s| s.to_string()).unwrap_or_default();
                    let n_clean = clean_wa_sender(&name);
                    if n_clean.is_empty() {
                        continue;
                    }
                    let n_low = n_clean.to_lowercase();
                    if is_wa_chrome_text_ext(&n_low, variants_low, is_row)
                        || zapfast_is_search_name(&n_low)
                        || zapfast_is_composer_name(&n_low)
                    {
                        continue;
                    }
                    if !zapfast_text_matches_contact(&n_clean, variants_low) {
                        continue;
                    }
                    let exact = variants_low.iter().any(|v| n_low == *v || wa_row_matches_sender(&n_clean, v));
                    let score = if exact { 4 } else { 2 } + if is_row { 1 } else { 0 };
                    let cx = (br.left + bw / 2).clamp(wr.left + 12, wr.right - 12);
                    let cy = (br.top + bh / 2).clamp(wr.top + 12, wr.bottom - 12);
                    if best.map_or(true, |(bs, btop, _, _)| score > bs || (score == bs && br.top < btop)) {
                        best = Some((score, br.top, cx, cy));
                    }
                }
            }
            best.map(|(_, _, cx, cy)| (cx, cy))
        }

        unsafe fn check_already_in_chat(
            uia: &IUIAutomation,
            root: &IUIAutomationElement,
            wr: RECT,
            variants_low: &[String],
        ) -> bool {
            for ct in [
                UIA_TextControlTypeId.0,
                UIA_ButtonControlTypeId.0,
                UIA_GroupControlTypeId.0,
                UIA_HyperlinkControlTypeId.0,
            ] {
                let v = make_uia_i4(ct);
                let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                    continue;
                };
                let Ok(items) = root.FindAll(TreeScope_Descendants, &cond) else {
                    continue;
                };
                let n = items.Length().unwrap_or(0).min(240);
                for i in 0..n {
                    let Ok(el) = items.GetElement(i) else {
                        continue;
                    };
                    if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                        continue;
                    }
                    let Ok(br) = el.CurrentBoundingRectangle() else {
                        continue;
                    };
                    if zapfast_pane_role(br, wr) != ZapFastPane::ChatHeader {
                        continue;
                    }
                    let name = el.CurrentName().map(|s| s.to_string()).unwrap_or_default();
                    if zapfast_text_matches_contact(&name, variants_low) {
                        return true;
                    }
                }
            }
            false
        }

        unsafe fn find_verified_search_edit(
            uia: &IUIAutomation,
            root: &IUIAutomationElement,
            wr: RECT,
        ) -> (Option<(IUIAutomationElement, RECT)>, bool) {
            let mut in_open_chat = false;
            let mut best: Option<(i32, IUIAutomationElement, RECT)> = None;
            let v_edit = make_uia_i4(UIA_EditControlTypeId.0);
            let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v_edit) else {
                return (None, false);
            };
            let Ok(edits) = root.FindAll(TreeScope_Descendants, &cond) else {
                return (None, false);
            };
            let n = edits.Length().unwrap_or(0).min(30);
            for i in 0..n {
                let Ok(ed) = edits.GetElement(i) else {
                    continue;
                };
                if ed.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                    continue;
                }
                let Ok(br) = ed.CurrentBoundingRectangle() else {
                    continue;
                };
                let bw = br.right - br.left;
                let bh = br.bottom - br.top;
                let nm = ed
                    .CurrentName()
                    .map(|s| s.to_string().to_lowercase())
                    .unwrap_or_default();
                let pane = zapfast_pane_role(br, wr);
                if pane == ZapFastPane::ChatComposerZone || zapfast_is_composer_name(&nm) {
                    in_open_chat = true;
                    continue;
                }
                if bw < 70 || bh < 16 {
                    continue;
                }
                let named_search = zapfast_is_search_name(&nm);
                let safe_pos = pane == ZapFastPane::LeftSidebar || br.top <= wr.top + 160;
                if named_search || (pane == ZapFastPane::LeftSidebar && br.top <= wr.top + 240 && nm.is_empty()) {
                    let score = if named_search { 3 } else { 1 } + if safe_pos { 1 } else { 0 };
                    if best.as_ref().map_or(true, |(bs, _, _)| score > *bs) {
                        best = Some((score, ed, br));
                    }
                }
            }
            (best.map(|(_, e, r)| (e, r)), in_open_chat)
        }

        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut handled = false;

        if let Ok(uia) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        {
            if let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) {
                // 1. Already viewing the target contact's chat? Do nothing to avoid touching composer.
                if check_already_in_chat(&uia, &root, wr, &variants_low) {
                    crate::backend::logfile::append(&format!(
                        "[notif-zapfast] already in target chat query='{}'",
                        variants[0]
                    ));
                    handled = true;
                }

                // 2. Contact visible directly in LeftSidebar? Click it without typing anything.
                if !handled {
                    if let Some((cx, cy)) =
                        scan_zapfast_sidebar(&uia, &root, wr, &variants_low, wr.top + 40)
                    {
                        if GetForegroundWindow().0 as isize == hwnd {
                            mouse_click_screen_restore(cx, cy);
                            crate::backend::logfile::append(&format!(
                                "[notif-zapfast] clicked sidebar contact query='{}' at ({},{})",
                                variants[0], cx, cy
                            ));
                            handled = true;
                        }
                    }
                }

                // 3. Dedicated verified Search Edit only — never focus/type in message composer or send Enter.
                if !handled {
                    let (mut search_opt, in_open_chat) =
                        find_verified_search_edit(&uia, &root, wr);

                    if search_opt.is_none() {
                        // Look for an explicit Search toggle button in LeftSidebar/top bar first.
                        let v_btn = make_uia_i4(UIA_ButtonControlTypeId.0);
                        if let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v_btn) {
                            if let Ok(btns) = root.FindAll(TreeScope_Descendants, &cond) {
                                let n = btns.Length().unwrap_or(0).min(120);
                                for i in 0..n {
                                    let Ok(b) = btns.GetElement(i) else { continue };
                                    if b.CurrentIsOffscreen().map(|v| v.as_bool()).unwrap_or(false) {
                                        continue;
                                    }
                                    let Ok(br) = b.CurrentBoundingRectangle() else { continue };
                                    let pane = zapfast_pane_role(br, wr);
                                    if pane == ZapFastPane::ChatComposerZone {
                                        continue;
                                    }
                                    let nm = b
                                        .CurrentName()
                                        .map(|s| s.to_string().to_lowercase())
                                        .unwrap_or_default();
                                    if (pane == ZapFastPane::LeftSidebar || br.top <= wr.top + 160)
                                        && zapfast_is_search_name(&nm)
                                        && GetForegroundWindow().0 as isize == hwnd
                                    {
                                        let cx = (br.left + br.right) / 2;
                                        let cy = (br.top + br.bottom) / 2;
                                        mouse_click_screen_restore(cx, cy);
                                        std::thread::sleep(Duration::from_millis(160));
                                        let (re_search, _) = find_verified_search_edit(&uia, &root, wr);
                                        search_opt = re_search;
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    if let Some((ed, s_rect)) = search_opt {
                        if GetForegroundWindow().0 as isize == hwnd {
                            let _ = ed.SetFocus();
                            let ecx = (s_rect.left + s_rect.right) / 2;
                            let ecy = (s_rect.top + s_rect.bottom) / 2;
                            mouse_click_screen_restore(ecx, ecy);
                            std::thread::sleep(Duration::from_millis(90));

                            let verified_focus = uia
                                .GetFocusedElement()
                                .ok()
                                .and_then(|fe| {
                                    let f_name = fe
                                        .CurrentName()
                                        .map(|s| s.to_string().to_lowercase())
                                        .unwrap_or_default();
                                    let f_rect = fe.CurrentBoundingRectangle().ok()?;
                                    let f_pane = zapfast_pane_role(f_rect, wr);
                                    if f_pane == ZapFastPane::ChatComposerZone
                                        || zapfast_is_composer_name(&f_name)
                                    {
                                        return Some(false);
                                    }
                                    Some(
                                        rects_overlap_or_near(f_rect, s_rect, 24)
                                            || zapfast_is_search_name(&f_name),
                                    )
                                })
                                .unwrap_or(false);

                            if verified_focus && GetForegroundWindow().0 as isize == hwnd {
                                let _ = win::send_keys("ctrl+a");
                                std::thread::sleep(Duration::from_millis(25));
                                let _ = win::send_keys("backspace");
                                std::thread::sleep(Duration::from_millis(30));
                                send_unicode_text(&variants[0]);

                                for wait_ms in [380u64, 380] {
                                    std::thread::sleep(Duration::from_millis(wait_ms));
                                    if GetForegroundWindow().0 as isize != hwnd {
                                        break;
                                    }
                                    if let Some((cx, cy)) = scan_zapfast_sidebar(
                                        &uia,
                                        &root,
                                        wr,
                                        &variants_low,
                                        s_rect.bottom + 6,
                                    ) {
                                        mouse_click_screen_restore(cx, cy);
                                        handled = true;
                                        crate::backend::logfile::append(&format!(
                                            "[notif-zapfast] search clicked result query='{}' at ({},{})",
                                            variants[0], cx, cy
                                        ));
                                        break;
                                    }
                                }
                                if !handled {
                                    crate::backend::logfile::append(&format!(
                                        "[notif-zapfast] search found no clickable row for '{}'; leaving without Enter",
                                        variants[0]
                                    ));
                                    if uia.GetFocusedElement().ok().is_some_and(|fe| {
                                        let nm = fe
                                            .CurrentName()
                                            .map(|s| s.to_string().to_lowercase())
                                            .unwrap_or_default();
                                        let rp = fe
                                            .CurrentBoundingRectangle()
                                            .ok()
                                            .map(|r| zapfast_pane_role(r, wr));
                                        !zapfast_is_composer_name(&nm)
                                            && rp != Some(ZapFastPane::ChatComposerZone)
                                    }) {
                                        let _ = win::send_keys("esc");
                                    }
                                }
                            } else {
                                crate::backend::logfile::append(&format!(
                                    "[notif-zapfast] blocked typing: search focus check failed (in_open_chat={})",
                                    in_open_chat
                                ));
                            }
                        }
                    } else {
                        crate::backend::logfile::append(&format!(
                            "[notif-zapfast] safe focus-only fallback (in_open_chat={}, query='{}')",
                            in_open_chat, variants[0]
                        ));
                    }
                }
            }
        }

        if init_ok {
            CoUninitialize();
        }
    }
    true
}

pub async fn notif_open_chat(
    hint: &str,
    url: &str,
    profile: &str,
    sender_title: &str,
) -> Value {
    let h = hint.trim().to_string();
    let raw_u = url
        .trim()
        .strip_prefix("via ")
        .or_else(|| url.trim().strip_prefix("Via "))
        .unwrap_or(url.trim())
        .trim()
        .to_string();
    let prof = profile.trim().to_string();
    let sender = sender_title.trim().to_string();

    if h.is_empty() && raw_u.is_empty() {
        return json!({ "ok": false, "outcome": "unavailable", "reason": "empty" });
    }

    let full_url = if !raw_u.is_empty() && !raw_u.contains(' ') {
        if raw_u.starts_with("http://") || raw_u.starts_with("https://") {
            raw_u
        } else {
            format!("https://{}", raw_u)
        }
    } else {
        String::new()
    };

    if full_url.is_empty() {
        if is_zapfast_hint(&h) {
            let ok = tokio::task::spawn_blocking(move || open_zapfast_chat_blocking(&h, &sender))
                .await
                .unwrap_or(false);
            return json!({
                "ok": ok,
                "outcome": if ok { "focused" } else { "unavailable" },
                "zapfast": true
            });
        }
        let ok = tokio::task::spawn_blocking(move || {
            let tokens = extract_app_focus_tokens(&h, &sender);
            let list = unsafe { enum_visible_windows() };
            for tok in &tokens {
                if let Some(w) = list.iter().find(|w| w.stem == *tok) {
                    if win::focus_hwnd(w.hwnd) {
                        return true;
                    }
                }
            }
            for tok in &tokens {
                if let Some(w) = list.iter().find(|w| w.stem.contains(tok.as_str())) {
                    if win::focus_hwnd(w.hwnd) {
                        return true;
                    }
                }
            }
            for tok in &tokens {
                if let Some(w) = list.iter().find(|w| w.title.to_lowercase().contains(tok.as_str())) {
                    if win::focus_hwnd(w.hwnd) {
                        return true;
                    }
                }
            }
            let first_tok = tokens.first().map(String::as_str).unwrap_or("");
            launch_app(&h, first_tok)
        })
        .await
        .unwrap_or(false);
        return json!({
            "ok": ok,
            "outcome": if ok { "focused" } else { "unavailable" }
        });
    }

    let tok = normalize_browser_token(&h);
    let sender_clean = clean_wa_sender(&sender);
    tokio::task::spawn_blocking(move || notif_open_url_blocking(&full_url, &tok, &prof, &sender_clean))
        .await
        .unwrap_or_else(|_| json!({ "ok": false, "outcome": "unavailable" }))
}

/// Notification body-open routing, on a blocking worker:
/// 1. reuse the verified site tab in the source browser/profile and navigate
///    THAT tab to the exact destination (no new tab);
/// 2. WhatsApp without a deep link: pick the sender chat only on a unique
///    exact sidebar match;
/// 3. no verifiable tab + specific destination URL: spawn the SOURCE browser
///    executable with the URL (firefox/zen remoting needs `-profile <path>`
///    to reach the running instance) — a new tab, but it reliably opens the
///    chat. Zen with accessibility off exposes no tab tree, so this is the
///    common path there;
/// 4. no verifiable tab + only a site root URL: focus the source browser
///    only — never launch a duplicate root tab;
/// 5. no hinted browser window: open the destination via the shell; a
///    hinted-but-not-running browser is never substituted by another one.
fn notif_open_url_blocking(full_url: &str, tok: &str, profile: &str, sender: &str) -> Value {
    let (is_root, domain_kws, specific_kws) = parse_url_tab_info(full_url);
    let site_host = extract_url_host(full_url);
    let url_low = full_url.to_lowercase();
    let is_wa = url_low.contains("whatsapp");
    // A destination beyond the site root: any non-root path/query (WA
    // send?phone= deeplink, a mail message, a meeting link...). A bare root
    // means there is no specific chat/page to open.
    let has_specific_dest = !is_root || !specific_kws.is_empty();
    let windows_list = unsafe { enum_visible_windows() };
    let browsers: Vec<WindowEnumItem> = windows_list
        .into_iter()
        .filter(|w| BROWSER_STEMS.contains(&w.stem.as_str()))
        .collect();
    let mut ordered: Vec<WindowEnumItem> = Vec::new();
    if !tok.is_empty() {
        for bw in browsers.iter().filter(|w| w.stem == tok || w.stem.contains(tok)) {
            ordered.push(WindowEnumItem { hwnd: bw.hwnd, title: bw.title.clone(), stem: bw.stem.clone(), exe_path: bw.exe_path.clone() });
        }
    }
    for bw in &browsers {
        if !ordered.iter().any(|o| o.hwnd == bw.hwnd) {
            ordered.push(WindowEnumItem { hwnd: bw.hwnd, title: bw.title.clone(), stem: bw.stem.clone(), exe_path: bw.exe_path.clone() });
        }
    }

    // Source browser first: when the notification came from Zen we must not
    // hijack a Chrome tab just because Zen does not expose its tab tree.
    let preferred: Vec<(isize, String, String)> = if !tok.is_empty() {
        ordered
            .iter()
            .filter(|b| b.stem == tok || b.stem.contains(tok))
            .map(|b| (b.hwnd, b.stem.clone(), b.title.clone()))
            .collect()
    } else {
        Vec::new()
    };
    let (mut hwnd_opt, mut tabs_seen, mut gecko_checked) =
        select_site_tab_blocking(&preferred, &domain_kws, &site_host);
    if hwnd_opt.is_none() && (preferred.is_empty() || tabs_seen > 0) {
        let candidates: Vec<(isize, String, String)> = ordered
            .iter()
            .filter(|b| !preferred.iter().any(|(ph, _, _)| *ph == b.hwnd))
            .map(|b| (b.hwnd, b.stem.clone(), b.title.clone()))
            .collect();
        if !candidates.is_empty() {
            let (h2, t2, g2) = select_site_tab_blocking(&candidates, &domain_kws, &site_host);
            if h2.is_some() {
                hwnd_opt = h2;
                tabs_seen += t2;
                gecko_checked |= g2;
            } else if preferred.is_empty() {
                tabs_seen = t2;
                gecko_checked |= g2;
            }
        }
    }
    if let Some(hwnd) = hwnd_opt {
        win::focus_hwnd(hwnd);
        // WhatsApp sets the tab title to the open chat. If the sender is
        // already the open chat, do not reload — a draft must survive.
        let sender_low = sender.to_lowercase();
        let already_in_chat =
            is_wa && !sender_low.is_empty() && window_title(hwnd).contains(&sender_low);
        if already_in_chat {
            return json!({ "ok": true, "outcome": "focused", "chat_opened": true, "reused": true });
        }
        let nav_ok = has_specific_dest && navigate_tab_to_url_blocking(hwnd, full_url);
        let chat_ok = !has_specific_dest
            && is_wa
            && !sender.is_empty()
            && try_open_whatsapp_chat_blocking(hwnd, sender, 220);
        let outcome = route_decision(true, has_specific_dest, is_wa, !sender.is_empty(), nav_ok, chat_ok, true, "none");
        let mut res = json!({ "ok": true, "outcome": outcome, "reused": true });
        if chat_ok {
            res["chat_opened"] = json!(true);
        }
        return res;
    }

    // No verifiable site tab. With a specific destination, spawn the SOURCE
    // browser exe with the URL — the Tauri-era approach that reliably opened
    // the chat even when Zen exposes no UIA tab tree. Never substitute
    // another browser for a hinted one.
    let hinted: Option<&WindowEnumItem> = if tok.is_empty() {
        None
    } else {
        ordered
            .iter()
            .find(|b| b.stem == tok || b.stem.contains(tok))
    };
    let source = match hinted {
        Some(bw) => Some(bw),
        None if tok.is_empty() => ordered.first(),
        None => None,
    };

    if has_specific_dest {
        if let Some(bw) = source {
            if !bw.exe_path.is_empty() {
                win::focus_hwnd(bw.hwnd);
                let mut cmd = Command::new(&bw.exe_path);
                // Zen/Firefox remoting only reaches the RUNNING instance with
                // `-profile <path>`; without it a second instance opens in
                // another profile. Chromium ignores -profile (its profile
                // flag is --profile-directory), so pass the URL alone.
                if !profile.is_empty() && matches!(bw.stem.as_str(), "zen" | "zenbrowser" | "firefox") {
                    cmd.args(["-profile", profile]);
                }
                cmd.arg(full_url);
                if silent(&mut cmd).spawn().is_ok() {
                    let outcome = route_decision(false, true, is_wa, !sender.is_empty(), false, false, true, "spawned");
                    return json!({
                        "ok": true,
                        "outcome": outcome,
                        "reused": false,
                        "browser": bw.stem,
                        "reason": "tab-unverified-exe"
                    });
                }
            }
            // Exe missing or spawn failed: the window exists, focus it and
            // be honest that the destination did not open.
            let focused = win::focus_hwnd(bw.hwnd);
            let outcome = route_decision(false, true, is_wa, !sender.is_empty(), false, false, true, "focus");
            return json!({
                "ok": focused,
                "outcome": if focused { outcome } else { "unavailable" },
                "reason": "tab-unverified"
            });
        }
        // No hinted browser window running: open once via the shell (default
        // browser) — the destination still opens, just not in the source app.
        let ok = open_uri_via_shell(full_url);
        let outcome = route_decision(false, true, is_wa, !sender.is_empty(), false, false, false, "none");
        return json!({
            "ok": ok,
            "outcome": if ok { outcome } else { "unavailable" },
            "reused": false,
            "reason": "no-source-window"
        });
    }

    // Only a site root URL:
    // - ONLY spawn a new tab when verified absent (`tabs_seen > 0`, and for Gecko
    //   also `gecko_checked` after `% host` switch-to-tab found no open tab).
    // - Never run WhatsApp sidebar search unless the active window title confirms WhatsApp.
    if let Some(bw) = source {
        let focused = win::focus_hwnd(bw.hwnd);
        let is_gecko = matches!(bw.stem.as_str(), "zen" | "zenbrowser" | "firefox");
        let verified_absent = should_spawn_root_tab(is_gecko, tabs_seen, gecko_checked);
        if verified_absent && !bw.exe_path.is_empty() {
            let mut cmd = Command::new(&bw.exe_path);
            if !profile.is_empty() && is_gecko {
                cmd.args(["-profile", profile]);
            }
            cmd.arg(full_url);
            if silent(&mut cmd).spawn().is_ok() {
                return json!({
                    "ok": true,
                    "outcome": "opened",
                    "reused": false,
                    "chat_opened": false,
                    "browser": bw.stem,
                    "reason": "confirmed-no-existing-tab"
                });
            }
        }
        let active_is_wa = is_wa && window_title(bw.hwnd).contains("whatsapp");
        let chat_ok = if focused && active_is_wa && !sender.is_empty() {
            try_open_whatsapp_chat_blocking(bw.hwnd, sender, 260)
        } else {
            false
        };
        let outcome = if chat_ok {
            "navigated"
        } else {
            route_decision(false, false, is_wa, !sender.is_empty(), false, false, true, "none")
        };
        return json!({
            "ok": focused,
            "outcome": if focused { outcome } else { "unavailable" },
            "chat_opened": chat_ok,
            "reason": "tab-unverified-root"
        });
    }

    // No browser window at all: open the known destination once.
    let ok = open_uri_via_shell(full_url);
    let outcome = route_decision(false, false, is_wa, !sender.is_empty(), false, false, false, "none");
    json!({ "ok": ok, "outcome": if ok { outcome } else { "unavailable" } })
}
pub async fn notif_open(hint: &str, url: &str, profile: &str) -> Value {
    notif_open_chat(hint, url, profile, "").await
}

pub async fn toggle_theme(mode: &str) -> Value {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_DWORD,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let light: u32 = if mode.eq_ignore_ascii_case("light") {
        1
    } else {
        0
    };
    let ok = tokio::task::spawn_blocking(move || unsafe {
        let sub = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) != 0 {
            return false;
        }
        let v1 = to_wide("AppsUseLightTheme");
        let v2 = to_wide("SystemUsesLightTheme");
        let bytes = light.to_le_bytes();
        let s1 = RegSetValueExW(hkey, v1.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4);
        let s2 = RegSetValueExW(hkey, v2.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4);
        RegCloseKey(hkey);

        let param = to_wide("ImmersiveColorSet");
        let mut res = 0usize;
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            param.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            100,
            &mut res,
        );
        s1 == 0 && s2 == 0
    })
    .await
    .unwrap_or(false);
    json!({ "ok": ok })
}

pub async fn set_volume(level: Option<i64>, delta: Option<i64>) -> Value {
    tokio::task::spawn_blocking(move || {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(
                &MMDeviceEnumerator,
                None,
                CLSCTX_ALL,
            ) else {
                return json!({ "ok": false });
            };
            let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) else {
                return json!({ "ok": false });
            };
            let Ok(vol) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else {
                return json!({ "ok": false });
            };
            let cur = vol.GetMasterVolumeLevelScalar().unwrap_or(0.5);
            let target = if let Some(lvl) = level {
                (lvl as f32 / 100.0).clamp(0.0, 1.0)
            } else if let Some(d) = delta {
                (cur + d as f32 / 100.0).clamp(0.0, 1.0)
            } else {
                return json!({ "ok": true, "level": (cur * 100.0).round() as i64 });
            };

            let ok = vol
                .SetMasterVolumeLevelScalar(target, std::ptr::null())
                .is_ok();
            let actual = vol.GetMasterVolumeLevelScalar().unwrap_or(target);
            json!({ "ok": ok, "level": (actual * 100.0).round() as i64 })
        }
    })
    .await
    .unwrap_or_else(|_| json!({ "ok": false }))
}

static APPS_CACHE: OnceLock<Mutex<Vec<InstalledApp>>> = OnceLock::new();

pub fn list_installed_apps(force_refresh: bool) -> Vec<InstalledApp> {
    let cache = APPS_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    if !force_refresh {
        if let Ok(g) = cache.lock() {
            if !g.is_empty() {
                return g.clone();
            }
        }
    }

    let mut apps = enumerate_apps_folder();
    if apps.is_empty() {
        apps = scan_start_menu_shortcuts();
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.id.eq_ignore_ascii_case(&b.id));

    if let Ok(mut g) = cache.lock() {
        *g = apps.clone();
    }
    apps
}

fn enumerate_apps_folder() -> Vec<InstalledApp> {
    use windows::core::{GUID, Interface};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2,
        KNOWN_FOLDER_FLAG, SHGetKnownFolderItem, SIGDN_NORMALDISPLAY,
    };

    const PKEY_AUMID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };

    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(
            &FOLDERID_AppsFolder,
            KNOWN_FOLDER_FLAG(0),
            None,
        ) else {
            return out;
        };
        let Ok(enumerator) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems)
        else {
            return out;
        };

        let mut buf: [Option<IShellItem>; 32] = std::array::from_fn(|_| None);
        loop {
            let mut fetched = 0u32;
            let hr = enumerator.Next(&mut buf, Some(&mut fetched));
            if fetched == 0 {
                break;
            }
            for item_opt in buf.iter_mut().take(fetched as usize) {
                let Some(item) = item_opt.take() else { continue };
                let name = match item.GetDisplayName(SIGDN_NORMALDISPLAY) {
                    Ok(pw) => {
                        let s = pw.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(pw.0 as *const _));
                        s
                    }
                    Err(_) => continue,
                };
                if name.is_empty() {
                    continue;
                }
                let aumid = match item.cast::<IShellItem2>() {
                    Ok(item2) => match item2.GetString(&PKEY_AUMID) {
                        Ok(pw) => {
                            let s = pw.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(pw.0 as *const _));
                            s
                        }
                        Err(_) => String::new(),
                    },
                    Err(_) => String::new(),
                };
                if !aumid.is_empty() {
                    out.push(InstalledApp {
                        id: aumid.clone(),
                        name,
                        aumid,
                        path: String::new(),
                        use_count: 0,
                    });
                }
            }
            if hr.is_err() {
                break;
            }
        }
    }
    out
}

fn scan_start_menu_shortcuts() -> Vec<InstalledApp> {
    let mut out = Vec::new();
    let mut roots = Vec::new();
    if let Ok(pd) = std::env::var("ProgramData") {
        roots.push(
            Path::new(&pd)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        roots.push(
            Path::new(&ad)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    for root in roots {
        collect_lnk_recursive(&root, &mut out, 0);
    }
    out
}

fn collect_lnk_recursive(dir: &Path, out: &mut Vec<InstalledApp>, depth: usize) {
    if depth > 4 || !dir.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_lnk_recursive(&p, out, depth + 1);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false)
            {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let low = name.to_lowercase();
                if name.is_empty() || low.contains("uninstall") || low.contains("readme") {
                    continue;
                }
                let ps = p.to_string_lossy().to_string();
                out.push(InstalledApp {
                    id: ps.clone(),
                    name,
                    aumid: String::new(),
                    path: ps,
                    use_count: 0,
                });
            }
        }
    }
}

pub fn launch_app(target_id: &str, target_name: &str) -> bool {
    let tid = target_id.trim();
    if tid.is_empty() {
        return false;
    }

    // 1. Existing path (.lnk / .exe)
    if Path::new(tid).exists() {
        return open_uri_via_shell(tid);
    }

    // 2. Check if already running by name/stem
    let needle = target_name.trim().to_lowercase();
    let list = unsafe { enum_visible_windows() };
    if !needle.is_empty() {
        if let Some(w) = list
            .iter()
            .find(|w| w.stem == needle || w.title.to_lowercase().contains(&needle))
        {
            if win::focus_hwnd(w.hwnd) {
                return true;
            }
        }
    }

    // 3. Launch via shell:AppsFolder\<aumid>
    let uri = format!(r"shell:AppsFolder\{}", tid);
    if open_uri_via_shell(&uri) {
        return true;
    }

    // 4. Fallback: match in cached Start Menu / AppsFolder list
    let apps = list_installed_apps(false);
    if let Some(app) = apps.iter().find(|a| {
        a.id.eq_ignore_ascii_case(tid)
            || (!needle.is_empty() && a.name.to_lowercase().contains(&needle))
    }) {
        if !app.path.is_empty() {
            return open_uri_via_shell(&app.path);
        }
        if !app.aumid.is_empty() {
            return open_uri_via_shell(&format!(r"shell:AppsFolder\{}", app.aumid));
        }
    }
    false
}

pub fn delete_to_recycle_bin(path_str: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};

    let p = path_str.trim().replace('/', "\\");
    if p.is_empty() || !Path::new(&p).exists() {
        return Err("file tidak ditemukan".to_string());
    }
    // SHFileOperationW requires double-null-terminated string
    let mut wide: Vec<u16> = std::ffi::OsStr::new(&p).encode_wide().collect();
    wide.push(0);
    wide.push(0);

    let mut op = SHFILEOPSTRUCTW {
        hwnd: windows::Win32::Foundation::HWND(std::ptr::null_mut()),
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(wide.as_ptr()),
        pTo: windows::core::PCWSTR::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT).0 as u16,
        fAnyOperationsAborted: false.into(),
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: windows::core::PCWSTR::null(),
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc == 0 && !op.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(format!("gagal menghapus ke Recycle Bin (kode {})", rc))
    }
}

pub fn photo_pick(owner: HwndHandle) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut file_buf = [0u16; 1024];
    let filter: Vec<u16> = "Images (*.png;*.jpg;*.jpeg;*.webp;*.bmp)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp\0All Files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = to_wide("Pilih Foto Profil");

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner.0 as *mut _),
        lpstrFilter: windows::core::PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: windows::core::PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    if unsafe { GetOpenFileNameW(&mut ofn).as_bool() } {
        let len = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        let path = String::from_utf16_lossy(&file_buf[..len]);
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

struct KillChildOnDrop(Option<std::process::Child>);

impl Drop for KillChildOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub async fn run_sandboxed_shell(cmd: &str) -> Value {
    run_sandboxed_shell_cancellable(cmd, None).await
}

pub async fn run_sandboxed_shell_cancellable(
    cmd: &str,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Value {
    use std::sync::atomic::Ordering;

    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return json!({ "ok": false, "code": -1, "out": "perintah kosong" });
    }
    if trimmed.len() > 2000 {
        return json!({ "ok": false, "code": -1, "out": "perintah terlalu panjang" });
    }

    let lower = trimmed.to_lowercase();
    let dangerous = [
        "format ",
        "mkfs",
        "diskpart",
        "rm -rf /",
        "bcdedit",
        "vssadmin delete",
        "cipher /w",
        "net user ",
        "reg delete hklm",
        "shutdown /s",
        "set-executionpolicy",
    ];
    if dangerous.iter().any(|&d| lower.contains(d)) {
        return json!({ "ok": false, "code": -1, "out": "perintah ditolak oleh pengaman" });
    }

    if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
        return json!({ "ok": false, "code": -3, "out": "dibatalkan oleh pengguna" });
    }

    let cmd_str = trimmed.to_string();
    let res = tokio::task::spawn_blocking(move || {
        let child = match silent(&mut Command::new("powershell.exe"))
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd_str])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return (false, -1, format!("gagal menjalankan powershell: {}", e)),
        };

        let mut guard = KillChildOnDrop(Some(child));
        let started = Instant::now();
        loop {
            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                drop(guard);
                return (false, -3, "dibatalkan oleh pengguna".to_string());
            }
            let status_res = guard.0.as_mut().unwrap().try_wait();
            match status_res {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if started.elapsed() >= Duration::from_secs(30) {
                        drop(guard);
                        return (false, -2, "timeout (30s)".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(e) => {
                    drop(guard);
                    return (false, -1, format!("gagal menunggu powershell: {}", e));
                }
            }
        }

        let Some(finished_child) = guard.0.take() else {
            return (false, -3, "dibatalkan oleh pengguna".to_string());
        };
        match finished_child.wait_with_output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = if stderr.is_empty() {
                    stdout
                } else {
                    format!("{}\n{}", stdout, stderr)
                };
                let capped = if combined.len() > 60_000 {
                    let mut end = 60_000;
                    while end > 0 && !combined.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}\n…(dipotong)", &combined[..end])
                } else {
                    combined
                };
                let code = out.status.code().unwrap_or(-1);
                (code == 0, code, capped.trim().to_string())
            }
            Err(e) => (false, -1, format!("gagal membaca output powershell: {}", e)),
        }
    })
    .await;

    match res {
        Ok((ok, code, out)) => json!({ "ok": ok, "code": code, "out": out }),
        Err(e) => json!({ "ok": false, "code": -1, "out": format!("task error: {}", e) }),
    }
}

const IGNORED_SEARCH_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "appdata",
    "$recycle.bin",
    "system volume information",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    ".cache",
    ".cargo",
    ".rustup",
    "venv",
    ".venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
];

pub fn search_files_in_roots(
    roots: &[std::path::PathBuf],
    query: &str,
    max_depth: usize,
    max_dirs: usize,
) -> Vec<Value> {
    use std::collections::{HashSet, VecDeque};
    use std::time::UNIX_EPOCH;

    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<String> = q
        .split_whitespace()
        .map(|t| t.trim_matches('.').to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let mut queue: VecDeque<(std::path::PathBuf, usize)> = VecDeque::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    for r in roots {
        if r.exists() && r.is_dir() {
            let key = r.to_string_lossy().to_lowercase();
            if seen_dirs.insert(key) {
                queue.push_back((r.clone(), 0));
            }
        }
    }

    struct ScoredHit {
        tier: u8,
        mtime: u64,
        name: String,
        path: String,
        dir: String,
    }

    let mut visited_dirs = 0usize;
    let mut hits: Vec<ScoredHit> = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if visited_dirs >= max_dirs || depth > max_depth {
            continue;
        }
        visited_dirs += 1;

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let dir_str = dir.to_string_lossy().to_string();

        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let name_low = file_name.to_lowercase();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let is_dir = ft.is_dir();
            if is_dir && IGNORED_SEARCH_DIRS.contains(&name_low.as_str()) {
                continue;
            }

            let entry_path = entry.path();
            if is_dir && depth < max_depth {
                let dkey = entry_path.to_string_lossy().to_lowercase();
                if seen_dirs.insert(dkey) {
                    queue.push_back((entry_path.clone(), depth + 1));
                }
            }

            let path_str = entry_path.to_string_lossy().to_string();
            let full_low = path_str.to_lowercase();
            let tier = if name_low.contains(&q) {
                3u8
            } else if !tokens.is_empty() && tokens.iter().all(|t| name_low.contains(t.as_str())) {
                2u8
            } else if !tokens.is_empty() && tokens.iter().all(|t| full_low.contains(t.as_str())) {
                1u8
            } else {
                0u8
            };

            if tier > 0 {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                hits.push(ScoredHit {
                    tier,
                    mtime,
                    name: file_name,
                    path: path_str,
                    dir: dir_str.clone(),
                });
            }
        }
    }

    hits.sort_by(|a, b| b.tier.cmp(&a.tier).then_with(|| b.mtime.cmp(&a.mtime)));
    hits.truncate(30);
    hits.into_iter()
        .map(|h| {
            json!({
                "name": h.name,
                "path": h.path,
                "dir": h.dir,
            })
        })
        .collect()
}

pub async fn search_files(query: &str, hint: Option<&str>) -> Value {
    let q = query.trim().to_string();
    if q.is_empty() {
        return json!([]);
    }

    let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\Default".to_string());
    let onedrive = std::env::var("OneDrive").unwrap_or_else(|_| format!(r"{}\OneDrive", user_profile));
    let hint_low = hint.unwrap_or("").trim().to_lowercase();
    let has_hint = !hint_low.is_empty();

    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    match hint_low.as_str() {
        "download" | "downloads" => {
            roots.push(std::path::PathBuf::from(format!(r"{}\Downloads", user_profile)));
            roots.push(std::path::PathBuf::from(format!(r"{}\Downloads", onedrive)));
        }
        "documents" | "dokumen" => {
            roots.push(std::path::PathBuf::from(format!(r"{}\Documents", user_profile)));
            roots.push(std::path::PathBuf::from(format!(r"{}\Documents", onedrive)));
        }
        "desktop" => {
            roots.push(std::path::PathBuf::from(format!(r"{}\Desktop", user_profile)));
            roots.push(std::path::PathBuf::from(format!(r"{}\Desktop", onedrive)));
        }
        _ => {
            for sub in ["Desktop", "Documents", "Downloads"] {
                roots.push(std::path::PathBuf::from(format!(r"{}\{}", user_profile, sub)));
                roots.push(std::path::PathBuf::from(format!(r"{}\{}", onedrive, sub)));
            }
        }
    }

    let hits = tokio::task::spawn_blocking(move || {
        let mut found = search_files_in_roots(&roots, &q, 4, 2500);
        if found.is_empty() && !has_hint {
            let d_drive = std::path::PathBuf::from(r"D:\");
            if d_drive.exists() {
                found = search_files_in_roots(&[d_drive], &q, 3, 1500);
            }
        }
        found
    })
    .await
    .unwrap_or_default();

    json!(hits)
}

pub fn describe_file(path_str: &str) -> Option<crate::state::FileCardInfo> {
    let path = Path::new(path_str);
    if !path.exists() {
        return None;
    }
    let is_dir = path.is_dir();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path_str)
        .to_string();

    let mut thumb: Option<String> = None;
    if !is_dir {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif") {
            if let Ok(metadata) = path.metadata() {
                if metadata.len() < 8 * 1024 * 1024 {
                    if let Ok(bytes) = std::fs::read(path) {
                        let mime = match ext.as_str() {
                            "jpg" | "jpeg" => "image/jpeg",
                            "webp" => "image/webp",
                            "bmp" => "image/bmp",
                            "gif" => "image/gif",
                            _ => "image/png",
                        };
                        thumb = Some(format!("data:{};base64,{}", mime, BASE64_STANDARD.encode(&bytes)));
                    }
                }
            }
        }
    }

    Some(crate::state::FileCardInfo {
        path: path_str.to_string(),
        name,
        is_dir,
        thumb,
    })
}

fn filetime_to_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn read_cpu_times() -> Option<(u64, u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
    }
    Some((
        filetime_to_u64(idle),
        filetime_to_u64(kernel),
        filetime_to_u64(user),
    ))
}

static PREV_CPU_SAMPLE: OnceLock<Mutex<Option<(u64, u64, u64)>>> = OnceLock::new();

/// True when Windows reports an internet-connected network (NLM COM).
/// Runs on the polling worker thread; conservative: a COM failure keeps
/// the previous state instead of flapping the offline popup.
pub fn internet_connected() -> Option<bool> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    unsafe {
        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| {
            let mgr: windows::Win32::Networking::NetworkListManager::INetworkListManager =
                CoCreateInstance(
                    &windows::Win32::Networking::NetworkListManager::NetworkListManager,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .ok()?;
            let connected = mgr.IsConnectedToInternet().ok()?;
            Some(connected.as_bool())
        })();
        if init_ok {
            CoUninitialize();
        }
        result
    }
}

pub fn get_live_usage() -> LiveUsage {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut mem_pct = 0.0f32;
    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut ms).is_ok() && ms.ullTotalPhys > 0 {
            let used = ms.ullTotalPhys.saturating_sub(ms.ullAvailPhys);
            mem_pct = ((used as f64 / ms.ullTotalPhys as f64) * 100.0).clamp(0.0, 100.0) as f32;
        }
    }

    let mut cpu_pct = 0.0f32;
    if let Some(cur) = read_cpu_times() {
        let slot = PREV_CPU_SAMPLE.get_or_init(|| Mutex::new(None));
        if let Ok(mut prev) = slot.lock() {
            if let Some(p) = *prev {
                let d_idle = cur.0.saturating_sub(p.0);
                let d_total = cur.1.saturating_sub(p.1) + cur.2.saturating_sub(p.2);
                if d_total > 0 {
                    let busy = d_total.saturating_sub(d_idle);
                    cpu_pct = ((busy as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0) as f32;
                }
            }
            *prev = Some(cur);
        }
    }

    LiveUsage {
        cpu: cpu_pct,
        gpu: 0.0,
        mem: mem_pct,
    }
}

fn round1(n: f64) -> f64 {
    (n * 10.0).round() / 10.0
}

pub async fn diagnose_system() -> Value {
    tokio::task::spawn_blocking(|| {
        use windows::Win32::Foundation::{CloseHandle, FILETIME};
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::SystemInformation::{
            GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
        };
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let n_cpu = std::thread::available_parallelism()
            .map(|p| p.get() as f64)
            .unwrap_or(1.0)
            .max(1.0);

        let sys_t0 = read_cpu_times();

        let snapshot_proc_cpu = || -> HashMap<u32, (String, u64, u64)> {
            let mut map = HashMap::new();
            unsafe {
                let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                    return map;
                };
                let mut pe = PROCESSENTRY32W {
                    dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                    ..Default::default()
                };
                if Process32FirstW(snap, &mut pe).is_ok() {
                    loop {
                        let pid = pe.th32ProcessID;
                        if pid != 0 {
                            let len = pe
                                .szExeFile
                                .iter()
                                .position(|&c| c == 0)
                                .unwrap_or(pe.szExeFile.len());
                            let raw_name = String::from_utf16_lossy(&pe.szExeFile[..len]);
                            let name = raw_name
                                .strip_suffix(".exe")
                                .or_else(|| raw_name.strip_suffix(".EXE"))
                                .unwrap_or(&raw_name)
                                .to_string();

                            let mut cpu_100ns = 0u64;
                            let mut mem_bytes = 0u64;
                            if let Ok(hp) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                                let (mut ct, mut et, mut kt, mut ut) = (
                                    FILETIME::default(),
                                    FILETIME::default(),
                                    FILETIME::default(),
                                    FILETIME::default(),
                                );
                                if GetProcessTimes(hp, &mut ct, &mut et, &mut kt, &mut ut).is_ok() {
                                    cpu_100ns = filetime_to_u64(kt) + filetime_to_u64(ut);
                                }
                                let mut pmc = PROCESS_MEMORY_COUNTERS {
                                    cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                                    ..Default::default()
                                };
                                if K32GetProcessMemoryInfo(
                                    hp,
                                    &mut pmc,
                                    std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                                )
                                .as_bool()
                                {
                                    mem_bytes = pmc.WorkingSetSize as u64;
                                }
                                let _ = CloseHandle(hp);
                            }
                            map.insert(pid, (name, cpu_100ns, mem_bytes));
                        }
                        if Process32NextW(snap, &mut pe).is_err() {
                            break;
                        }
                    }
                }
                let _ = CloseHandle(snap);
            }
            map
        };

        let p0 = snapshot_proc_cpu();
        std::thread::sleep(Duration::from_millis(500));
        let p1 = snapshot_proc_cpu();
        let sys_t1 = read_cpu_times();

        let cpu_pct = match (sys_t0, sys_t1) {
            (Some(a), Some(b)) => {
                let d_idle = b.0.saturating_sub(a.0);
                let d_total = b.1.saturating_sub(a.1) + b.2.saturating_sub(a.2);
                if d_total > 0 {
                    Some(round1(
                        (d_total.saturating_sub(d_idle) as f64 / d_total as f64) * 100.0,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        };

        let (ram_used_gb, ram_total_gb) = unsafe {
            let mut ms = MEMORYSTATUSEX {
                dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if GlobalMemoryStatusEx(&mut ms).is_ok() && ms.ullTotalPhys > 0 {
                let total = round1(ms.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0));
                let used = round1(
                    ms.ullTotalPhys.saturating_sub(ms.ullAvailPhys) as f64
                        / (1024.0 * 1024.0 * 1024.0),
                );
                (Some(used), Some(total))
            } else {
                (None, None)
            }
        };

        let (disk_c_free_gb, disk_c_total_gb) = unsafe {
            let drive = to_wide("C:\\");
            let mut free_avail = 0u64;
            let mut total_bytes = 0u64;
            let mut total_free = 0u64;
            if GetDiskFreeSpaceExW(
                windows::core::PCWSTR(drive.as_ptr()),
                Some(&mut free_avail),
                Some(&mut total_bytes),
                Some(&mut total_free),
            )
            .is_ok()
            {
                (
                    Some(round1(free_avail as f64 / (1024.0 * 1024.0 * 1024.0))),
                    Some(round1(total_bytes as f64 / (1024.0 * 1024.0 * 1024.0))),
                )
            } else {
                (None, None)
            }
        };

        let uptime_h = unsafe { round1(GetTickCount64() as f64 / 3_600_000.0) };

        let mut procs: Vec<(String, f64, f64)> = p1
            .into_iter()
            .filter_map(|(pid, (name, cpu1, mem1))| {
                if name.is_empty() || mem1 == 0 {
                    return None;
                }
                let cpu0 = p0.get(&pid).map(|x| x.1).unwrap_or(cpu1);
                let delta_sec = cpu1.saturating_sub(cpu0) as f64 / 10_000_000.0;
                let cpu_proc = round1(((delta_sec / 0.5) * 100.0 / n_cpu).clamp(0.0, 100.0));
                let mem_mb = round1(mem1 as f64 / (1024.0 * 1024.0));
                Some((name, mem_mb, cpu_proc))
            })
            .collect();
        procs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        procs.truncate(5);

        let top_processes: Vec<Value> = procs
            .into_iter()
            .map(|(name, mem_mb, cpu)| json!({ "name": name, "mem_mb": mem_mb, "cpu": cpu }))
            .collect();

        let mut notes = Vec::new();
        if let (Some(u), Some(t)) = (ram_used_gb, ram_total_gb) {
            if t > 0.0 && (u / t) * 100.0 > 85.0 {
                notes.push("RAM hampir penuh".to_string());
            }
        }
        if let Some(c) = cpu_pct {
            if c > 80.0 {
                notes.push("CPU tinggi".to_string());
            }
        }
        if let Some(free) = disk_c_free_gb {
            if free < 15.0 {
                notes.push("disk C: hampir penuh".to_string());
            }
        }

        json!({
            "ok": true,
            "cpu_pct": cpu_pct,
            "ram_used_gb": ram_used_gb,
            "ram_total_gb": ram_total_gb,
            "disk_c_free_gb": disk_c_free_gb,
            "disk_c_total_gb": disk_c_total_gb,
            "uptime_h": uptime_h,
            "top_processes": top_processes,
            "notes": notes,
        })
    })
    .await
    .unwrap_or_else(|e| json!({ "ok": false, "reason": e.to_string() }))
}

fn parse_published_age_mins(txt: &str) -> u64 {
    let low = txt.to_lowercase();
    let num: u64 = low
        .split_whitespace()
        .find_map(|w| w.parse::<u64>().ok())
        .unwrap_or(999_999);
    if low.contains("second") || low.contains("detik") {
        0
    } else if low.contains("minute") || low.contains("menit") {
        num
    } else if low.contains("hour") || low.contains("jam") {
        num * 60
    } else if low.contains("day") || low.contains("hari") {
        num * 1440
    } else if low.contains("week") || low.contains("minggu") {
        num * 10_080
    } else if low.contains("month") || low.contains("bulan") {
        num * 43_200
    } else if low.contains("year") || low.contains("tahun") {
        num * 525_600
    } else {
        999_999_999
    }
}

pub async fn youtube_search(query: &str, newest: bool) -> Value {
    let q = query.trim();
    if q.is_empty() {
        return json!({ "ok": false, "reason": "query kosong" });
    }
    let sp = if newest { "&sp=CAI%253D" } else { "" };
    let url = format!(
        "https://www.youtube.com/results?search_query={}{}",
        crate::backend::lrc::urlencode(q),
        sp
    );
    let cl = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/125.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "reason": e.to_string() }),
    };

    let html = match cl.get(&url).send().await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return json!({ "ok": false, "reason": e.to_string() }),
    };

    // Extract ytInitialData JSON blob
    let mut videos: Vec<(u64, String, String, String)> = Vec::new();
    let marker = "var ytInitialData = ";
    if let Some(pos) = html.find(marker) {
        let rest = &html[pos + marker.len()..];
        if let Some(end) = rest.find(";</script>") {
            if let Ok(v) = serde_json::from_str::<Value>(&rest[..end]) {
                collect_yt_videos(&v, &mut videos);
            }
        }
    }

    if videos.is_empty() {
        // Regex fallback for videoId
        let re = regex::Regex::new(r#""videoRenderer":\{"videoId":"([a-zA-Z0-9_-]{11})""#).ok();
        if let Some(re) = re {
            if let Some(cap) = re.captures(&html) {
                let vid = cap[1].to_string();
                let vurl = format!("https://www.youtube.com/watch?v={}", vid);
                return json!({ "ok": true, "url": vurl, "videoId": vid, "title": q });
            }
        }
        return json!({ "ok": false, "reason": "tidak ditemukan hasil video" });
    }

    if newest {
        videos.sort_by_key(|x| x.0);
    }
    let (_, vid, title, pub_txt) = &videos[0];
    json!({
        "ok": true,
        "videoId": vid,
        "title": title,
        "published": pub_txt,
        "url": format!("https://www.youtube.com/watch?v={}", vid),
    })
}

fn collect_yt_videos(v: &Value, out: &mut Vec<(u64, String, String, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(vr) = map.get("videoRenderer").and_then(|x| x.as_object()) {
                let vid = vr.get("videoId").and_then(|x| x.as_str()).unwrap_or("");
                if vid.len() == 11 {
                    let title = vr
                        .get("title")
                        .and_then(|t| t.get("runs"))
                        .and_then(|r| r.get(0))
                        .and_then(|r| r.get("text"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let pub_txt = vr
                        .get("publishedTimeText")
                        .and_then(|t| t.get("simpleText"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let age = if pub_txt.is_empty() {
                        u64::MAX
                    } else {
                        parse_published_age_mins(&pub_txt)
                    };
                    out.push((age, vid.to_string(), title, pub_txt));
                }
            }
            for val in map.values() {
                if out.len() >= 15 {
                    return;
                }
                collect_yt_videos(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                if out.len() >= 15 {
                    return;
                }
                collect_yt_videos(val, out);
            }
        }
        _ => {}
    }
}

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE_NAME: &str = "SelaBar";
const LEGACY_RUN_VALUE_NAME: &str = "DeskNotch";

fn query_run_value(
    hkey: windows_sys::Win32::System::Registry::HKEY,
    value_name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    unsafe {
        let name = to_wide(value_name);
        let mut buf: Vec<u16> = vec![0; 1024];
        let mut len: u32 = (buf.len() * 2) as u32;
        let status = RegGetValueW(
            hkey,
            std::ptr::null(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        );
        if status != 0 {
            return None;
        }
        let chars = (len as usize / 2).saturating_sub(1);
        let text = String::from_utf16_lossy(&buf[..chars]);
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

pub fn start_on_boot_entry() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    };
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let subkey = to_wide(RUN_KEY_PATH);
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) != 0
        {
            return None;
        }
        let res = query_run_value(hkey, RUN_VALUE_NAME)
            .or_else(|| query_run_value(hkey, LEGACY_RUN_VALUE_NAME));
        RegCloseKey(hkey);
        res
    }
}

fn is_dev_build_exe(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

fn strip_verbatim_prefix_path(p: &Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return std::path::PathBuf::from(format!(r"\\{}", rest));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return std::path::PathBuf::from(rest);
    }
    p.to_path_buf()
}

fn start_menu_shortcut_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    if appdata.is_empty() {
        return None;
    }
    Some(
        std::path::PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("SelaBar.lnk"),
    )
}

/// Ensures portable `SelaBar.exe` is discoverable from Windows Search, Start Menu,
/// and `Win+R` (`App Paths`) automatically on launch, without touching dev `target/` builds.
pub fn ensure_app_registration() {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, REG_SZ,
    };

    let Ok(raw_exe) = std::env::current_exe() else {
        return;
    };
    if is_dev_build_exe(&raw_exe) {
        return;
    }
    let exe = strip_verbatim_prefix_path(&raw_exe);
    let _ = crate::backend::installer::register_windows_uninstall(&exe);
    let exe_str = exe.to_string_lossy().into_owned();
    let dir_str = exe
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();

    // 1. Ensure Start Menu shortcut (%APPDATA%\Microsoft\Windows\Start Menu\Programs\SelaBar.lnk)
    if let Some(lnk_path) = start_menu_shortcut_path() {
        let mut needs_write = true;
        unsafe {
            let init_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            if let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER)
            {
                if let Ok(persist) = link.cast::<IPersistFile>() {
                    let lnk_w = to_wide(&lnk_path.to_string_lossy());
                    if lnk_path.exists()
                        && persist.Load(PCWSTR(lnk_w.as_ptr()), STGM_READ).is_ok()
                    {
                        let mut cur_buf = [0u16; 1024];
                        if link
                            .GetPath(&mut cur_buf, std::ptr::null_mut(), 0)
                            .is_ok()
                        {
                            let end = cur_buf
                                .iter()
                                .position(|&c| c == 0)
                                .unwrap_or(cur_buf.len());
                            let cur_path = String::from_utf16_lossy(&cur_buf[..end]);
                            if cur_path.eq_ignore_ascii_case(&exe_str) {
                                needs_write = false;
                            }
                        }
                    }

                    if needs_write {
                        if let Some(parent) = lnk_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let exe_w = to_wide(&exe_str);
                        let desc_w = to_wide("SelaBar — Dynamic Notch & Desktop AI Companion");
                        let dir_w = to_wide(&dir_str);
                        let _ = link.SetPath(PCWSTR(exe_w.as_ptr()));
                        if !dir_str.is_empty() {
                            let _ = link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()));
                        }
                        let _ = link.SetDescription(PCWSTR(desc_w.as_ptr()));
                        let _ = link.SetIconLocation(PCWSTR(exe_w.as_ptr()), 0);
                        if persist.Save(PCWSTR(lnk_w.as_ptr()), true).is_ok() {
                            crate::backend::logfile::append(&format!(
                                "[startup] registered Start Menu shortcut '{}' -> '{}'",
                                lnk_path.display(),
                                exe_str
                            ));
                        }
                    }
                }
            }
            if init_ok {
                CoUninitialize();
            }
        }
    }

    // 2. Register HKCU App Paths so `SelaBar` / `Win+R` resolves from anywhere
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\App Paths\SelaBar.exe");
        let status = RegCreateKeyW(HKEY_CURRENT_USER, subkey.as_ptr(), &mut hkey);
        if status == 0 {
            let existing = query_run_value(hkey, "");
            if existing
                .as_deref()
                .map(|s| !s.eq_ignore_ascii_case(&exe_str))
                .unwrap_or(true)
            {
                let val_w = to_wide(&exe_str);
                let _ = RegSetValueExW(
                    hkey,
                    std::ptr::null(),
                    0,
                    REG_SZ,
                    val_w.as_ptr().cast(),
                    (val_w.len() as u32) * 2,
                );
                if !dir_str.is_empty() {
                    let path_name_w = to_wide("Path");
                    let dir_w = to_wide(&dir_str);
                    let _ = RegSetValueExW(
                        hkey,
                        path_name_w.as_ptr(),
                        0,
                        REG_SZ,
                        dir_w.as_ptr().cast(),
                        (dir_w.len() as u32) * 2,
                    );
                }
                crate::backend::logfile::append(&format!(
                    "[startup] registered App Paths for '{}'",
                    exe_str
                ));
            }
            RegCloseKey(hkey);
        }
    }
}

pub fn set_start_on_boot(enable: bool) -> Result<bool, String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let subkey = to_wide(RUN_KEY_PATH);
        let name = to_wide(RUN_VALUE_NAME);
        let legacy_name = to_wide(LEGACY_RUN_VALUE_NAME);
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            &mut hkey,
        );
        if status != 0 {
            return Err(format!("buka registry Run gagal (kode {})", status));
        }

        // Remove stale legacy DeskNotch autostart value so Windows never launches duplicates.
        let _ = RegDeleteValueW(hkey, legacy_name.as_ptr());

        let result = if enable {
            let exe = std::env::current_exe()
                .map_err(|e| format!("path exe tidak tersedia: {}", e))?;
            if is_dev_build_exe(&exe) {
                Ok(false)
            } else {
                let cmdline = format!("\"{}\"", exe.display());
                let val = to_wide(&cmdline);
                let st = RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    val.as_ptr().cast(),
                    (val.len() as u32) * 2,
                );
                if st == 0 {
                    Ok(true)
                } else {
                    Err(format!("tulis nilai Run gagal (kode {})", st))
                }
            }
        } else {
            let st = RegDeleteValueW(hkey, name.as_ptr());
            if st == 0 || st == ERROR_FILE_NOT_FOUND as u32 {
                Ok(true)
            } else {
                Err(format!("hapus nilai Run gagal (kode {})", st))
            }
        };
        RegCloseKey(hkey);
        result
    }
}

pub fn detect_meeting_link(text: &str) -> Option<MeetingLink> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    static MEET_RE: OnceLock<regex::Regex> = OnceLock::new();
    static ZOOM_RE: OnceLock<regex::Regex> = OnceLock::new();
    static TEAMS_RE: OnceLock<regex::Regex> = OnceLock::new();
    static PWD_RE: OnceLock<regex::Regex> = OnceLock::new();

    let meet_re = MEET_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)https?://meet\.google\.com/([a-z]{3}-[a-z]{4}-[a-z]{3})(?:[^\s\x22<>]*)?").unwrap()
    });
    if let Some(cap) = meet_re.captures(trimmed) {
        let raw_url = cap[0].trim_end_matches(&['.', ',', ';', ')', ']', '"', '\''][..]);
        return Some(MeetingLink {
            provider: MeetingProvider::GoogleMeet,
            url: raw_url.to_string(),
            code: cap[1].to_lowercase(),
            pwd: String::new(),
        });
    }

    let zoom_re = ZOOM_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)https?://(?:[a-z0-9-]+\.)?zoom\.us/(?:j|my|wc/join)/([^\s\x22<>?]+)(?:\?([^\s\x22<>]*))?").unwrap()
    });
    if let Some(cap) = zoom_re.captures(trimmed) {
        let raw_url = cap[0].trim_end_matches(&['.', ',', ';', ')', ']', '"', '\''][..]);
        let code = cap[1]
            .trim_end_matches(&['/', '.', ',', ')'][..])
            .to_string();
        let pwd = cap
            .get(2)
            .and_then(|q| {
                let re = PWD_RE
                    .get_or_init(|| regex::Regex::new(r"(?i)(?:^|&)pwd=([^&\s]+)").unwrap());
                re.captures(q.as_str()).map(|c| c[1].to_string())
            })
            .unwrap_or_default();
        return Some(MeetingLink {
            provider: MeetingProvider::Zoom,
            url: raw_url.to_string(),
            code,
            pwd,
        });
    }

    let teams_re = TEAMS_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)https?://teams\.(?:microsoft|live)\.com/l/meetup-join/[^\s\x22<>]+").unwrap()
    });
    if let Some(m) = teams_re.find(trimmed) {
        let raw_url = m
            .as_str()
            .trim_end_matches(&['.', ',', ';', ')', ']', '"', '\''][..]);
        return Some(MeetingLink {
            provider: MeetingProvider::Teams,
            url: raw_url.to_string(),
            code: String::new(),
            pwd: String::new(),
        });
    }

    None
}

fn has_protocol_handler(scheme: &str) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    let sub = to_wide(&format!(r"Software\Classes\{}", scheme));
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut hk: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ, &mut hk) } == 0 && !hk.is_null()
        {
            unsafe { RegCloseKey(hk) };
            return true;
        }
    }
    false
}

pub fn is_zoom_installed() -> bool {
    if has_protocol_handler("zoommtg") {
        return true;
    }
    for (var, rel) in [
        ("APPDATA", r"Zoom\bin\Zoom.exe"),
        ("LOCALAPPDATA", r"Zoom\bin\Zoom.exe"),
        ("ProgramFiles", r"Zoom\bin\Zoom.exe"),
    ] {
        if let Ok(base) = std::env::var(var) {
            if Path::new(&base).join(rel).exists() {
                return true;
            }
        }
    }
    false
}

pub fn is_teams_installed() -> bool {
    if has_protocol_handler("msteams") {
        return true;
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        if Path::new(&la).join(r"Microsoft\WindowsApps\ms-teams.exe").exists()
            || Path::new(&la).join(r"Microsoft\Teams\current\Teams.exe").exists()
        {
            return true;
        }
    }
    false
}

pub async fn join_meeting_smart(link: MeetingLink) -> Value {
    match link.provider {
        MeetingProvider::GoogleMeet => open_url_in_active_browser(&link.url, false).await,
        MeetingProvider::Zoom => {
            if is_zoom_installed()
                && !link.code.is_empty()
                && link.code.chars().all(|c| c.is_ascii_digit())
            {
                let mut uri = format!("zoommtg://zoom.us/join?action=join&confno={}", link.code);
                if !link.pwd.is_empty() {
                    uri.push_str(&format!("&pwd={}", link.pwd));
                }
                if open_uri_via_shell(&uri) {
                    return json!({ "ok": true, "app": "zoom" });
                }
            }
            open_url_in_active_browser(&link.url, false).await
        }
        MeetingProvider::Teams => {
            if is_teams_installed() {
                let uri = if link.url.starts_with("https://") {
                    link.url.replacen("https://", "msteams://", 1)
                } else {
                    link.url.replacen("http://", "msteams://", 1)
                };
                if open_uri_via_shell(&uri) {
                    return json!({ "ok": true, "app": "teams" });
                }
            }
            open_url_in_active_browser(&link.url, false).await
        }
    }
}

pub fn poll_voip_call_window() -> Option<NotifPayload> {
    let windows = unsafe { enum_visible_windows() };
    for w in windows {
        let stem_low = w.stem.to_lowercase();
        let is_voip = [
            "whatsapp",
            "zapfast",
            "discord",
            "telegram",
            "zoom",
            "teams",
            "ms-teams",
            "zen",
            "chrome",
            "msedge",
            "firefox",
            "brave",
        ]
        .iter()
        .any(|s| stem_low.contains(s));
        if !is_voip {
            continue;
        }
        let tl = w.title.to_lowercase();
        let is_call_win = [
            "incoming call",
            "panggilan masuk",
            "is calling",
            "menelepon anda",
            "panggilan suara masuk",
            "panggilan video masuk",
        ]
        .iter()
        .any(|k| tl.contains(k));
        if is_call_win {
            let app_lbl = if stem_low.contains("zapfast") {
                "ZapFast"
            } else if stem_low.contains("whatsapp") || tl.contains("whatsapp") {
                "WhatsApp"
            } else if stem_low.contains("discord") || tl.contains("discord") {
                "Discord"
            } else if stem_low.contains("telegram") || tl.contains("telegram") {
                "Telegram"
            } else if stem_low.contains("zoom") {
                "Zoom"
            } else if stem_low.contains("teams") || tl.contains("teams") {
                "Teams"
            } else {
                "Browser"
            };
            return Some(NotifPayload {
                id: format!("win:{}", w.hwnd),
                launch: w.hwnd.to_string(),
                app: app_lbl.to_string(),
                raw_app: w.stem,
                title: w.title.clone(),
                body: format!("Panggilan masuk di {}", app_lbl),
                scenario: "incomingCall".to_string(),
                is_call: true,
                ..Default::default()
            });
        }
    }
    None
}

/// Consecutive call-window polls that must come back empty before we declare
/// the call ended. At the 800ms poll cadence that is ~2.4s — long enough to
/// ride out title flicker while a call is still up, short enough that the
/// green strip does not linger after hangup.
pub const CALL_END_MISS_POLLS: u32 = 3;

/// Tracks the ringing-call window across polls and decides when the call has
/// ended. The incoming-call title ("Pam Pam is calling…") disappears the
/// moment the call is answered, declined or hung up, so a run of empty polls
/// after a detection is the reliable end signal — mic state alone is not
/// (browsers can leave a stale ConsentStore entry that reads as "open").
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VoipCallTracker {
    key: String,
    misses: u32,
    ended_sent: bool,
}

impl VoipCallTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one poll result (`Some(app|title key)` while the ringing window
    /// is visible, `None` otherwise). Returns `true` exactly once per
    /// detected call, when the window has been absent for
    /// [`CALL_END_MISS_POLLS`] consecutive polls.
    pub fn observe(&mut self, poll_key: Option<&str>) -> bool {
        match poll_key {
            Some(k) => {
                self.key.clear();
                self.key.push_str(k);
                self.misses = 0;
                self.ended_sent = false;
                false
            }
            None => {
                if self.key.is_empty() || self.ended_sent {
                    return false;
                }
                self.misses = self.misses.saturating_add(1);
                if self.misses >= CALL_END_MISS_POLLS {
                    self.key.clear();
                    self.misses = 0;
                    self.ended_sent = true;
                    return true;
                }
                false
            }
        }
    }

    /// Whether a call window was detected and its end has not been declared.
    pub fn is_tracking(&self) -> bool {
        !self.key.is_empty()
    }
}

/// Whether a window handle still refers to an existing window. Used as a
/// hangup backstop: WhatsApp Web destroys its call popup on hangup, but the
/// window title alone may not change for browsers that keep the tab.
pub fn call_window_alive(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { IsWindow(Some(HWND(hwnd as *mut _))).as_bool() }
}

pub fn is_browser_raw_app(raw_app: &str) -> bool {
    let tok = normalize_browser_token(raw_app);
    !tok.is_empty() && BROWSER_STEMS.contains(&tok.as_str())
}

pub fn is_call_process_running(raw_app: &str, app_name: &str) -> bool {
    let raw_low = raw_app.trim().to_lowercase();
    let app_low = app_name.trim().to_lowercase();
    if raw_low.is_empty() && app_low.is_empty() {
        return true;
    }
    let raw_tok = normalize_browser_token(raw_app);
    let windows = unsafe { enum_visible_windows() };
    if is_browser_raw_app(raw_app) && !raw_tok.is_empty() {
        return windows
            .iter()
            .any(|w| w.stem == raw_tok || w.stem.contains(&raw_tok));
    }
    windows.iter().any(|w| {
        let s = w.stem.to_lowercase();
        let t = w.title.to_lowercase();
        (!raw_tok.is_empty() && (s == raw_tok || s.contains(&raw_tok)))
            || (!app_low.is_empty() && (s.contains(&app_low) || t.contains(&app_low)))
    })
}

/// Checks a browser call tab via UIA when that site is currently the active tab
/// in `hwnd`. Returns `Some(true)` if live call controls are present,
/// `Some(false)` if the site's document is active with >=5 visible controls and
/// NO live call controls exist, or `None` if inconclusive (e.g. user switched to
/// another tab or UIA is temporarily unavailable).
pub fn check_browser_call_ui_active(hwnd: isize, app_name: &str) -> Option<bool> {
    if !call_window_alive(hwnd) {
        return Some(false);
    }
    let app_low = app_name.trim().to_lowercase();
    let wt = window_title(hwnd);
    if !app_low.is_empty() && !wt.contains(&app_low) {
        return None;
    }

    std::thread::spawn(move || unsafe {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        };
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationLegacyIAccessiblePattern,
            TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
            UIA_ControlTypePropertyId, UIA_GroupControlTypeId, UIA_LegacyIAccessiblePatternId,
        };
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let mut wr = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut wr).is_err() {
            return None;
        }
        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut out: Option<bool> = None;

        if let Ok(uia) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        {
            if let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) {
                let doc_hit = find_active_browser_doc_for_hwnd(
                    &uia,
                    &root,
                    hwnd,
                    wr,
                    (!app_low.is_empty()).then_some(app_low.as_str()),
                );
                if let Some((doc, _)) = doc_hit {
                    const ACTIVE_KWS: &[&str] = &[
                        "akhiri panggilan",
                        "end call",
                        "tutup panggilan",
                        "leave call",
                        "disconnect",
                        "keluar panggilan",
                        "matikan mikrofon",
                        "nyalakan mikrofon",
                        "bisukan",
                        "unmute",
                        "mute",
                        "stop camera",
                        "matikan kamera",
                        "bagikan layar",
                        "share screen",
                    ];
                    let mut visible_controls = 0usize;
                    let mut found_call_ctrl = false;
                    for ct in [
                        UIA_ButtonControlTypeId.0,
                        UIA_GroupControlTypeId.0,
                        UIA_CheckBoxControlTypeId.0,
                    ] {
                        let v = make_uia_i4(ct);
                        let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v)
                        else {
                            continue;
                        };
                        let Ok(els) = doc.FindAll(TreeScope_Descendants, &cond) else {
                            continue;
                        };
                        let n = els.Length().unwrap_or(0).min(180);
                        for i in 0..n {
                            let Ok(el) = els.GetElement(i) else { continue };
                            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                                continue;
                            }
                            visible_controls += 1;
                            let name = el
                                .CurrentName()
                                .map(|s| s.to_string().to_lowercase())
                                .unwrap_or_default();
                            let help = el
                                .CurrentHelpText()
                                .map(|s| s.to_string().to_lowercase())
                                .unwrap_or_default();
                            let (acc_name, acc_desc) = el
                                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                                    UIA_LegacyIAccessiblePatternId,
                                )
                                .ok()
                                .map(|acc| {
                                    (
                                        acc.CurrentName()
                                            .map(|s| s.to_string().to_lowercase())
                                            .unwrap_or_default(),
                                        acc.CurrentDescription()
                                            .map(|s| s.to_string().to_lowercase())
                                            .unwrap_or_default(),
                                    )
                                })
                                .unwrap_or_default();
                            let combined =
                                format!("{} {} {} {}", name, help, acc_name, acc_desc);
                            if ACTIVE_KWS.iter().any(|k| combined.contains(k)) {
                                found_call_ctrl = true;
                                break;
                            }
                        }
                        if found_call_ctrl {
                            break;
                        }
                    }
                    if found_call_ctrl {
                        out = Some(true);
                    } else if visible_controls >= 5 {
                        out = Some(false);
                    }
                }
            }
        }
        if init_ok {
            CoUninitialize();
        }
        out
    })
    .join()
    .ok()
    .flatten()
}

fn is_browser_toast_payload(payload: &NotifPayload) -> bool {
    let raw_tok = normalize_browser_token(&payload.raw_app);
    (!raw_tok.is_empty() && BROWSER_STEMS.contains(&raw_tok.as_str()))
        || (!payload.site.trim().is_empty() && payload.site.contains('.'))
}

fn focus_call_window_blocking(payload: &NotifPayload) -> bool {
    if let Ok(hwnd) = payload.launch.trim().parse::<isize>() {
        if hwnd != 0 && call_window_alive(hwnd) && win::focus_hwnd(hwnd) {
            return true;
        }
    }
    if is_zapfast_hint(&payload.raw_app) || is_zapfast_hint(&payload.app) {
        return focus_zapfast_window_only(&payload.raw_app);
    }
    if is_browser_toast_payload(payload) {
        let url = crate::backend::notif::target_url_of(
            &payload.launch,
            &payload.jid,
            &payload.site,
        );
        let prof = crate::backend::notif::parse_profile(&payload.launch);
        let tok = normalize_browser_token(&payload.raw_app);
        if !url.is_empty() {
            let res = notif_open_url_blocking(&url, &tok, &prof, "");
            return res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        }
    }
    let hint = if payload.raw_app.trim().is_empty() {
        payload.app.as_str()
    } else {
        payload.raw_app.as_str()
    };
    let tokens = extract_app_focus_tokens(hint, &payload.title);
    let list = unsafe { enum_visible_windows() };
    for tok in &tokens {
        if let Some(w) = list
            .iter()
            .find(|w| w.stem == *tok || w.stem.contains(tok.as_str()) || w.title.to_lowercase().contains(tok.as_str()))
        {
            if win::focus_hwnd(w.hwnd) {
                return true;
            }
        }
    }
    let first_tok = tokens.first().map(String::as_str).unwrap_or("");
    launch_app(hint, first_tok)
}

pub async fn focus_call_window_no_uia(payload: NotifPayload) -> bool {
    tokio::task::spawn_blocking(move || focus_call_window_blocking(&payload))
        .await
        .unwrap_or(false)
}

/// Pure decision table for notification tab reuse, unit-testable without
/// UIA. `tab_verified` means the site tab was selected AND confirmed in the
/// source browser; `nav_ok`/`chat_ok` are the results of the follow-up
/// action. Without a verified tab, a specific destination is opened through
/// the source exe (`fallback_action == "spawned"`) — a new tab is acceptable
/// there because focus-only would leave the chat unopened. A bare site root
/// never spawns a duplicate tab while a source browser window exists.
fn route_decision(
    tab_verified: bool,
    has_specific_dest: bool,
    is_wa: bool,
    has_sender: bool,
    nav_ok: bool,
    chat_ok: bool,
    browser_running: bool,
    fallback_action: &str,
) -> &'static str {
    if tab_verified {
        if has_specific_dest {
            return if nav_ok { "navigated" } else { "focused" };
        }
        if is_wa && has_sender && chat_ok {
            return "navigated";
        }
        return "focused";
    }
    if browser_running {
        if has_specific_dest && fallback_action == "spawned" {
            return "opened";
        }
        return "focused";
    }
    "opened"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voip_tracker_ends_after_consecutive_misses() {
        let mut t = VoipCallTracker::new();
        assert!(!t.is_tracking());
        // Misses before any detection never end a call.
        for _ in 0..10 {
            assert!(!t.observe(None));
        }
        assert!(!t.observe(Some("WhatsApp|Pam Pam is calling")));
        assert!(t.is_tracking());
        // Hangup: window gone for CALL_END_MISS_POLLS polls -> end once.
        for i in 1..CALL_END_MISS_POLLS {
            assert!(!t.observe(None), "miss {} ended too early", i);
        }
        assert!(t.observe(None));
        assert!(!t.is_tracking());
        // Further misses do not re-fire.
        for _ in 0..5 {
            assert!(!t.observe(None));
        }
    }

    #[test]
    fn voip_tracker_title_flicker_does_not_end_call() {
        let mut t = VoipCallTracker::new();
        t.observe(Some("WhatsApp|Pam Pam is calling"));
        assert!(!t.observe(None));
        assert!(!t.observe(None));
        // Title flickers back before the threshold: counter resets.
        assert!(!t.observe(Some("WhatsApp|Pam Pam is calling")));
        assert!(!t.observe(None));
        assert!(!t.observe(None));
        assert!(t.is_tracking());
        assert!(t.observe(None));
    }

    #[test]
    fn voip_tracker_new_call_rearms_end_signal() {
        let mut t = VoipCallTracker::new();
        t.observe(Some("WhatsApp|A is calling"));
        for _ in 0..CALL_END_MISS_POLLS {
            t.observe(None);
        }
        // Second call from another contact must produce its own end event.
        assert!(!t.observe(Some("WhatsApp|B is calling")));
        assert!(!t.observe(None));
        assert!(!t.observe(None));
        assert!(t.observe(None));
        assert!(!t.observe(None));
    }

    #[test]
    fn route_reuses_verified_tab_for_specific_destination() {
        // Same WA tab, different contact: navigate in place, never a new tab.
        assert_eq!(route_decision(true, true, true, true, true, false, true, "none"), "navigated");
        // Navigation keystrokes failed: stay focused, still no duplicate.
        assert_eq!(route_decision(true, true, true, true, false, false, true, "none"), "focused");
    }

    #[test]
    fn route_wa_sender_chat_only_on_unique_match() {
        assert_eq!(route_decision(true, false, true, true, false, true, true, "none"), "navigated");
        // Ambiguous or missing sender row: focus only.
        assert_eq!(route_decision(true, false, true, true, false, false, true, "none"), "focused");
        assert_eq!(route_decision(true, false, true, false, false, false, true, "none"), "focused");
        // Non-WA root URL without destination: focus only.
        assert_eq!(route_decision(true, false, false, true, false, false, true, "none"), "focused");
    }

    #[test]
    fn route_unverified_specific_dest_spawns_source_exe() {
        // Zen tab tree not exposed but a send?phone= link exists: the exe
        // remoting fallback opened the chat in a new tab — report "opened".
        assert_eq!(route_decision(false, true, true, true, false, false, true, "spawned"), "opened");
        // Non-WA explicit path (e.g. gmail message) behaves the same.
        assert_eq!(route_decision(false, true, false, false, false, false, true, "spawned"), "opened");
        // Spawn failed or exe unknown: honest focus-only, not "opened".
        assert_eq!(route_decision(false, true, true, true, false, false, true, "focus"), "focused");
        assert_eq!(route_decision(false, true, true, true, false, false, true, "none"), "focused");
    }

    #[test]
    fn route_root_url_never_duplicates_while_browser_running() {
        // Bare site root with the browser running: focus only, no new tab,
        // even if a spawn somehow happened.
        assert_eq!(route_decision(false, false, true, true, false, false, true, "none"), "focused");
        assert_eq!(route_decision(false, false, true, true, false, false, true, "spawned"), "focused");
        // Open a fresh destination only when no browser window exists.
        assert_eq!(route_decision(false, true, true, true, false, false, false, "none"), "opened");
        assert_eq!(route_decision(false, false, true, true, false, false, false, "none"), "opened");
    }

    #[test]
    fn detects_meet_zoom_teams_links() {
        let meet = detect_meeting_link("Join google meet now: https://meet.google.com/abc-defg-hij?authuser=0.").unwrap();
        assert_eq!(meet.provider, MeetingProvider::GoogleMeet);
        assert_eq!(meet.code, "abc-defg-hij");
        assert_eq!(meet.url, "https://meet.google.com/abc-defg-hij?authuser=0");

        let zoom = detect_meeting_link("Weekly Sync\nhttps://us05web.zoom.us/j/98765432100?pwd=Secret123\nThanks").unwrap();
        assert_eq!(zoom.provider, MeetingProvider::Zoom);
        assert_eq!(zoom.code, "98765432100");
        assert_eq!(zoom.pwd, "Secret123");

        let teams = detect_meeting_link("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc/0").unwrap();
        assert_eq!(teams.provider, MeetingProvider::Teams);
    }

    #[test]
    fn recursive_search_finds_nested_files_and_skips_noise() {
        let tmp = std::env::temp_dir().join(format!(
            "dn_search_test_{}_{}",
            std::process::id(),
            crate::state::now_unix_ms()
        ));
        let nested = tmp.join("Job Application");
        let ignored = tmp.join("node_modules");
        let _ = std::fs::create_dir_all(&nested);
        let _ = std::fs::create_dir_all(&ignored);
        let cv_path = nested.join("CV_ATS_Test.docx");
        let noise_path = ignored.join("CV_ignored_noise.docx");
        let _ = std::fs::write(&cv_path, b"dummy");
        let _ = std::fs::write(&noise_path, b"dummy");

        let hits = search_files_in_roots(&[tmp.clone()], "cv", 4, 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].get("name").and_then(|v| v.as_str()), Some("CV_ATS_Test.docx"));
        assert_eq!(
            hits[0].get("path").and_then(|v| v.as_str()),
            Some(cv_path.to_string_lossy().as_ref())
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn wa_sender_variants_handles_groups_and_counts() {
        assert_eq!(
            wa_sender_variants("fikar @ Koordinasi PDD X Acara"),
            vec![
                "Koordinasi PDD X Acara",
                "fikar",
                "fikar @ Koordinasi PDD X Acara"
            ]
        );
        assert_eq!(
            wa_sender_variants("Pam Pam (3 pesan)"),
            vec!["Pam Pam"]
        );
        assert_eq!(
            wa_sender_variants("Koordinasi PDD X Acara (8 pesan): fikar"),
            vec![
                "Koordinasi PDD X Acara",
                "Koordinasi PDD X Acara (8 pesan): fikar"
            ]
        );
    }

    #[test]
    fn gemini_and_subdomain_tab_matching() {
        let (is_root, kws, _) = parse_url_tab_info("https://gemini.google.com");
        assert!(is_root);
        assert!(kws.iter().any(|k| k == "gemini"));
        assert!(!kws.iter().any(|k| k == "google"));

        let (_, doc_kws, _) = parse_url_tab_info("https://docs.google.com/document/d/123");
        assert!(doc_kws.iter().any(|k| k == "docs" || k == "google docs"));
    }

    #[test]
    fn app_focus_tokens_and_protocol_uri_validation() {
        let agy = extract_app_focus_tokens("electron.app.Antigravity", "Antigravity");
        assert!(agy.iter().any(|t| t == "antigravity"));
        assert!(!agy.iter().any(|t| t == "electron" || t == "app"));

        let zc = extract_app_focus_tokens("dev.zcode.app", "Task completed");
        assert!(zc.iter().any(|t| t == "zcode"));
        assert!(!zc.iter().any(|t| t.contains("task completed")));

        let nv = extract_app_focus_tokens("com.nvidia.nvapp", "New Reward Available");
        assert!(nv.iter().any(|t| t == "nvapp"));
        assert!(nv.iter().any(|t| t == "nvidia"));

        assert!(is_safe_protocol_uri("nvidiaapp://route/#nvapp/rewards"));
        assert!(is_safe_protocol_uri("windowsdefender://fullhistory"));
        assert!(is_safe_protocol_uri("antigravity-ide://google.google-antigravity/open-conversation?cascadeId=123"));
        assert!(!is_safe_protocol_uri("file:///C:/Windows/System32/calc.exe"));
        assert!(!is_safe_protocol_uri("javascript:alert(1)"));
        assert!(!is_safe_protocol_uri("https://example.com"));
    }

    #[test]
    fn wa_chrome_text_filter_blocks_ui_labels() {
        let variants = vec!["gianl".to_string()];
        assert!(is_wa_chrome_text("whatsapp", &variants));
        assert!(is_wa_chrome_text("unduh whatsapp untuk windows", &variants));
        assert!(is_wa_chrome_text("tanya meta ai atau cari", &variants));
        assert!(!is_wa_chrome_text("gianl", &variants));
        assert!(is_wa_chrome_text("whatsapp web", &variants));

        assert!(!is_wa_chrome_text_ext(
            "gianl pesan whatsapp masuk belum dibaca",
            &variants,
            true
        ));
        assert!(is_wa_chrome_text_ext(
            "unduh whatsapp untuk windows",
            &variants,
            true
        ));
        assert_eq!(
            wa_sender_variants("Tim Proyek  Utama"),
            vec!["Tim Proyek Utama"]
        );
    }

    #[test]
    fn gecko_switch_to_tab_and_root_spawn_rules() {
        assert!(is_gecko_switch_to_tab_label("gemini — gemini.google.com switch to tab"));
        assert!(is_gecko_switch_to_tab_label("whatsapp web beralih ke tab"));
        assert!(is_gecko_switch_to_tab_label("pindah ke tab web.whatsapp.com"));
        assert!(!is_gecko_switch_to_tab_label("gemini google search"));

        assert!(!should_spawn_root_tab(true, 0, false));
        assert!(!should_spawn_root_tab(true, 3, false));
        assert!(!should_spawn_root_tab(true, 0, true));
        assert!(should_spawn_root_tab(true, 3, true));
        assert!(!should_spawn_root_tab(false, 0, false));
        assert!(should_spawn_root_tab(false, 2, false));
    }

    #[test]
    fn zapfast_detection_and_query_cleaning_never_touches_wa_web() {
        assert!(is_zapfast_hint("me.paolino.zapfast"));
        assert!(is_zapfast_hint("ZapFast"));
        assert!(!is_zapfast_hint("ZenToast-F0DC299D809B9700"));
        assert!(!is_zapfast_hint("5319275A.WhatsAppDesktop"));

        assert_eq!(
            clean_zapfast_chat_query("\u{200e}Tim Proyek   Utama"),
            "Tim Proyek Utama"
        );
        assert_eq!(clean_zapfast_chat_query("~ Budi"), "Budi");
        assert_eq!(clean_zapfast_chat_query("ZapFast"), "");
        assert_eq!(clean_zapfast_chat_query("   "), "");
    }

    #[test]
    fn zapfast_composer_vs_search_and_pane_classification() {
        use windows::Win32::Foundation::RECT;

        assert!(zapfast_is_composer_name("ketik pesan"));
        assert!(zapfast_is_composer_name("type a message"));
        assert!(zapfast_is_composer_name("balas ke fikar"));
        assert!(zapfast_is_composer_name("pesan"));
        assert!(!zapfast_is_composer_name("cari pesan"));
        assert!(!zapfast_is_composer_name("search messages"));
        assert!(!zapfast_is_composer_name("cari atau mulai obrolan"));

        assert!(zapfast_is_search_name("cari atau mulai obrolan"));
        assert!(zapfast_is_search_name("search chats"));
        assert!(!zapfast_is_search_name("ketik pesan"));
        assert!(!zapfast_is_search_name("type a message"));

        let win = RECT {
            left: 100,
            top: 50,
            right: 1300,
            bottom: 850,
        };
        let sidebar_item = RECT {
            left: 120,
            top: 200,
            right: 430,
            bottom: 265,
        };
        let chat_header = RECT {
            left: 480,
            top: 70,
            right: 720,
            bottom: 110,
        };
        let composer = RECT {
            left: 480,
            top: 785,
            right: 1240,
            bottom: 830,
        };
        assert_eq!(zapfast_pane_role(sidebar_item, win), ZapFastPane::LeftSidebar);
        assert_eq!(zapfast_pane_role(chat_header, win), ZapFastPane::ChatHeader);
        assert_eq!(zapfast_pane_role(composer, win), ZapFastPane::ChatComposerZone);

        let vars = vec!["fikar".to_string()];
        assert!(zapfast_text_matches_contact("Fikar", &vars));
        assert!(zapfast_text_matches_contact("Fikar 09:12 Halo", &vars));
        assert!(!zapfast_text_matches_contact("Cari atau mulai obrolan", &vars));
    }

    #[test]
    fn strip_verbatim_and_start_menu_path_helper() {
        assert_eq!(
            strip_verbatim_prefix_path(Path::new(r"\\?\D:\Apps\SelaBar.exe")),
            std::path::PathBuf::from(r"D:\Apps\SelaBar.exe")
        );
        assert_eq!(
            strip_verbatim_prefix_path(Path::new(r"\\?\UNC\server\share\SelaBar.exe")),
            std::path::PathBuf::from(r"\\server\share\SelaBar.exe")
        );
        if let Some(p) = start_menu_shortcut_path() {
            let norm = p.to_string_lossy().replace('/', "\\");
            assert!(
                norm.ends_with(r"Start Menu\Programs\SelaBar.lnk"),
                "unexpected Start Menu path: {}",
                norm
            );
        }
    }
}
