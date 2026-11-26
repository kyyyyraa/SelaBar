use crate::events::{BatteryState, HwndHandle};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_F5,
    VK_HOME, VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

static PASTE_TARGET: OnceLock<Mutex<Option<isize>>> = OnceLock::new();

fn paste_target_slot() -> &'static Mutex<Option<isize>> {
    PASTE_TARGET.get_or_init(|| Mutex::new(None))
}

pub fn battery_state() -> BatteryState {
    let mut status = SYSTEM_POWER_STATUS {
        ACLineStatus: 0,
        BatteryFlag: 255,
        BatteryLifePercent: 255,
        SystemStatusFlag: 0,
        BatteryLifeTime: 0,
        BatteryFullLifeTime: 0,
    };
    let ok = unsafe { GetSystemPowerStatus(&mut status) }.is_ok();
    if !ok {
        return BatteryState::default();
    }

    let has_battery = status.BatteryFlag != 8 && status.BatteryFlag != 255;
    let on_ac = status.ACLineStatus == 1;
    let charging = if has_battery {
        (status.BatteryFlag & 4) != 0 || (on_ac && (status.BatteryFlag & 1) == 0)
    } else {
        on_ac
    };
    let level = if has_battery && status.BatteryLifePercent != 255 {
        f64::from(status.BatteryLifePercent) / 100.0
    } else {
        1.0
    };
    BatteryState {
        level,
        charging,
        supported: has_battery,
    }
}

pub fn get_foreground_window_info() -> Value {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return json!({ "hwnd": 0, "title": "" });
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        let title = String::from_utf16_lossy(&buf[..len]);
        json!({ "hwnd": hwnd.0 as usize, "title": title })
    }
}

pub fn capture_paste_target(self_hwnd: HwndHandle) -> isize {
    let hwnd = unsafe { GetForegroundWindow() };
    let val = hwnd.0 as isize;
    if val == 0 || (self_hwnd.is_valid() && val == self_hwnd.0) {
        return paste_target_slot()
            .lock()
            .ok()
            .and_then(|s| *s)
            .unwrap_or(0);
    }
    if let Ok(mut slot) = paste_target_slot().lock() {
        *slot = Some(val);
    }
    val
}

pub fn restore_paste_target() {
    if let Ok(mut slot) = paste_target_slot().lock() {
        *slot = None;
    }
}

pub fn stored_paste_target() -> Option<isize> {
    paste_target_slot().lock().ok().and_then(|slot| *slot)
}

pub fn focus_hwnd(target_val: isize) -> bool {
    if target_val == 0 {
        return false;
    }
    let target = HWND(target_val as *mut _);
    unsafe {
        if GetForegroundWindow() == target {
            return true;
        }
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        let current_thread = GetCurrentThreadId();
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let target_thread = GetWindowThreadProcessId(target, None);

        let mut attached_fg = false;
        let mut attached_target = false;

        if foreground_thread != 0 && foreground_thread != current_thread {
            attached_fg = AttachThreadInput(current_thread, foreground_thread, true).as_bool();
        }
        if target_thread != 0 && target_thread != current_thread {
            attached_target = AttachThreadInput(current_thread, target_thread, true).as_bool();
        }

        let _ = SetForegroundWindow(target);

        if attached_fg {
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
        if attached_target {
            let _ = AttachThreadInput(current_thread, target_thread, false);
        }

        GetForegroundWindow() == target
    }
}

fn write_clipboard_utf16(text: &str) -> Result<(), &'static str> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("clipboard busy");
        }
        let _ = EmptyClipboard();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            let _ = CloseClipboard();
            return Err("clipboard alloc failed");
        };
        let dst = GlobalLock(mem);
        if dst.is_null() {
            let _ = CloseClipboard();
            return Err("clipboard lock failed");
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, dst as *mut u8, bytes);
        let _ = GlobalUnlock(mem);

        if SetClipboardData(
            CF_UNICODETEXT.0 as u32,
            Some(windows::Win32::Foundation::HANDLE(mem.0)),
        )
        .is_err()
        {
            let _ = CloseClipboard();
            return Err("clipboard write failed");
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

fn make_key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub fn paste_text(text: &str, self_hwnd: HwndHandle) -> Result<(), String> {
    if text.is_empty() {
        return Err("empty".to_string());
    }
    let cur_fg = unsafe { GetForegroundWindow().0 as isize };
    let target = if cur_fg != 0 && (!self_hwnd.is_valid() || cur_fg != self_hwnd.0) {
        cur_fg
    } else {
        match stored_paste_target() {
            Some(h) if h != 0 && (!self_hwnd.is_valid() || h != self_hwnd.0) => h,
            _ => 0,
        }
    };
    if target == 0 {
        return Err("no target window".to_string());
    }
    if cur_fg != target {
        let _ = focus_hwnd(target);
        std::thread::sleep(std::time::Duration::from_millis(55));
        let after_fg = unsafe { GetForegroundWindow().0 as isize };
        if after_fg != target {
            return Err("foreground-lock".to_string());
        }
    }
    if !crate::backend::clipboard::copy_text_silent(text) {
        if let Err(reason) = write_clipboard_utf16(text) {
            return Err(reason.to_string());
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(35));

    let vk_v = VIRTUAL_KEY(0x56);
    let inputs = [
        make_key_input(VK_CONTROL, false),
        make_key_input(vk_v, false),
        make_key_input(vk_v, true),
        make_key_input(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err("keystroke injection failed".to_string());
    }
    Ok(())
}

/// Strictly allowlisted key combinations matching `dn-voice-exec.js` (`press_key`).
pub fn send_keys(combo: &str) -> Result<(), String> {
    let norm = combo
        .trim()
        .to_lowercase()
        .replace("escape", "esc")
        .replace("return", "enter")
        .replace("control+", "ctrl+");

    let single_vk = |vk: VIRTUAL_KEY| vec![make_key_input(vk, false), make_key_input(vk, true)];
    let inputs: Vec<INPUT> = match norm.as_str() {
        "ctrl+s" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x53), false),
            make_key_input(VIRTUAL_KEY(0x53), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+c" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x43), false),
            make_key_input(VIRTUAL_KEY(0x43), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+v" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x56), false),
            make_key_input(VIRTUAL_KEY(0x56), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+z" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x5A), false),
            make_key_input(VIRTUAL_KEY(0x5A), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+a" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x41), false),
            make_key_input(VIRTUAL_KEY(0x41), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+l" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x4C), false),
            make_key_input(VIRTUAL_KEY(0x4C), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+k" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x4B), false),
            make_key_input(VIRTUAL_KEY(0x4B), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+t" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x54), false),
            make_key_input(VIRTUAL_KEY(0x54), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+r" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x52), false),
            make_key_input(VIRTUAL_KEY(0x52), true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+tab" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VK_TAB, false),
            make_key_input(VK_TAB, true),
            make_key_input(VK_CONTROL, true),
        ],
        "ctrl+shift+tab" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VK_SHIFT, false),
            make_key_input(VK_TAB, false),
            make_key_input(VK_TAB, true),
            make_key_input(VK_SHIFT, true),
            make_key_input(VK_CONTROL, true),
        ],
        "f5" => single_vk(VK_F5),
        "alt+left" => vec![
            make_key_input(VK_MENU, false),
            make_key_input(VK_LEFT, false),
            make_key_input(VK_LEFT, true),
            make_key_input(VK_MENU, true),
        ],
        "alt+right" => vec![
            make_key_input(VK_MENU, false),
            make_key_input(VK_RIGHT, false),
            make_key_input(VK_RIGHT, true),
            make_key_input(VK_MENU, true),
        ],
        "k" => single_vk(VIRTUAL_KEY(0x4B)),
        "j" => single_vk(VIRTUAL_KEY(0x4A)),
        "l" => single_vk(VIRTUAL_KEY(0x4C)),
        "f" => single_vk(VIRTUAL_KEY(0x46)),
        "m" => single_vk(VIRTUAL_KEY(0x4D)),
        "c" => single_vk(VIRTUAL_KEY(0x43)),
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
            let b = norm.as_bytes()[0];
            single_vk(VIRTUAL_KEY(b as u16))
        }
        "enter" => single_vk(VK_RETURN),
        "esc" => single_vk(VK_ESCAPE),
        "tab" => single_vk(VK_TAB),
        "space" => single_vk(VK_SPACE),
        "backspace" => single_vk(VK_BACK),
        "up" => single_vk(VK_UP),
        "down" => single_vk(VK_DOWN),
        "left" => single_vk(VK_LEFT),
        "right" => single_vk(VK_RIGHT),
        "pgdn" | "pagedown" | "page_down" => single_vk(VK_NEXT),
        "pgup" | "pageup" | "page_up" => single_vk(VK_PRIOR),
        "home" => single_vk(VK_HOME),
        "end" => single_vk(VK_END),
        "delete" | "del" => single_vk(VK_DELETE),
        "ctrl+w" => vec![
            make_key_input(VK_CONTROL, false),
            make_key_input(VIRTUAL_KEY(0x57), false),
            make_key_input(VIRTUAL_KEY(0x57), true),
            make_key_input(VK_CONTROL, true),
        ],
        "alt+f4" => vec![
            make_key_input(VK_MENU, false),
            make_key_input(VK_F4, false),
            make_key_input(VK_F4, true),
            make_key_input(VK_MENU, true),
        ],
        _ => return Err(format!("tombol '{}' tidak diizinkan", norm)),
    };

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err("SendInput gagal".to_string());
    }
    Ok(())
}
