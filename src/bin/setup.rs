#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use selabar_lib::backend::installer::{self, InstallOptions};
use std::path::PathBuf;
use std::process::Command;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkColor, SetBkMode,
    SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_DONTCARE, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, HFONT, OUT_DEFAULT_PRECIS,
    TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, LoadCursorW,
    MessageBoxW, PostQuitMessage, RegisterClassW, SendMessageW, SetWindowLongPtrW, SetWindowTextW,
    ShowWindow, TranslateMessage, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, CW_USEDEFAULT, ES_AUTOHSCROLL, GWLP_USERDATA, IDC_ARROW, MB_ICONERROR,
    MB_ICONINFORMATION, MB_OK, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION,
    WS_CHILD, WS_EX_CLIENTEDGE, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

const ID_EDIT_DIR: isize = 101;
const ID_BTN_BROWSE: isize = 102;
const ID_CHK_STARTMENU: isize = 103;
const ID_CHK_DESKTOP: isize = 104;
const ID_CHK_STARTUP: isize = 105;
const ID_CHK_LAUNCH: isize = 106;
const ID_LBL_STATUS: isize = 107;
const ID_BTN_INSTALL: isize = 108;
const ID_BTN_CANCEL: isize = 109;
const BST_CHECKED: usize = 1;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn load_payload() -> Result<Vec<u8>, String> {
    #[cfg(feature = "setup-payload")]
    {
        use flate2::read::GzDecoder;
        use std::io::Read;

        const GZ_BYTES: &[u8] = include_bytes!("../../target/release/SelaBar.exe.gz");
        let mut decoder = GzDecoder::new(GZ_BYTES);
        let mut out = Vec::with_capacity(15 * 1024 * 1024);
        decoder
            .read_to_end(&mut out)
            .map_err(|e| format!("Gagal mengekstrak payload SelaBar.exe: {}", e))?;
        return Ok(out);
    }

    #[cfg(not(feature = "setup-payload"))]
    {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("SelaBar.exe")));
        if let Some(p) = sibling {
            if p.is_file() {
                return std::fs::read(&p)
                    .map_err(|e| format!("Gagal membaca '{}': {}", p.display(), e));
            }
        }
        Err(
            "Setup ini dibuat tanpa payload tersemat (build dengan --features setup-payload atau taruh SelaBar.exe di sebelah Setup)."
                .to_string(),
        )
    }
}

struct SetupState {
    edit_dir: HWND,
    btn_browse: HWND,
    chk_start_menu: HWND,
    chk_desktop: HWND,
    chk_startup: HWND,
    chk_launch: HWND,
    lbl_status: HWND,
    btn_install: HWND,
    btn_cancel: HWND,
    bg_brush: HBRUSH,
    font_title: HFONT,
    font_body: HFONT,
    font_small: HFONT,
}

impl Drop for SetupState {
    fn drop(&mut self) {
        unsafe {
            if !self.bg_brush.0.is_null() {
                let _ = DeleteObject(self.bg_brush.into());
            }
            for f in [self.font_title, self.font_body, self.font_small] {
                if !f.0.is_null() {
                    let _ = DeleteObject(f.into());
                }
            }
        }
    }
}

fn make_font(height_px: i32, weight: u32) -> HFONT {
    let face = to_wide("Segoe UI");
    unsafe {
        CreateFontW(
            -height_px,
            0,
            0,
            0,
            weight as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn get_hwnd_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len as usize) + 2];
        let copied = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied as usize])
    }
}

fn is_checked(hwnd: HWND) -> bool {
    unsafe { SendMessageW(hwnd, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 as usize == BST_CHECKED }
}

fn set_font(hwnd: HWND, font: HFONT) {
    unsafe {
        let _ = SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
}

fn set_text(hwnd: HWND, txt: &str) {
    let w = to_wide(txt);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
    }
}

fn pick_folder(owner: HWND, initial: &str) -> Option<String> {
    unsafe {
        let init_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let res = (|| -> Option<String> {
            let dlg: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let opts = dlg.GetOptions().ok()?;
            dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
                .ok()?;
            let title = to_wide("Pilih Folder Instalasi SelaBar");
            let _ = dlg.SetTitle(PCWSTR(title.as_ptr()));
            if !initial.trim().is_empty() {
                let init_w = to_wide(initial.trim());
                if let Ok(folder_item) =
                    SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(init_w.as_ptr()), None)
                {
                    let _ = dlg.SetFolder(&folder_item);
                }
            }
            if dlg.Show(Some(owner)).is_err() {
                return None;
            }
            let item = dlg.GetResult().ok()?;
            let pwstr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = pwstr.to_string().ok();
            CoTaskMemFree(Some(pwstr.0.cast()));
            path
        })();
        if init_ok {
            CoUninitialize();
        }
        res
    }
}

fn run_install_action(hwnd: HWND, st: &SetupState) {
    let mut raw_dir = get_hwnd_text(st.edit_dir).trim().to_string();
    if raw_dir.is_empty() {
        raw_dir = installer::default_install_dir().to_string_lossy().into_owned();
        set_text(st.edit_dir, &raw_dir);
    }

    let opts = InstallOptions {
        start_menu_shortcut: is_checked(st.chk_start_menu),
        desktop_shortcut: is_checked(st.chk_desktop),
        start_on_boot: is_checked(st.chk_startup),
        launch_after: is_checked(st.chk_launch),
    };

    unsafe {
        let _ = EnableWindow(st.btn_install, false);
        let _ = EnableWindow(st.btn_cancel, false);
        let _ = EnableWindow(st.btn_browse, false);
    }
    set_text(st.lbl_status, "Mengekstrak & menginstall SelaBar.exe...");

    let result = load_payload().and_then(|bytes| {
        let mut target_dir = PathBuf::from(&raw_dir);
        let needs_subdir = target_dir
            .file_name()
            .and_then(|s| s.to_str())
            .is_none_or(|n| !n.eq_ignore_ascii_case("selabar"))
            && raw_dir != installer::default_install_dir().to_string_lossy();
        if needs_subdir && !target_dir.join("SelaBar.exe").exists() {
            target_dir = target_dir.join("SelaBar");
        }
        installer::install_payload(&bytes, &target_dir, &opts)
    });

    match result {
        Ok(dest_exe) => {
            if opts.launch_after {
                let mut cmd = Command::new(&dest_exe);
                if let Some(dir) = dest_exe.parent() {
                    cmd.current_dir(dir);
                }
                let _ = cmd.spawn();
            }
            set_text(st.lbl_status, "Instalasi selesai!");
            let msg = to_wide(&format!(
                "SelaBar v{} berhasil diinstall ke:\n{}\n\nAnda dapat meng-uninstall kapan saja lewat Windows Settings -> Apps -> SelaBar.",
                env!("CARGO_PKG_VERSION"),
                dest_exe.display()
            ));
            let title = to_wide("Instalasi SelaBar Selesai");
            unsafe {
                MessageBoxW(
                    Some(hwnd),
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONINFORMATION,
                );
                let _ = DestroyWindow(hwnd);
            }
        }
        Err(e) => {
            unsafe {
                let _ = EnableWindow(st.btn_install, true);
                let _ = EnableWindow(st.btn_cancel, true);
                let _ = EnableWindow(st.btn_browse, true);
            }
            set_text(st.lbl_status, &format!("Gagal: {}", e));
            let msg = to_wide(&format!("Gagal menginstall SelaBar:\n{}", e));
            let title = to_wide("Error Instalasi SelaBar");
            unsafe {
                MessageBoxW(
                    Some(hwnd),
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let dpi = GetDpiForWindow(hwnd).max(96) as f32;
            let sc = |px: i32| -> i32 { ((px as f32) * (dpi / 96.0)).round() as i32 };
            let hinst = HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0);

            let cls_static = to_wide("STATIC");
            let cls_edit = to_wide("EDIT");
            let cls_button = to_wide("BUTTON");

            let font_title = make_font(sc(22), FW_BOLD.0);
            let font_body = make_font(sc(15), FW_NORMAL.0);
            let font_small = make_font(sc(13), FW_SEMIBOLD.0);
            let bg_brush = CreateSolidBrush(COLORREF(0x00F8F9FA));

            let mk = |cls: &[u16],
                      ex: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
                      style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
                      x: i32,
                      y: i32,
                      w: i32,
                      h: i32,
                      id: isize,
                      text: &str|
             -> HWND {
                let tw = to_wide(text);
                CreateWindowExW(
                    ex,
                    PCWSTR(cls.as_ptr()),
                    PCWSTR(tw.as_ptr()),
                    style,
                    sc(x),
                    sc(y),
                    sc(w),
                    sc(h),
                    Some(hwnd),
                    Some(windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)),
                    Some(hinst),
                    None,
                )
                .unwrap_or_default()
            };

            let title_lbl = mk(
                &cls_static,
                Default::default(),
                WS_CHILD | WS_VISIBLE,
                24,
                20,
                440,
                28,
                0,
                concat!("Install SelaBar v", env!("CARGO_PKG_VERSION")),
            );
            set_font(title_lbl, font_title);

            let sub_lbl = mk(
                &cls_static,
                Default::default(),
                WS_CHILD | WS_VISIBLE,
                24,
                50,
                440,
                20,
                0,
                "Dynamic Notch & Desktop AI Companion untuk Windows",
            );
            set_font(sub_lbl, font_body);

            let dir_lbl = mk(
                &cls_static,
                Default::default(),
                WS_CHILD | WS_VISIBLE,
                24,
                84,
                200,
                20,
                0,
                "Lokasi Instalasi:",
            );
            set_font(dir_lbl, font_small);

            let def_dir = installer::default_install_dir().to_string_lossy().into_owned();
            let edit_dir = mk(
                &cls_edit,
                WS_EX_CLIENTEDGE,
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_TABSTOP
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                24,
                108,
                348,
                26,
                ID_EDIT_DIR,
                &def_dir,
            );
            set_font(edit_dir, font_body);

            let btn_browse = mk(
                &cls_button,
                Default::default(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
                380,
                107,
                84,
                28,
                ID_BTN_BROWSE,
                "Ubah...",
            );
            set_font(btn_browse, font_body);

            let make_chk = |y: i32, id: isize, txt: &str| -> HWND {
                let h = mk(
                    &cls_button,
                    Default::default(),
                    WS_CHILD
                        | WS_VISIBLE
                        | WS_TABSTOP
                        | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                            BS_AUTOCHECKBOX as u32,
                        ),
                    24,
                    y,
                    440,
                    22,
                    id,
                    txt,
                );
                set_font(h, font_body);
                let _ = SendMessageW(h, BM_SETCHECK, Some(WPARAM(BST_CHECKED)), Some(LPARAM(0)));
                h
            };

            let chk_start_menu = make_chk(148, ID_CHK_STARTMENU, "Buat shortcut di Start Menu");
            let chk_desktop = make_chk(174, ID_CHK_DESKTOP, "Buat shortcut di Desktop");
            let chk_startup = make_chk(200, ID_CHK_STARTUP, "Jalankan otomatis saat Windows menyala");
            let chk_launch = make_chk(226, ID_CHK_LAUNCH, "Jalankan SelaBar setelah selesai");

            let lbl_note = mk(
                &cls_static,
                Default::default(),
                WS_CHILD | WS_VISIBLE,
                24,
                258,
                440,
                18,
                0,
                "Terdaftar di Windows Settings -> Apps untuk uninstall bersih kapan saja.",
            );
            set_font(lbl_note, font_small);

            let lbl_status = mk(
                &cls_static,
                Default::default(),
                WS_CHILD | WS_VISIBLE,
                24,
                284,
                220,
                22,
                ID_LBL_STATUS,
                "Siap menginstall.",
            );
            set_font(lbl_status, font_body);

            let btn_install = mk(
                &cls_button,
                Default::default(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                        BS_DEFPUSHBUTTON as u32,
                    ),
                254,
                282,
                122,
                32,
                ID_BTN_INSTALL,
                "Install SelaBar",
            );
            set_font(btn_install, font_small);

            let btn_cancel = mk(
                &cls_button,
                Default::default(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(BS_PUSHBUTTON as u32),
                384,
                282,
                80,
                32,
                ID_BTN_CANCEL,
                "Batal",
            );
            set_font(btn_cancel, font_body);

            let boxed = Box::new(SetupState {
                edit_dir,
                btn_browse,
                chk_start_menu,
                chk_desktop,
                chk_startup,
                chk_launch,
                lbl_status,
                btn_install,
                btn_cancel,
                bg_brush,
                font_title,
                font_body,
                font_small,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(boxed) as isize);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const SetupState;
            let hdc = HDC(wparam.0 as *mut _);
            SetBkMode(hdc, TRANSPARENT);
            SetBkColor(hdc, COLORREF(0x00F8F9FA));
            SetTextColor(hdc, COLORREF(0x0018181B));
            if !ptr.is_null() && !(*ptr).bg_brush.0.is_null() {
                return LRESULT((*ptr).bg_brush.0 as isize);
            }
            LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as isize;
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const SetupState;
            if !ptr.is_null() {
                let st = &*ptr;
                match id {
                    ID_BTN_BROWSE => {
                        let cur = get_hwnd_text(st.edit_dir);
                        if let Some(chosen) = pick_folder(hwnd, &cur) {
                            let chosen_path = PathBuf::from(&chosen);
                            let final_dir = if chosen_path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .is_some_and(|n| n.eq_ignore_ascii_case("selabar"))
                            {
                                chosen_path
                            } else {
                                chosen_path.join("SelaBar")
                            };
                            set_text(st.edit_dir, &final_dir.to_string_lossy());
                        }
                    }
                    ID_BTN_INSTALL => {
                        run_install_action(hwnd, st);
                    }
                    ID_BTN_CANCEL => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut SetupState;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn parse_cli_silent_dir() -> (bool, Option<PathBuf>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let silent = args
        .iter()
        .any(|a| a.eq_ignore_ascii_case("--silent") || a.eq_ignore_ascii_case("/S"));
    let mut custom_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(val) = a.strip_prefix("--dir=") {
            if !val.trim().is_empty() {
                custom_dir = Some(PathBuf::from(val.trim()));
            }
        } else if a.eq_ignore_ascii_case("--dir") {
            if let Some(next) = args.get(i + 1) {
                if !next.trim().is_empty() {
                    custom_dir = Some(PathBuf::from(next.trim()));
                }
                i += 1;
            }
        }
        i += 1;
    }
    (silent, custom_dir)
}

fn main() {
    let (silent, custom_dir) = parse_cli_silent_dir();
    if silent {
        let target_dir = custom_dir.unwrap_or_else(installer::default_install_dir);
        let opts = InstallOptions::default();
        match load_payload().and_then(|b| installer::install_payload(&b, &target_dir, &opts)) {
            Ok(dest_exe) => {
                if opts.launch_after {
                    let mut cmd = Command::new(&dest_exe);
                    if let Some(dir) = dest_exe.parent() {
                        cmd.current_dir(dir);
                    }
                    let _ = cmd.spawn();
                }
                std::process::exit(0);
            }
            Err(_) => std::process::exit(1),
        }
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0);
        let cls_name = to_wide("SelaBarSetupWnd");
        let bg = CreateSolidBrush(COLORREF(0x00F8F9FA));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: bg,
            lpszClassName: PCWSTR(cls_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let title = to_wide(concat!("SelaBar v", env!("CARGO_PKG_VERSION"), " Setup"));
        let screen_w = GetSystemMetrics(SM_CXSCREEN).max(800);
        let screen_h = GetSystemMetrics(SM_CYSCREEN).max(600);
        let win_w = 500;
        let win_h = 362;
        let pos_x = (screen_w - win_w) / 2;
        let pos_y = (screen_h - win_h) / 2;

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(cls_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            if pos_x > 0 { pos_x } else { CW_USEDEFAULT },
            if pos_y > 0 { pos_y } else { CW_USEDEFAULT },
            win_w,
            win_h,
            None,
            None,
            Some(hinst),
            None,
        )
        .unwrap_or_default();

        if !hwnd.0.is_null() {
            // Re-scale window bounds accurately on high-DPI monitors
            let dpi = GetDpiForWindow(hwnd).max(96) as f32;
            if dpi > 96.0 {
                let sw = (500.0 * dpi / 96.0).round() as i32;
                let sh = (362.0 * dpi / 96.0).round() as i32;
                let sx = ((screen_w - sw) / 2).max(0);
                let sy = ((screen_h - sh) / 2).max(0);
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    hwnd,
                    None,
                    sx,
                    sy,
                    sw,
                    sh,
                    Default::default(),
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = DeleteObject(bg.into());
    }
}
