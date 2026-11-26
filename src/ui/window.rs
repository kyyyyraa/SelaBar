use crate::backend::click_through::{self, ClickThroughManager};
use crate::backend::display::{sync_notch_window, window_scale_factor};
use crate::backend::store::StoreManager;
use crate::backend::win::capture_paste_target;
use crate::events::{HwndHandle, UiEvent, UiSender, WM_APP_WAKE};
use crate::ui::d2d::{D2dEngine, Rect};
use crate::ui::widgets::{CursorKind, KeyEvent, UiInput};
use crate::voice::hotkey::HotkeyManager;
use crossbeam_channel::Receiver;
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_MENU,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongPtrW,
    GetWindowRect, KillTimer, LoadCursorW, LoadIconW, PostQuitMessage, RegisterClassExW, SetCursor,
    SetForegroundWindow, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, GWLP_USERDATA,
    HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM, MA_NOACTIVATE, MSG, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_APP, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP, WS_VISIBLE,
};

const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_RENDER_ID: usize = 1;
const WM_APP_TRAY_QUIT: u32 = WM_APP + 99;

#[derive(Debug, Clone)]
pub struct FrameDirectives {
    pub cursor: CursorKind,
    pub wants_focus: bool,
    pub capture_escape: bool,
    pub interactive_bounds: Vec<Rect>,
    pub hide_in_screenshots: bool,
    pub is_animating: bool,
    pub target_display_id: String,
    pub request_redraw: bool,
}

impl Default for FrameDirectives {
    fn default() -> Self {
        Self {
            cursor: CursorKind::Arrow,
            wants_focus: false,
            capture_escape: false,
            interactive_bounds: Vec::new(),
            hide_in_screenshots: true,
            is_animating: false,
            target_display_id: String::new(),
            request_redraw: false,
        }
    }
}

pub trait UiApp {
    fn on_event(&mut self, ev: UiEvent);
    fn render(
        &mut self,
        d2d: &mut D2dEngine,
        input: &UiInput,
        canvas_w: f32,
        canvas_h: f32,
        scale: f32,
    ) -> FrameDirectives;
}

struct DibSurface {
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_obj: HGDIOBJ,
    w: u32,
    h: u32,
}

impl DibSurface {
    fn ensure(&mut self, w: u32, h: u32) -> bool {
        let w = w.max(1);
        let h = h.max(1);
        if self.w == w && self.h == h && !self.mem_dc.is_invalid() && !self.bitmap.is_invalid() {
            return true;
        }
        self.destroy();
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return false;
            }
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // Top-down DIB matching Direct2D coordinates
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return false;
            };
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            self.mem_dc = dc;
            self.bitmap = bmp;
            self.old_obj = old;
            self.w = w;
            self.h = h;
            true
        }
    }

    fn destroy(&mut self) {
        unsafe {
            if !self.mem_dc.is_invalid() {
                if !self.old_obj.is_invalid() {
                    SelectObject(self.mem_dc, self.old_obj);
                }
                if !self.bitmap.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                }
                let _ = DeleteDC(self.mem_dc);
            }
            self.mem_dc = HDC::default();
            self.bitmap = HBITMAP::default();
            self.old_obj = HGDIOBJ::default();
            self.w = 0;
            self.h = 0;
        }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        self.destroy();
    }
}

struct WindowHost<A: UiApp> {
    hwnd: HWND,
    app: A,
    d2d: D2dEngine,
    dib: DibSurface,
    rx: Receiver<UiEvent>,
    click_through: ClickThroughManager,
    hotkeys: HotkeyManager,
    store: StoreManager,
    input: UiInput,
    start_time: Instant,
    last_frame: Instant,
    pending_surrogate: Option<u16>,
    tracking_leave: bool,
    last_cursor: CursorKind,
    last_wants_focus: bool,
    ignore_cursor: bool,
    last_capture_escape: bool,
    last_esc_attempt: Instant,
    last_hide_capture: Option<bool>,
    last_display_id: String,
    timer_ms: u32,
    dirty: bool,
}

impl<A: UiApp> WindowHost<A> {
    fn sync_ex_style(&self) {
        unsafe {
            let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let no_act = WS_EX_NOACTIVATE.0 as isize;
            let trans = WS_EX_TRANSPARENT.0 as isize;
            let mut next = style
                | (WS_EX_LAYERED.0 as isize)
                | (WS_EX_TOPMOST.0 as isize)
                | (WS_EX_TOOLWINDOW.0 as isize);
            if self.last_wants_focus {
                next &= !no_act;
                next &= !trans;
            } else {
                next |= no_act;
                if self.ignore_cursor {
                    next |= trans;
                } else {
                    next &= !trans;
                }
            }
            if next != style {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next);
            }
        }
    }

    fn scale_and_phys_size(&self) -> (f32, i32, i32, u32, u32) {
        let mut wr = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut wr);
        }
        let pw = (wr.right - wr.left).max(1) as u32;
        let ph = (wr.bottom - wr.top).max(1) as u32;
        let scale = window_scale_factor(HwndHandle(self.hwnd.0 as isize)).max(0.5) as f32;
        (scale, wr.left, wr.top, pw, ph)
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                UiEvent::SetClickThrough(ignore) => {
                    if self.ignore_cursor != ignore {
                        self.ignore_cursor = ignore;
                        self.sync_ex_style();
                    }
                }
                other => {
                    self.app.on_event(other);
                    self.dirty = true;
                }
            }
        }
    }

    fn tick_and_render(&mut self) {
        self.drain_events();
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().clamp(0.001, 0.1);
        self.last_frame = now;
        self.input.now_ms = (now - self.start_time).as_millis() as u64;
        self.input.dt = dt;

        let (scale, win_x, win_y, phys_w, phys_h) = self.scale_and_phys_size();
        let canvas_w = phys_w as f32 / scale;
        let canvas_h = phys_h as f32 / scale;

        if !self.dib.ensure(phys_w, phys_h) {
            return;
        }
        if !self.d2d.begin_frame(self.dib.mem_dc, phys_w, phys_h, scale) {
            return;
        }

        let dir = self
            .app
            .render(&mut self.d2d, &self.input, canvas_w, canvas_h, scale);
        self.d2d.end_frame();

        unsafe {
            let pt_dst = POINT { x: win_x, y: win_y };
            let sz = SIZE {
                cx: phys_w as i32,
                cy: phys_h as i32,
            };
            let pt_src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pt_dst),
                Some(&sz),
                Some(self.dib.mem_dc),
                Some(&pt_src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }

        self.apply_directives(dir);

        // Reset transient per-frame input triggers
        self.input.left_pressed = false;
        self.input.left_released = false;
        self.input.right_pressed = false;
        self.input.wheel_y = 0.0;
        self.input.wheel_x = 0.0;
        self.input.keys.clear();
        self.input.chars.clear();
    }

    fn apply_directives(&mut self, dir: FrameDirectives) {
        self.last_cursor = dir.cursor;

        // 1. Update click-through hit regions
        let ct_rects: Vec<click_through::Rect> = dir
            .interactive_bounds
            .iter()
            .map(|r| click_through::Rect {
                x: r.x as f64,
                y: r.y as f64,
                width: r.w as f64,
                height: r.h as f64,
            })
            .collect();
        self.click_through.update_bounds(ct_rects);

        // 2. Dynamic Escape hotkey capture (reconciled with cooldown on conflict)
        if dir.capture_escape != self.last_capture_escape {
            self.last_capture_escape = dir.capture_escape;
            self.last_esc_attempt = Instant::now();
            self.hotkeys.set_escape_captured(dir.capture_escape);
        } else if dir.capture_escape != self.hotkeys.is_escape_captured()
            && self.last_esc_attempt.elapsed() >= Duration::from_secs(2)
        {
            self.last_esc_attempt = Instant::now();
            self.hotkeys.set_escape_captured(dir.capture_escape);
        }

        // 3. Dynamic WS_EX_NOACTIVATE + click-through toggle synchronized on UI thread
        if dir.wants_focus != self.last_wants_focus {
            self.last_wants_focus = dir.wants_focus;
            self.sync_ex_style();
            if dir.wants_focus {
                capture_paste_target(HwndHandle(self.hwnd.0 as isize));
                unsafe {
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
        }

        // 4. Screenshot display affinity
        let allow_capture = ["SELABAR_ALLOW_CAPTURE", "DESKNOTCH_ALLOW_CAPTURE"]
            .into_iter()
            .any(|k| {
                std::env::var(k)
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
            });
        let want_exclude = dir.hide_in_screenshots && !allow_capture;
        if self.last_hide_capture != Some(want_exclude) {
            self.last_hide_capture = Some(want_exclude);
            unsafe {
                let _ = SetWindowDisplayAffinity(
                    self.hwnd,
                    if want_exclude {
                        WDA_EXCLUDEFROMCAPTURE
                    } else {
                        WDA_NONE
                    },
                );
            }
        }

        // 5. Monitor move if target display changed
        if !dir.target_display_id.is_empty() && dir.target_display_id != self.last_display_id {
            self.last_display_id = dir.target_display_id.clone();
            let handle = HwndHandle(self.hwnd.0 as isize);
            let st = self.store.clone();
            sync_notch_window(handle, &st);
        }

        // 6. Adaptive timer cadence (14ms while animating/interacting, 250ms/1000ms idle)
        let desired_ms = if dir.is_animating || dir.request_redraw || self.dirty {
            14
        } else if !dir.interactive_bounds.is_empty() && dir.wants_focus {
            250
        } else {
            1000
        };
        self.dirty = dir.request_redraw;
        if desired_ms != self.timer_ms {
            self.timer_ms = desired_ms;
            unsafe {
                let _ = SetTimer(Some(self.hwnd), TIMER_RENDER_ID, desired_ms, None);
            }
        }
    }
}

pub fn run_window<A: UiApp + 'static>(
    app: A,
    rx: Receiver<UiEvent>,
    ui_sender: UiSender,
    click_through: ClickThroughManager,
    hotkeys: HotkeyManager,
    store: StoreManager,
    on_hwnd_ready: impl FnOnce(HwndHandle),
) -> Result<(), String> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst: HINSTANCE = GetModuleHandleW(None)
            .map(|h| HINSTANCE(h.0))
            .unwrap_or_default();

        let class_name: Vec<u16> = "SelaBarOverlayWnd\0".encode_utf16().collect();
        let h_icon = LoadIconW(Some(hinst), PCWSTR(1usize as *const u16)).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc::<A>),
            hInstance: hinst,
            hIcon: h_icon,
            hIconSm: h_icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let ex_style = WS_EX_LAYERED
            | WS_EX_TOPMOST
            | WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE
            | WS_EX_TRANSPARENT;
        let title: Vec<u16> = "SelaBar\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            1920,
            500,
            None,
            None,
            Some(hinst),
            None,
        )
        .map_err(|e| format!("CreateWindowExW failed: {e}"))?;

        let handle = HwndHandle(hwnd.0 as isize);
        ui_sender.set_hwnd(handle);
        sync_notch_window(handle, &store);
        on_hwnd_ready(handle);

        let d2d = D2dEngine::new()?;
        let now = Instant::now();
        let host = Rc::new(RefCell::new(WindowHost {
            hwnd,
            app,
            d2d,
            dib: DibSurface {
                mem_dc: HDC::default(),
                bitmap: HBITMAP::default(),
                old_obj: HGDIOBJ::default(),
                w: 0,
                h: 0,
            },
            rx,
            click_through,
            hotkeys,
            store,
            input: UiInput::default(),
            start_time: now,
            last_frame: now,
            pending_surrogate: None,
            tracking_leave: false,
            last_cursor: CursorKind::Arrow,
            last_wants_focus: false,
            ignore_cursor: true,
            last_capture_escape: false,
            last_esc_attempt: now - Duration::from_secs(5),
            last_hide_capture: None,
            last_display_id: String::new(),
            timer_ms: 16,
            dirty: true,
        }));

        let raw_ptr = Rc::into_raw(host.clone());
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw_ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetTimer(Some(hwnd), TIMER_RENDER_ID, 16, None);
        host.borrow_mut().tick_and_render();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_APP_TRAY_QUIT {
                let _ = DestroyWindow(hwnd);
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<WindowHost<A>>;
        if !ptr.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(ptr));
        }
    }
    Ok(())
}

unsafe fn host_from_hwnd<A: UiApp>(hwnd: HWND) -> Option<Rc<RefCell<WindowHost<A>>>> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<WindowHost<A>>;
    if ptr.is_null() {
        return None;
    }
    let rc = Rc::from_raw(ptr);
    let cloned = Rc::clone(&rc);
    std::mem::forget(rc);
    Some(cloned)
}

unsafe extern "system" fn wnd_proc<A: UiApp + 'static>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if rc.try_borrow().map(|h| h.last_wants_focus).unwrap_or(false) {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
            }
            return LRESULT(MA_NOACTIVATE as isize);
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                let cursor_kind = host_from_hwnd::<A>(hwnd)
                    .and_then(|rc| rc.try_borrow().ok().map(|h| h.last_cursor))
                    .unwrap_or(CursorKind::Arrow);
                let id = match cursor_kind {
                    CursorKind::Arrow => IDC_ARROW,
                    CursorKind::Pointer => IDC_HAND,
                    CursorKind::Text => IDC_IBEAM,
                };
                if let Ok(hc) = LoadCursorW(None, id) {
                    SetCursor(Some(hc));
                }
                return LRESULT(1);
            }
        }
        WM_MOUSEMOVE => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let Ok(mut h) = rc.try_borrow_mut() else {
                    return LRESULT(0);
                };
                let scale = window_scale_factor(HwndHandle(hwnd.0 as isize)).max(0.5) as f32;
                let lx = (lparam.0 & 0xFFFF) as i16 as f32 / scale;
                let ly = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / scale;
                h.input.mouse_x = lx;
                h.input.mouse_y = ly;
                let first_enter = !h.input.mouse_inside;
                h.input.mouse_inside = true;
                if !h.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut tme).is_ok() {
                        h.tracking_leave = true;
                    }
                }
                let should_render = first_enter || h.timer_ms > 16;
                drop(h);
                if should_render {
                    if let Ok(mut h) = rc.try_borrow_mut() {
                        h.tick_and_render();
                    }
                }
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.tracking_leave = false;
                    h.input.mouse_inside = false;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if let Ok(mut h) = rc.try_borrow_mut() {
                    let scale = window_scale_factor(HwndHandle(hwnd.0 as isize)).max(0.5) as f32;
                    h.input.mouse_x = (lparam.0 & 0xFFFF) as i16 as f32 / scale;
                    h.input.mouse_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / scale;
                    h.input.mouse_inside = true;
                    h.input.left_down = true;
                    h.input.left_pressed = true;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if let Ok(mut h) = rc.try_borrow_mut() {
                    let scale = window_scale_factor(HwndHandle(hwnd.0 as isize)).max(0.5) as f32;
                    h.input.mouse_x = (lparam.0 & 0xFFFF) as i16 as f32 / scale;
                    h.input.mouse_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / scale;
                    h.input.left_down = false;
                    h.input.left_released = true;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if let Ok(mut h) = rc.try_borrow_mut() {
                    let scale = window_scale_factor(HwndHandle(hwnd.0 as isize)).max(0.5) as f32;
                    h.input.mouse_x = (lparam.0 & 0xFFFF) as i16 as f32 / scale;
                    h.input.mouse_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32 / scale;
                    h.input.right_pressed = true;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.input.wheel_y += delta;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_MOUSEHWHEEL => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.input.wheel_x += delta;
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let alt = GetKeyState(VK_MENU.0 as i32) < 0;
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.input.keys.push(KeyEvent {
                        vk: wparam.0 as u32,
                        ctrl,
                        shift,
                        alt,
                    });
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_CHAR => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let unit = (wparam.0 & 0xFFFF) as u16;
                if let Ok(mut h) = rc.try_borrow_mut() {
                    if (0xD800..=0xDBFF).contains(&unit) {
                        h.pending_surrogate = Some(unit);
                    } else if (0xDC00..=0xDFFF).contains(&unit) {
                        if let Some(hi) = h.pending_surrogate.take() {
                            if let Some(Ok(ch)) = char::decode_utf16([hi, unit]).next() {
                                h.input.chars.push(ch);
                            }
                        }
                    } else {
                        h.pending_surrogate = None;
                        if let Some(ch) = char::from_u32(unit as u32) {
                            h.input.chars.push(ch);
                        }
                    }
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = suggested;
                if let Ok(st) = rc.try_borrow().map(|h| h.store.clone()) {
                    sync_notch_window(HwndHandle(hwnd.0 as isize), &st);
                }
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_APP_WAKE | WM_TIMER => {
            if let Some(rc) = host_from_hwnd::<A>(hwnd) {
                if let Ok(mut h) = rc.try_borrow_mut() {
                    h.tick_and_render();
                }
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_RENDER_ID);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
