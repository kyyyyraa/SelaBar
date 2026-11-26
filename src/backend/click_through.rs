use crate::backend::display::window_scale_factor;
use crate::events::{HwndHandle, UiEvent, UiSender};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, GWL_EXSTYLE,
    WS_EX_TRANSPARENT,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone)]
pub struct ClickThroughManager {
    bounds: Arc<Mutex<Vec<Rect>>>,
    last_cursor_inside: Arc<AtomicBool>,
    drag_active: Arc<AtomicBool>,
}

impl ClickThroughManager {
    pub fn new() -> Self {
        Self {
            bounds: Arc::new(Mutex::new(Vec::new())),
            last_cursor_inside: Arc::new(AtomicBool::new(false)),
            drag_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn update_bounds(&self, rects: Vec<Rect>) {
        let mut guard = self.bounds.lock().unwrap_or_else(|e| e.into_inner());
        *guard = rects;
    }

    pub fn set_drag_active(&self, hwnd: HwndHandle, active: bool) {
        self.drag_active.store(active, Ordering::SeqCst);
        if active {
            self.last_cursor_inside.store(true, Ordering::SeqCst);
            set_ignore_cursor_events(hwnd, false);
        }
    }

    pub fn start_polling(&self, hwnd: HwndHandle, ui: UiSender) {
        let bounds_clone = Arc::clone(&self.bounds);
        let last_inside_clone = Arc::clone(&self.last_cursor_inside);
        let drag_clone = Arc::clone(&self.drag_active);

        std::thread::spawn(move || {
            let mut last_rel_x = -999;
            let mut last_rel_y = -999;
            let mut first_poll = true;
            let mut release_grace = 0u32;

            loop {
                std::thread::sleep(Duration::from_millis(80));
                if !hwnd.is_valid() {
                    continue;
                }

                let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let ok_rect = unsafe { GetWindowRect(hwnd.0 as HWND, &mut wr) != 0 };
                if !ok_rect {
                    continue;
                }

                let scale = window_scale_factor(hwnd).max(0.5);
                let mut pt = POINT { x: 0, y: 0 };
                let cursor_ok = unsafe { GetCursorPos(&mut pt) != 0 };
                if !cursor_ok {
                    continue;
                }

                let cur_x = pt.x as f64;
                let cur_y = pt.y as f64;
                let win_x = wr.left as f64;
                let win_y = wr.top as f64;

                let rel_x = ((cur_x - win_x) / scale).round() as i32;
                let rel_y = ((cur_y - win_y) / scale).round() as i32;

                let is_inside = if drag_clone.load(Ordering::Relaxed) {
                    true
                } else {
                    let guard = bounds_clone.lock().unwrap_or_else(|e| e.into_inner());
                    guard.iter().any(|r| {
                        let rx = r.x * scale;
                        let ry = r.y * scale;
                        let rw = r.width * scale;
                        let rh = r.height * scale;
                        cur_x >= win_x + rx
                            && cur_x <= win_x + rx + rw
                            && cur_y >= win_y + ry
                            && cur_y <= win_y + ry + rh
                    })
                };

                // While the left button is held (and shortly after release)
                // keep the window interactive: a card action that re-renders
                // mid-click must not flip click-through before WM_LBUTTONUP.
                let lmb_down = unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 };
                if lmb_down {
                    release_grace = 4;
                } else if release_grace > 0 {
                    release_grace -= 1;
                }
                let effective_inside = is_inside || lmb_down || release_grace > 0;

                let was_inside = last_inside_clone.swap(effective_inside, Ordering::SeqCst);
                if first_poll || effective_inside != was_inside {
                    first_poll = false;
                    ui.send(UiEvent::SetClickThrough(!effective_inside));
                }

                if rel_x != last_rel_x || rel_y != last_rel_y {
                    last_rel_x = rel_x;
                    last_rel_y = rel_y;
                    ui.send(UiEvent::CursorMoved { x: rel_x, y: rel_y });
                }
            }
        });
    }
}

pub fn set_ignore_cursor_events(hwnd: HwndHandle, ignore: bool) {
    if !hwnd.is_valid() {
        return;
    }
    unsafe {
        let raw = hwnd.0 as HWND;
        let ex = GetWindowLongPtrW(raw, GWL_EXSTYLE);
        let next = if ignore {
            ex | (WS_EX_TRANSPARENT as isize)
        } else {
            ex & !(WS_EX_TRANSPARENT as isize)
        };
        if next != ex {
            SetWindowLongPtrW(raw, GWL_EXSTYLE, next);
        }
    }
}
