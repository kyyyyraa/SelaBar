pub mod events;
pub mod state;
pub mod backend;
pub mod voice;
pub mod logic;
pub mod ui;

use crate::backend::approval_gate::ApprovalGate;
use crate::backend::click_through::ClickThroughManager;
use crate::backend::clipboard::ClipboardWatcher;
use crate::backend::display;
use crate::backend::drop_target;
use crate::backend::notif::NotificationWatcher;
use crate::backend::ocr::WinRtOcr;
use crate::backend::pipe_server::PipeServerManager;
use crate::backend::privacy;
use crate::backend::screenshot::ScreenshotWatcher;
use crate::backend::smtc::{self, SmtcWatcher};
use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::backend::win;
use crate::backend::window_watcher::WindowWatcher;
use crate::backend::{AskRunController, BackendServices};
use crate::events::{PrivacySnapshot, UiEvent, UiSender};
use crate::voice::{hotkey::HotkeyBindings, VoiceManager};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Ole::{IDropTarget, OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, ShowWindow, SW_SHOWNOACTIVATE};

struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn try_acquire_single_instance(mutex_name: &str) -> Option<SingleInstanceGuard> {
    let wide: Vec<u16> = mutex_name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Ok(handle) = CreateMutexW(None, false, PCWSTR(wide.as_ptr())) else {
            return None;
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(SingleInstanceGuard(handle))
    }
}

fn signal_existing_instance() {
    let class_w: Vec<u16> = "SelaBarOverlayWnd\0".encode_utf16().collect();
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR(class_w.as_ptr()), PCWSTR::null()) {
            if !hwnd.0.is_null() {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = PostMessageW(Some(hwnd), crate::events::WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|a| a.eq_ignore_ascii_case("--uninstall") || a.eq_ignore_ascii_case("/uninstall"))
    {
        let quiet = args
            .iter()
            .any(|a| a.eq_ignore_ascii_case("--quiet") || a.eq_ignore_ascii_case("/S"));
        let _ = crate::backend::installer::run_uninstaller(quiet);
        return;
    }

    let Some(_single_instance_guard) =
        try_acquire_single_instance("Local\\SelaBar_SingleInstance_Mutex_v1")
    else {
        signal_existing_instance();
        crate::backend::logfile::append(
            "[startup] duplicate instance detected; signaling existing SelaBar and exiting",
        );
        return;
    };

    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        crate::backend::logfile::append(&format!("[panic] at {}: {}", loc, msg));
    }));

    unsafe {
        let _ = OleInitialize(None);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    let (ui, rx) = UiSender::pair();
    let store = StoreManager::new();
    let click_through = ClickThroughManager::new();
    let pipe = PipeServerManager::new();
    let window_watcher = WindowWatcher::new();
    let approval = ApprovalGate::new();
    let ask_ctrl = Arc::new(AskRunController::new());
    let ocr = WinRtOcr::new().ok().map(Arc::new);
    let voice = Arc::new(VoiceManager::new());

    let svc = BackendServices {
        store: store.clone(),
        click_through: click_through.clone(),
        pipe: pipe.clone(),
        window_watcher: window_watcher.clone(),
        approval: approval.clone(),
        ask_ctrl,
        ocr,
        voice: Arc::clone(&voice),
        rt: rt.handle().clone(),
        ui: ui.clone(),
    };

    // Start background OS watchers and hotkey daemon
    let initial_hotkeys = HotkeyBindings::from_settings(&svc.store.get_settings());
    svc.voice.hotkeys.start_listener(svc.ui.clone(), initial_hotkeys);
    svc.pipe.start(&svc.rt, svc.ui.clone());
    NotificationWatcher::new().start(svc.ui.clone());
    ScreenshotWatcher::new().start(svc.ui.clone());
    ClipboardWatcher::new().start(svc.ui.clone());
    SmtcWatcher::new().start(svc.ui.clone());
    std::thread::spawn(system::ensure_app_registration);

    // Initial async hydration (active SMTC track)
    {
        let u = svc.ui.clone();
        std::thread::spawn(move || {
            let info = smtc::get_current_session_info();
            u.send(UiEvent::SmtcNowPlaying(info));
        });
    }

    // 2s live usage + 15s battery status + 4s connectivity poller
    {
        let u = svc.ui.clone();
        std::thread::spawn(move || {
            let mut ticks = 0u64;
            let mut last_connected: Option<bool> = None;
            let mut offline_hits = 0u32;
            let mut online_hits = 0u32;
            loop {
                u.send(UiEvent::LiveUsageUpdated(system::get_live_usage()));
                if ticks % 7 == 0 {
                    u.send(UiEvent::BatteryUpdated(win::battery_state()));
                }
                if ticks % 2 == 0 {
                    if let Some(connected) = system::internet_connected() {
                        // Debounce: two consecutive readings before reporting a
                        // transition, so a roaming blip does not flash a popup.
                        if connected {
                            online_hits += 1;
                            offline_hits = 0;
                        } else {
                            offline_hits += 1;
                            online_hits = 0;
                        }
                        let stable = if connected { online_hits >= 2 } else { offline_hits >= 2 };
                        if stable && last_connected != Some(connected) {
                            last_connected = Some(connected);
                            u.send(UiEvent::NetworkChanged { connected });
                        }
                    }
                }
                ticks = ticks.wrapping_add(1);
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    // 800ms Windows real-time privacy (CoreAudio + MediaFoundation + ConsentStore) & VoIP call poller
    {
        let u = svc.ui.clone();
        std::thread::spawn(move || {
            let mut last = PrivacySnapshot::default();
            let mut last_call_key = String::new();
            let mut last_call_at = std::time::Instant::now() - Duration::from_secs(60);
            let mut call_tracker = system::VoipCallTracker::new();
            loop {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(snap) = privacy::poll_changed(&mut last) {
                        u.send(UiEvent::PrivacyChanged(snap));
                    }
                    if let Some(call_payload) = system::poll_voip_call_window() {
                        let key = format!("{}|{}", call_payload.app, call_payload.title);
                        call_tracker.observe(Some(&key));
                        if key != last_call_key
                            || last_call_at.elapsed() >= Duration::from_secs(12)
                        {
                            last_call_key = key;
                            last_call_at = std::time::Instant::now();
                            u.send(UiEvent::IncomingCallDetected(call_payload));
                        }
                    } else {
                        // The ringing title disappears on answer/decline/hangup;
                        // N consecutive empty polls mean the call is over even if
                        // the mic ConsentStore entry stays stale.
                        if call_tracker.observe(None) {
                            u.send(UiEvent::IncomingCallDismissed);
                        }
                        last_call_key.clear();
                    }
                }));
                std::thread::sleep(Duration::from_millis(800));
            }
        });
    }

    // Startup + 6-hourly background GitHub release update check
    {
        let u = svc.ui.clone();
        svc.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            loop {
                let outcome = crate::backend::updater::check_latest_release().await;
                u.send(UiEvent::UpdateCheckDone {
                    manual: false,
                    outcome,
                });
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }

    let app = ui::NotchApp::new(svc.clone());
    let drop_target_holder: Rc<RefCell<Option<IDropTarget>>> = Rc::new(RefCell::new(None));
    let dt_ref = Rc::clone(&drop_target_holder);
    let ui_ready = svc.ui.clone();
    let ct_ready = svc.click_through.clone();
    let ww_ready = svc.window_watcher.clone();
    let st_ready = svc.store.clone();
    let hk = svc.voice.hotkeys.clone();

    let res = ui::window::run_window(
        app,
        rx,
        svc.ui.clone(),
        svc.click_through.clone(),
        hk,
        svc.store.clone(),
        move |hwnd| {
            *dt_ref.borrow_mut() =
                drop_target::register_drop_target(hwnd, ui_ready.clone(), ct_ready.clone());
            ct_ready.start_polling(hwnd, ui_ready.clone());
            ww_ready.start_polling(hwnd, st_ready, ui_ready);

            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                if !hwnd.is_valid() {
                    break;
                }
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    display::sync_zorder_with_myfinder(hwnd);
                }));
            });
        },
    );

    if let Err(e) = res {
        crate::backend::logfile::append(&format!("[main] window exited with error: {}", e));
    }

    svc.pipe.deny_all();
    svc.approval.deny_all();
    drop(drop_target_holder);
    unsafe {
        OleUninitialize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_mutex_blocks_second_acquire_while_held() {
        let name = format!("Local\\SelaBar_TestMutex_{}", std::process::id());
        let first = try_acquire_single_instance(&name);
        assert!(first.is_some());
        let second = try_acquire_single_instance(&name);
        assert!(second.is_none());
        drop(first);
        let third = try_acquire_single_instance(&name);
        assert!(third.is_some());
    }
}
