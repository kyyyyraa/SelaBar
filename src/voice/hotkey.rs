use crate::events::{PttAbortReason, PttMode, UiEvent, UiSender};
use crate::state::Settings;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_APP_ESC_SET: u32 = 0x8002;
pub const WM_APP_HOTKEYS_RELOAD: u32 = 0x8003;
pub const WM_APP_HOTKEYS_PAUSE: u32 = 0x8004;
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

pub const HOTKEY_COMMAND: i32 = 1;
pub const HOTKEY_DICTATE: i32 = 2;
pub const HOTKEY_ESCAPE: i32 = 3;
pub const HOTKEY_LIVE: i32 = 4;
pub const HOTKEY_CHAT: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBindings {
    pub voice: String,
    pub dictate: String,
    pub ask: String,
    pub chat: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            voice: "Ctrl+Alt+V".to_string(),
            dictate: "Ctrl+Alt+D".to_string(),
            ask: "Ctrl+Alt+C".to_string(),
            chat: "Alt+C".to_string(),
        }
    }
}

impl HotkeyBindings {
    pub fn from_settings(s: &Settings) -> Self {
        let def = Self::default();
        Self {
            voice: if s.hotkey_voice.trim().is_empty() { def.voice } else { s.hotkey_voice.clone() },
            dictate: if s.hotkey_dictate.trim().is_empty() { def.dictate } else { s.hotkey_dictate.clone() },
            ask: if s.hotkey_ask.trim().is_empty() { def.ask } else { s.hotkey_ask.clone() },
            chat: if s.hotkey_chat.trim().is_empty() { def.chat } else { s.hotkey_chat.clone() },
        }
    }
}

fn parse_key_token(tok: &str) -> Option<u32> {
    let up = tok.trim().to_uppercase();
    if up.len() == 1 {
        let b = up.as_bytes()[0];
        if b.is_ascii_alphabetic() || b.is_ascii_digit() {
            return Some(b as u32);
        }
        return match b {
            b'`' => Some(0xC0),
            b'-' => Some(0xBD),
            b'=' => Some(0xBB),
            b'[' => Some(0xDB),
            b']' => Some(0xDD),
            b'\\' => Some(0xDC),
            b';' => Some(0xBA),
            b'\'' => Some(0xDE),
            b',' => Some(0xBC),
            b'.' => Some(0xBE),
            b'/' => Some(0xBF),
            _ => None,
        };
    }
    match up.as_str() {
        "F1" => Some(0x70),
        "F2" => Some(0x71),
        "F3" => Some(0x72),
        "F4" => Some(0x73),
        "F5" => Some(0x74),
        "F6" => Some(0x75),
        "F7" => Some(0x76),
        "F8" => Some(0x77),
        "F9" => Some(0x78),
        "F10" => Some(0x79),
        "F11" => Some(0x7A),
        "F12" => Some(0x7B),
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "INSERT" | "INS" => Some(0x2D),
        "DELETE" | "DEL" => Some(0x2E),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PGUP" | "PAGEUP" => Some(0x21),
        "PGDN" | "PAGEDOWN" => Some(0x22),
        "UP" => Some(0x26),
        "DOWN" => Some(0x28),
        "LEFT" => Some(0x25),
        "RIGHT" => Some(0x27),
        _ => None,
    }
}

fn vk_to_label(vk: u32) -> Option<String> {
    if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
        return Some((vk as u8 as char).to_string());
    }
    if (0x70..=0x7B).contains(&vk) {
        return Some(format!("F{}", vk - 0x70 + 1));
    }
    let s = match vk {
        0x20 => "Space",
        0x09 => "Tab",
        0x2D => "Insert",
        0x2E => "Delete",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PgUp",
        0x22 => "PgDn",
        0x26 => "Up",
        0x28 => "Down",
        0x25 => "Left",
        0x27 => "Right",
        0xC0 => "`",
        0xBD => "-",
        0xBB => "=",
        0xDB => "[",
        0xDD => "]",
        0xDC => "\\",
        0xBA => ";",
        0xDE => "'",
        0xBC => ",",
        0xBE => ".",
        0xBF => "/",
        _ => return None,
    };
    Some(s.to_string())
}

/// Parses `"Ctrl+Alt+V"`, `"Shift+F9"`, etc. into `(fsModifiers | MOD_NOREPEAT, vk)`.
/// Rejects bare non-function keys and `Esc`.
pub fn parse_hotkey(spec: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    for raw in spec.split('+') {
        let tok = raw.trim();
        if tok.is_empty() {
            return None;
        }
        match tok.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => mods |= MOD_CONTROL,
            "ALT" | "MENU" => mods |= MOD_ALT,
            "SHIFT" => mods |= MOD_SHIFT,
            "WIN" | "SUPER" => mods |= MOD_WIN,
            _ => {
                if vk.is_some() {
                    return None;
                }
                vk = parse_key_token(tok);
                vk?;
            }
        }
    }
    let code = vk?;
    if mods == 0 && !(0x70..=0x7B).contains(&code) {
        return None;
    }
    Some((mods | MOD_NOREPEAT, code))
}

/// Formats a live key press into a normalized shortcut string (`"Ctrl+Alt+V"`),
/// returning `None` for standalone modifiers, `Esc`, or unmodified letter/digit keys.
pub fn format_hotkey_key_event(vk: u32, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
    if matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C | 0x1B) {
        return None;
    }
    if !(ctrl || alt || shift) && !(0x70..=0x7B).contains(&vk) {
        return None;
    }
    let key_lbl = vk_to_label(vk)?;
    let mut parts = Vec::with_capacity(4);
    if ctrl {
        parts.push("Ctrl".to_string());
    }
    if alt {
        parts.push("Alt".to_string());
    }
    if shift {
        parts.push("Shift".to_string());
    }
    parts.push(key_lbl);
    Some(parts.join("+"))
}

type HWND = *mut c_void;

#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[allow(non_snake_case)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: usize,
    pub lParam: isize,
    pub time: u32,
    pub pt: POINT,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hWnd: HWND, id: i32, fsModifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hWnd: HWND, id: i32) -> i32;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn PostThreadMessageW(idThread: u32, msg: u32, wParam: usize, lParam: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
}

unsafe fn register_hotkey_bindings(b: &HotkeyBindings) -> Vec<String> {
    let mut failed = Vec::new();
    let entries = [
        (HOTKEY_COMMAND, b.voice.as_str(), "Voice AI"),
        (HOTKEY_DICTATE, b.dictate.as_str(), "Dikte"),
        (HOTKEY_CHAT, b.chat.as_str(), "Chat AI"),
        (HOTKEY_LIVE, b.ask.as_str(), "Ask AI"),
    ];
    for (id, spec, _) in entries {
        if let Some((mods, vk)) = parse_hotkey(spec) {
            if RegisterHotKey(std::ptr::null_mut(), id, mods, vk) == 0 {
                failed.push(spec.to_string());
            }
        } else {
            failed.push(format!("{} (invalid)", spec));
        }
    }
    failed
}

unsafe fn unregister_action_hotkeys() {
    UnregisterHotKey(std::ptr::null_mut(), HOTKEY_COMMAND);
    UnregisterHotKey(std::ptr::null_mut(), HOTKEY_DICTATE);
    UnregisterHotKey(std::ptr::null_mut(), HOTKEY_CHAT);
    UnregisterHotKey(std::ptr::null_mut(), HOTKEY_LIVE);
}

#[derive(Clone)]
pub struct HotkeyManager {
    is_busy: Arc<AtomicBool>,
    is_live: Arc<AtomicBool>,
    esc_captured: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    bindings: Arc<Mutex<HotkeyBindings>>,
    reg_failed: Arc<Mutex<Vec<String>>>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        Self {
            is_busy: Arc::new(AtomicBool::new(false)),
            is_live: Arc::new(AtomicBool::new(false)),
            esc_captured: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            thread_id: Arc::new(AtomicU32::new(0)),
            bindings: Arc::new(Mutex::new(HotkeyBindings::default())),
            reg_failed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn registration_failures(&self) -> Vec<String> {
        self.reg_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_busy(&self, busy: bool) {
        self.is_busy.store(busy, Ordering::Relaxed);
    }

    pub fn is_busy(&self) -> bool {
        self.is_busy.load(Ordering::Relaxed)
    }

    pub fn set_live(&self, live: bool) {
        self.is_live.store(live, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        self.is_live.load(Ordering::Relaxed)
    }

    pub fn is_escape_captured(&self) -> bool {
        self.esc_captured.load(Ordering::SeqCst)
    }

    pub fn reload_bindings(&self, next: HotkeyBindings) {
        if let Ok(mut g) = self.bindings.lock() {
            if *g == next {
                return;
            }
            *g = next;
        }
        let tid = self.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                PostThreadMessageW(tid, WM_APP_HOTKEYS_RELOAD, 0, 0);
            }
        }
    }

    pub fn set_paused(&self, pause: bool) {
        let prev = self.paused.swap(pause, Ordering::SeqCst);
        if prev == pause {
            return;
        }
        let tid = self.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                PostThreadMessageW(tid, WM_APP_HOTKEYS_PAUSE, if pause { 1 } else { 0 }, 0);
            }
        }
    }

    /// Dynamically captures/releases bare `Esc` ONLY while voice/TTS/approval/ask overlay is active,
    /// so `Esc` in every other Windows application works normally when DeskNotch is idle.
    pub fn set_escape_captured(&self, enable: bool) {
        let prev = self.esc_captured.swap(enable, Ordering::SeqCst);
        if prev == enable {
            return;
        }
        let tid = self.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                PostThreadMessageW(tid, WM_APP_ESC_SET, if enable { 1 } else { 0 }, 0);
            }
        }
    }

    pub fn start_listener(&self, ui: UiSender, initial: HotkeyBindings) {
        if let Ok(mut g) = self.bindings.lock() {
            *g = initial;
        }
        let is_busy = Arc::clone(&self.is_busy);
        let esc_captured = Arc::clone(&self.esc_captured);
        let paused = Arc::clone(&self.paused);
        let thread_id = Arc::clone(&self.thread_id);
        let bindings = Arc::clone(&self.bindings);
        let reg_failed = Arc::clone(&self.reg_failed);

        std::thread::spawn(move || {
            unsafe {
                let tid = GetCurrentThreadId();
                thread_id.store(tid, Ordering::SeqCst);

                let init_b = bindings.lock().map(|g| g.clone()).unwrap_or_default();
                let failures = register_hotkey_bindings(&init_b);
                if let Ok(mut f) = reg_failed.lock() {
                    *f = failures.clone();
                }
                if !failures.is_empty() {
                    ui.send(UiEvent::HotkeyRegistrationFailures(failures));
                }

                let mut esc_registered = false;
                let mut esc_fail_logged = false;
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_APP_HOTKEYS_PAUSE {
                        unregister_action_hotkeys();
                        let is_paused = paused.load(Ordering::SeqCst);
                        if !is_paused {
                            let cur = bindings.lock().map(|g| g.clone()).unwrap_or_default();
                            let fails = register_hotkey_bindings(&cur);
                            if let Ok(mut f) = reg_failed.lock() {
                                *f = fails.clone();
                            }
                            ui.send(UiEvent::HotkeyRegistrationFailures(fails));
                        }
                        continue;
                    }

                    if msg.message == WM_APP_HOTKEYS_RELOAD {
                        unregister_action_hotkeys();
                        if !paused.load(Ordering::SeqCst) {
                            let cur = bindings.lock().map(|g| g.clone()).unwrap_or_default();
                            let fails = register_hotkey_bindings(&cur);
                            if let Ok(mut f) = reg_failed.lock() {
                                *f = fails.clone();
                            }
                            ui.send(UiEvent::HotkeyRegistrationFailures(fails));
                        }
                        continue;
                    }

                    if msg.message == WM_APP_ESC_SET {
                        let want = msg.wParam != 0;
                        if want && !esc_registered {
                            if RegisterHotKey(std::ptr::null_mut(), HOTKEY_ESCAPE, 0, 0x1B) != 0 {
                                esc_registered = true;
                                esc_fail_logged = false;
                            } else {
                                esc_captured.store(false, Ordering::SeqCst);
                                if !esc_fail_logged {
                                    esc_fail_logged = true;
                                    crate::backend::logfile::append("[hotkey] register escape failed");
                                }
                            }
                        } else if !want {
                            esc_fail_logged = false;
                            if esc_registered {
                                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ESCAPE);
                                esc_registered = false;
                            }
                        }
                        continue;
                    }

                    if msg.message == WM_HOTKEY {
                        match msg.wParam as i32 {
                            HOTKEY_COMMAND => {
                                let busy = is_busy.load(Ordering::Relaxed);
                                crate::backend::logfile::append(&format!(
                                    "[hotkey] cmd busy={} -> {}",
                                    busy,
                                    if busy { "finish" } else { "start" }
                                ));
                                if busy {
                                    is_busy.store(false, Ordering::Relaxed);
                                    ui.send(UiEvent::HotkeyFinishPtt);
                                } else {
                                    is_busy.store(true, Ordering::Relaxed);
                                    ui.send(UiEvent::HotkeyPttStart {
                                        mode: PttMode::Command,
                                    });
                                }
                            }
                            HOTKEY_DICTATE => {
                                let busy = is_busy.load(Ordering::Relaxed);
                                crate::backend::logfile::append(&format!(
                                    "[hotkey] dictate busy={} -> {}",
                                    busy,
                                    if busy { "finish" } else { "start" }
                                ));
                                if busy {
                                    is_busy.store(false, Ordering::Relaxed);
                                    ui.send(UiEvent::HotkeyFinishPtt);
                                } else {
                                    is_busy.store(true, Ordering::Relaxed);
                                    ui.send(UiEvent::HotkeyPttStart {
                                        mode: PttMode::Dictate,
                                    });
                                }
                            }
                            HOTKEY_ESCAPE => {
                                is_busy.store(false, Ordering::Relaxed);
                                crate::backend::logfile::append("[hotkey] escape -> abort");
                                ui.send(UiEvent::HotkeyVoiceStop);
                                ui.send(UiEvent::HotkeyPttAbort {
                                    mode: PttAbortReason::User,
                                });
                            }
                            HOTKEY_CHAT => {
                                crate::backend::logfile::append("[hotkey] chat -> open chat");
                                ui.send(UiEvent::HotkeyOpenChat);
                            }
                            HOTKEY_LIVE => {
                                crate::backend::logfile::append("[hotkey] ask -> open ask");
                                ui.send(UiEvent::HotkeyOpenAsk);
                            }
                            _ => {}
                        }
                    }
                }

                unregister_action_hotkeys();
                if esc_registered {
                    UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ESCAPE);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{format_hotkey_key_event, parse_hotkey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT};

    #[test]
    fn parses_valid_and_rejects_bare_hotkeys() {
        assert_eq!(parse_hotkey("Ctrl+Alt+V"), Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x56)));
        assert_eq!(parse_hotkey("alt + c"), Some((MOD_ALT | MOD_NOREPEAT, 0x43)));
        assert_eq!(parse_hotkey("Shift+Alt+Space"), Some((MOD_SHIFT | MOD_ALT | MOD_NOREPEAT, 0x20)));
        assert_eq!(parse_hotkey("F9"), Some((MOD_NOREPEAT, 0x78)));

        assert_eq!(parse_hotkey("V"), None);
        assert_eq!(parse_hotkey("Esc"), None);
        assert_eq!(parse_hotkey(""), None);
    }

    #[test]
    fn formats_key_events_safely() {
        assert_eq!(
            format_hotkey_key_event(0x56, true, true, false),
            Some("Ctrl+Alt+V".to_string())
        );
        assert_eq!(
            format_hotkey_key_event(0x78, false, false, false),
            Some("F9".to_string())
        );
        // Reject bare letters, Esc, and modifier-only keys
        assert_eq!(format_hotkey_key_event(0x56, false, false, false), None);
        assert_eq!(format_hotkey_key_event(0x1B, true, false, false), None);
        assert_eq!(format_hotkey_key_event(0x11, true, false, false), None);
    }
}
