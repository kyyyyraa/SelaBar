use crate::backend::store::StoreManager;
use crate::events::{DisplayInfo, HwndHandle};
use std::sync::atomic::{AtomicU32, Ordering};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};

const MONITORINFOF_PRIMARY: u32 = 1;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, SetWindowPos,
    GWL_EXSTYLE, GW_HWNDPREV, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING,
    SWP_NOSIZE, SWP_NOZORDER, WS_EX_TOPMOST,
};

#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(
        hwnd: *mut std::ffi::c_void,
        dwAttribute: u32,
        pvAttribute: *mut std::ffi::c_void,
        cbAttribute: u32,
    ) -> i32;
}

#[derive(Debug, Clone)]
pub struct PhysicalMonitorInfo {
    pub id: String,
    pub phys_x: i32,
    pub phys_y: i32,
    pub phys_w: u32,
    pub phys_h: u32,
    pub work_x: i32,
    pub work_y: i32,
    pub work_w: u32,
    pub work_h: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

unsafe extern "system" fn enum_mon_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<PhysicalMonitorInfo>);
    let mut info: MONITORINFOEXW = std::mem::zeroed();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut _).as_bool() {
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let name = String::from_utf16_lossy(&info.szDevice[..len]);
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = if dpi_x > 0 { dpi_x as f64 / 96.0 } else { 1.0 };

        let rc = info.monitorInfo.rcMonitor;
        let wc = info.monitorInfo.rcWork;
        let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;

        list.push(PhysicalMonitorInfo {
            id: name,
            phys_x: rc.left,
            phys_y: rc.top,
            phys_w: (rc.right - rc.left).max(1) as u32,
            phys_h: (rc.bottom - rc.top).max(1) as u32,
            work_x: wc.left,
            work_y: wc.top,
            work_w: (wc.right - wc.left).max(1) as u32,
            work_h: (wc.bottom - wc.top).max(1) as u32,
            scale_factor: scale,
            is_primary,
        });
    }
    BOOL(1)
}

pub fn enumerate_monitors() -> Vec<PhysicalMonitorInfo> {
    let mut list = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_mon_proc),
            LPARAM(&mut list as *mut Vec<PhysicalMonitorInfo> as isize),
        );
    }
    list
}

pub fn get_displays_info() -> Vec<DisplayInfo> {
    enumerate_monitors()
        .into_iter()
        .enumerate()
        .map(|(idx, m)| {
            let s = if m.scale_factor > 0.0 {
                m.scale_factor
            } else {
                1.0
            };
            let w_log = (m.phys_w as f64 / s).round() as u32;
            let h_log = (m.phys_h as f64 / s).round() as u32;
            let res = format!("{}×{}", w_log, h_log);
            let name = if m.id.is_empty() {
                format!("Display {}", idx + 1)
            } else {
                m.id.clone()
            };
            let label = if m.is_primary {
                format!("{} (Primary - {})", name, res)
            } else {
                format!("{} ({})", name, res)
            };
            DisplayInfo {
                id: name,
                label,
                x: (m.phys_x as f64 / s).round() as i32,
                y: (m.phys_y as f64 / s).round() as i32,
                width: w_log,
                height: h_log,
                work_x: (m.work_x as f64 / s).round() as i32,
                work_y: (m.work_y as f64 / s).round() as i32,
                work_width: (m.work_w as f64 / s).round() as u32,
                work_height: (m.work_h as f64 / s).round() as u32,
                scale_factor: s,
                is_primary: m.is_primary,
            }
        })
        .collect()
}

static NOTCH_UI_SCALE_PCT: AtomicU32 = AtomicU32::new(100);

pub fn set_notch_ui_scale(scale: f32) {
    let pct = (scale.clamp(0.75, 1.35) * 100.0).round() as u32;
    NOTCH_UI_SCALE_PCT.store(pct, Ordering::Relaxed);
}

pub fn notch_ui_scale() -> f64 {
    (NOTCH_UI_SCALE_PCT.load(Ordering::Relaxed).max(75) as f64) / 100.0
}

pub fn target_window_geometry(store: &StoreManager) -> Option<(i32, i32, u32, u32, f64)> {
    let settings = store.get_settings();
    let ui_scale = settings.notch_scale();
    set_notch_ui_scale(ui_scale);
    let selected_id = settings.selected_display_id.as_str();
    let monitors = enumerate_monitors();
    if monitors.is_empty() {
        return None;
    }

    let target = if selected_id != "primary" && selected_id != "all" {
        monitors
            .iter()
            .find(|m| m.id == selected_id)
            .or_else(|| monitors.iter().find(|m| m.is_primary))
            .unwrap_or(&monitors[0])
    } else {
        monitors
            .iter()
            .find(|m| m.is_primary)
            .unwrap_or(&monitors[0])
    };

    let eff_scale = target.scale_factor * ui_scale as f64;
    let max_log_w = 1140.0_f64;
    let target_w = ((max_log_w * eff_scale).round() as u32)
        .min(target.phys_w)
        .max(320);
    let target_x = target.phys_x + ((target.phys_w as i32 - target_w as i32) / 2);
    let target_h = ((410.0 * eff_scale).round() as u32)
        .min(target.phys_h)
        .max(160);
    Some((
        target_x,
        target.phys_y,
        target_w,
        target_h,
        eff_scale,
    ))
}

pub fn sync_notch_window(hwnd: HwndHandle, store: &StoreManager) -> Option<(i32, i32, u32, u32, f64)> {
    if !hwnd.is_valid() {
        return None;
    }
    let geom = target_window_geometry(store)?;
    let (x, y, w, h, _) = geom;
    unsafe {
        let raw_hwnd = windows::Win32::Foundation::HWND(hwnd.0 as *mut _);
        let _ = SetWindowPos(
            raw_hwnd,
            None,
            x,
            y,
            w as i32,
            h as i32,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    sync_zorder_with_myfinder(hwnd);
    Some(geom)
}

pub fn window_scale_factor(hwnd: HwndHandle) -> f64 {
    let ui_scale = notch_ui_scale();
    if !hwnd.is_valid() {
        return ui_scale;
    }
    unsafe {
        let dpi = GetDpiForWindow(windows::Win32::Foundation::HWND(hwnd.0 as *mut _));
        let dpi_scale = if dpi > 0 { dpi as f64 / 96.0 } else { 1.0 };
        dpi_scale * ui_scale
    }
}

unsafe fn is_window_uncloaked_visible(hwnd: windows::Win32::Foundation::HWND) -> bool {
    if !IsWindowVisible(hwnd).as_bool() {
        return false;
    }
    const DWMWA_CLOAKED: u32 = 14;
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd.0,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut std::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    );
    hr != 0 || cloaked == 0
}

pub fn sync_zorder_with_myfinder(hwnd: HwndHandle) {
    if !hwnd.is_valid() {
        return;
    }
    unsafe {
        let notch_hwnd = windows::Win32::Foundation::HWND(hwnd.0 as *mut _);
        let ex = GetWindowLongPtrW(notch_hwnd, GWL_EXSTYLE);
        let has_topmost = (ex & (WS_EX_TOPMOST.0 as isize)) != 0;

        let mut nr = RECT::default();
        let has_nr = GetWindowRect(notch_hwnd, &mut nr).is_ok();
        let nr_cx = if has_nr { (nr.left + nr.right) / 2 } else { 0 };

        let mut competitor_above = false;
        let mut shell_flyout_above = false;
        let mut cur = GetWindow(notch_hwnd, GW_HWNDPREV).ok();
        let mut steps = 0usize;
        while let Some(prev) = cur {
            if prev.0.is_null() || steps >= 160 {
                break;
            }
            steps += 1;
            if is_window_uncloaked_visible(prev) {
                let mut r = RECT::default();
                let has_r = GetWindowRect(prev, &mut r).is_ok()
                    && r.right > r.left
                    && r.bottom > r.top;
                let mut cbuf = [0u16; 128];
                let clen = GetClassNameW(prev, &mut cbuf);
                let cname = if clen > 0 {
                    String::from_utf16_lossy(&cbuf[..clen as usize])
                } else {
                    String::new()
                };
                if matches!(cname.as_str(), "MultitaskingViewFrame" | "ScreenClippingHost") {
                    shell_flyout_above = true;
                }
                let clow = cname.to_ascii_lowercase();
                let overlaps_top = has_nr
                    && has_r
                    && r.top <= nr.top + 48
                    && r.bottom > nr.top
                    && (r.bottom - r.top) <= 140
                    && r.right > nr_cx - 240
                    && r.left < nr_cx + 240;
                if clow.starts_with("mydock") || clow.starts_with("myfinder") || overlaps_top {
                    competitor_above = true;
                    break;
                }
            }
            cur = GetWindow(prev, GW_HWNDPREV).ok();
        }

        if (!has_topmost || competitor_above) && !shell_flyout_above {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOSENDCHANGING;
            let _ = SetWindowPos(notch_hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        }
    }
}
