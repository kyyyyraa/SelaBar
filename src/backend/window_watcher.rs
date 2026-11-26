use crate::backend::store::StoreManager;
use crate::events::{HwndHandle, UiEvent, UiSender};
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

type HWND = *mut c_void;
type HMONITOR = *mut c_void;
type HANDLE = *mut c_void;
type BOOL = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MONITORINFO {
    cb_size: u32,
    rc_monitor: RECT,
    rc_work: RECT,
    dw_flags: u32,
}

#[repr(C)]
struct PROCESSENTRY32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}

const MONITOR_DEFAULTTONEAREST: u32 = 2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
const INVALID_HANDLE_VALUE: isize = -1;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;

const TUCK_BROWSERS: [&str; 9] = [
    "chrome",
    "msedge",
    "brave",
    "firefox",
    "opera",
    "vivaldi",
    "arc",
    "zen",
    "zenbrowser",
];

const EXAM_APPS: [&str; 2] = ["safeexambrowser", "safeexambrowser.client"];

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> HWND;
    fn GetWindowRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    fn MonitorFromWindow(hWnd: HWND, dwFlags: u32) -> HMONITOR;
    fn GetMonitorInfoW(hMonitor: HMONITOR, lpmi: *mut MONITORINFO) -> BOOL;
    fn GetClassNameW(hWnd: HWND, lpClassName: *mut u16, nMaxCount: i32) -> i32;
    fn GetWindowTextW(hWnd: HWND, lpString: *mut u16, nMaxCount: i32) -> i32;
    fn GetClientRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    fn ClientToScreen(hWnd: HWND, lpPoint: *mut POINT) -> BOOL;
    fn GetWindowThreadProcessId(hWnd: HWND, lpdwProcessId: *mut u32) -> u32;
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> BOOL;
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: BOOL, dwProcessId: u32) -> HANDLE;
    fn CloseHandle(hObject: HANDLE) -> BOOL;
    fn QueryFullProcessImageNameW(
        hProcess: HANDLE,
        dwFlags: u32,
        lpExeName: *mut u16,
        lpdwSize: *mut u32,
    ) -> BOOL;
    fn CreateToolhelp32Snapshot(dwFlags: u32, th32ProcessID: u32) -> HANDLE;
    fn Process32FirstW(hSnapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> BOOL;
    fn Process32NextW(hSnapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> BOOL;
}

fn is_exam_app(exe_name: &str) -> bool {
    EXAM_APPS.contains(&exe_name)
}

fn exam_app_running() -> bool {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot as isize == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut found = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .sz_exe_file
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.sz_exe_file.len());
                let raw = String::from_utf16_lossy(&entry.sz_exe_file[..len]);
                let name = std::path::Path::new(&raw)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if is_exam_app(&name) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        found
    }
}

fn log_file() -> Option<std::path::PathBuf> {
    const MAX_LOG_BYTES: u64 = 256 * 1024;
    let appdata = std::env::var("APPDATA").ok()?;
    let log_path = std::path::Path::new(&appdata)
        .join("SelaBar")
        .join("notch-tuck.log");
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = std::fs::write(&log_path, b"");
        }
    }
    Some(log_path)
}

fn log_transition(reason: &str, foreground: &str, tucked: bool) {
    use std::io::Write;
    let state = if tucked { "TUCKED" } else { "EXPANDED" };
    let Some(path) = log_file() else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "{stamp} {state} reason={reason} foreground={foreground}");
    }
}

#[derive(Clone)]
pub struct WindowWatcher {
    is_tucked: Arc<AtomicBool>,
    is_fullscreen_hidden: Arc<AtomicBool>,
}

impl WindowWatcher {
    pub fn new() -> Self {
        Self {
            is_tucked: Arc::new(AtomicBool::new(false)),
            is_fullscreen_hidden: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_tucked(&self) -> bool {
        self.is_tucked.load(Ordering::Relaxed)
    }

    pub fn start_polling(&self, hwnd: HwndHandle, store: StoreManager, ui: UiSender) {
        let is_tucked_clone = Arc::clone(&self.is_tucked);
        let is_hidden_clone = Arc::clone(&self.is_fullscreen_hidden);
        let browsers: HashSet<&'static str> = TUCK_BROWSERS.iter().cloned().collect();

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(300));

                let fg = unsafe { inspect_foreground_detail(hwnd) };
                if fg.exe == "self" {
                    continue;
                }

                let settings = store.get_settings();
                let hide_fullscreen = settings.hide_on_fullscreen;
                let tuck_browsers = settings.tuck_for_browsers;
                let exam_open = exam_app_running() || is_exam_app(&fg.exe);

                let hide = hide_fullscreen && fg.fullscreen && !exam_open;
                if hide {
                    if !is_hidden_clone.swap(true, Ordering::Relaxed) {
                        unsafe { ShowWindow(hwnd.0 as HWND, SW_HIDE); }
                    }
                } else if is_hidden_clone.swap(false, Ordering::Relaxed) {
                    unsafe { ShowWindow(hwnd.0 as HWND, SW_SHOWNOACTIVATE); }
                    crate::backend::display::sync_zorder_with_myfinder(hwnd);
                }

                let is_browser = browsers.contains(fg.exe.as_str());
                let should_tuck = !hide && (exam_open || (tuck_browsers && is_browser));
                let was_tucked = is_tucked_clone.load(Ordering::Relaxed);

                if should_tuck != was_tucked {
                    is_tucked_clone.store(should_tuck, Ordering::Relaxed);
                    ui.send(UiEvent::NotchTucked(should_tuck));
                    let reason = if exam_open {
                        "SafeExamBrowser"
                    } else if is_browser {
                        "browser"
                    } else {
                        "other"
                    };
                    log_transition(reason, &fg.exe, should_tuck);
                }
            }
        });
    }
}

unsafe fn process_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    let h_proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h_proc.is_null() {
        return String::new();
    }
    let mut name_buf = [0u16; 1024];
    let mut size = 1024u32;
    let mut exe_name = String::new();
    if QueryFullProcessImageNameW(h_proc, 0, name_buf.as_mut_ptr(), &mut size) != 0 {
        let full_path = String::from_utf16_lossy(&name_buf[..size as usize]);
        if let Some(file_name) = std::path::Path::new(&full_path).file_stem() {
            exe_name = file_name.to_string_lossy().to_lowercase();
        }
    }
    CloseHandle(h_proc);
    exe_name
}

struct FgDetail {
    fullscreen: bool,
    exe: String,
}

unsafe fn inspect_foreground_detail(self_hwnd: HwndHandle) -> FgDetail {
    let hwnd = GetForegroundWindow();
    if hwnd.is_null() {
        return FgDetail { fullscreen: false, exe: String::new() };
    }
    if self_hwnd.is_valid() && hwnd == (self_hwnd.0 as HWND) {
        return FgDetail { fullscreen: false, exe: "self".to_string() };
    }

    let mut title_buf = [0u16; 256];
    let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256).max(0) as usize;
    let title = String::from_utf16_lossy(&title_buf[..title_len]);
    if title == "SelaBar" || title == "DeskNotch" {
        return FgDetail { fullscreen: false, exe: "self".to_string() };
    }

    let mut class_buf = [0u16; 256];
    let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256).max(0) as usize;
    let class_name = String::from_utf16_lossy(&class_buf[..class_len]);
    let is_shell = matches!(
        class_name.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "MyFinderApp"
    ) || class_name.starts_with("MyDock")
        || class_name.starts_with("MyFinder");

    let h_mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cb_size = std::mem::size_of::<MONITORINFO>() as u32;

    if GetMonitorInfoW(h_mon, &mut mi) == 0 {
        return FgDetail { fullscreen: false, exe: String::new() };
    }

    let mut rect: RECT = std::mem::zeroed();
    let mut is_fullscreen = false;

    if !is_shell && GetWindowRect(hwnd, &mut rect) != 0 {
        is_fullscreen = rect.left <= mi.rc_monitor.left + 2
            && rect.top <= mi.rc_monitor.top + 2
            && rect.right >= mi.rc_monitor.right - 2
            && rect.bottom >= mi.rc_monitor.bottom - 2;
    }

    if !is_fullscreen && !is_shell {
        let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut origin = POINT { x: 0, y: 0 };
        if GetClientRect(hwnd, &mut cr) != 0 && ClientToScreen(hwnd, &mut origin) != 0 {
            let cl = origin.x;
            let ct = origin.y;
            let cr_r = origin.x + (cr.right - cr.left);
            let cr_b = origin.y + (cr.bottom - cr.top);
            if cl <= mi.rc_monitor.left + 2
                && ct <= mi.rc_monitor.top + 2
                && cr_r >= mi.rc_monitor.right - 2
                && cr_b >= mi.rc_monitor.bottom - 2
            {
                is_fullscreen = true;
            }
        }
    }

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let exe_name = process_name(pid);
    if matches!(
        exe_name.as_str(),
        "dock_64" | "dock" | "dockmod" | "dockmod64" | "mydockfinder" | "myfinder"
    ) {
        is_fullscreen = false;
    }

    if exe_name.is_empty() && title.to_lowercase().contains("safe exam browser") {
        return FgDetail { fullscreen: is_fullscreen, exe: EXAM_APPS[0].to_string() };
    }

    FgDetail { fullscreen: is_fullscreen, exe: exe_name }
}
