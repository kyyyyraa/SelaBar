//! Shared Windows installer, uninstaller, Start Menu / Desktop shortcut, and
//! `Add or Remove Programs` registry management for SelaBar.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_SZ,
};
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, IShellLinkW, SHGetKnownFolderPath, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, MessageBoxW, PostMessageW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK,
    MB_SETFOREGROUND, MB_YESNO, WM_CLOSE,
};

pub const UNINSTALL_REG_PATH: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\SelaBar";
pub const APP_PATHS_REG_PATH: &str =
    r"Software\Microsoft\Windows\CurrentVersion\App Paths\SelaBar.exe";
const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub start_menu_shortcut: bool,
    pub desktop_shortcut: bool,
    pub start_on_boot: bool,
    pub launch_after: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            start_menu_shortcut: true,
            desktop_shortcut: true,
            start_on_boot: true,
            launch_after: true,
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn strip_verbatim_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{}", rest));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p.to_path_buf()
}

pub fn default_install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(|p| PathBuf::from(p).join("AppData").join("Local"))
        })
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join("Programs").join("SelaBar")
}

pub fn app_data_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    appdata.join("SelaBar")
}

pub fn start_menu_shortcut_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("SelaBar.lnk"),
    )
}

pub fn desktop_dir() -> Option<PathBuf> {
    unsafe {
        if let Ok(pwstr) = SHGetKnownFolderPath(&FOLDERID_Desktop, Default::default(), None) {
            let s = pwstr.to_string().unwrap_or_default();
            CoTaskMemFree(Some(pwstr.0.cast()));
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .map(|p| PathBuf::from(p).join("Desktop"))
}

pub fn desktop_shortcut_path() -> Option<PathBuf> {
    desktop_dir().map(|d| d.join("SelaBar.lnk"))
}

/// Guard ensuring `--uninstall` never recursively deletes shared folders like Downloads,
/// Desktop, drive roots, or a Rust workspace checkout.
pub fn is_safe_uninstall_dir(dir: &Path) -> bool {
    let is_named_selabar = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("selabar"));
    if !is_named_selabar {
        return false;
    }
    if dir.join("Cargo.toml").exists() {
        return false;
    }
    if dir.components().any(|c| c.as_os_str() == "target") {
        return false;
    }
    true
}

fn reg_set_sz(hkey: HKEY, name: Option<&str>, value: &str) {
    let name_w = name.map(to_wide);
    let val_w = to_wide(value);
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(val_w.as_ptr().cast(), val_w.len() * 2) };
    let name_ptr = name_w
        .as_ref()
        .map(|v| PCWSTR(v.as_ptr()))
        .unwrap_or(PCWSTR::null());
    unsafe {
        let _ = RegSetValueExW(hkey, name_ptr, Some(0), REG_SZ, Some(bytes));
    }
}

fn reg_set_dword(hkey: HKEY, name: &str, value: u32) {
    let name_w = to_wide(name);
    let bytes = value.to_le_bytes();
    unsafe {
        let _ = RegSetValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            Some(0),
            REG_DWORD,
            Some(&bytes),
        );
    }
}

/// Registers SelaBar in `HKCU\...\Uninstall\SelaBar` (Settings -> Apps / Control Panel)
/// and `HKCU\...\App Paths\SelaBar.exe`.
pub fn register_windows_uninstall(exe_path: &Path) -> Result<(), String> {
    let exe = strip_verbatim_path(exe_path);
    let exe_str = exe.to_string_lossy().into_owned();
    let dir_str = exe
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size_kb = fs::metadata(&exe)
        .map(|m| (m.len() / 1024).max(1) as u32)
        .unwrap_or(14_000);

    unsafe {
        let mut hkey = HKEY::default();
        let sub = to_wide(UNINSTALL_REG_PATH);
        let st = RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), &mut hkey);
        if st.0 != 0 {
            return Err(format!("gagal menulis registry Uninstall (kode {})", st.0));
        }
        reg_set_sz(hkey, Some("DisplayName"), "SelaBar");
        reg_set_sz(hkey, Some("DisplayVersion"), env!("CARGO_PKG_VERSION"));
        reg_set_sz(hkey, Some("Publisher"), "SelaBar");
        reg_set_sz(hkey, Some("DisplayIcon"), &format!("{},0", exe_str));
        if !dir_str.is_empty() {
            reg_set_sz(hkey, Some("InstallLocation"), &dir_str);
        }
        reg_set_sz(
            hkey,
            Some("UninstallString"),
            &format!("\"{}\" --uninstall", exe_str),
        );
        reg_set_sz(
            hkey,
            Some("QuietUninstallString"),
            &format!("\"{}\" --uninstall --quiet", exe_str),
        );
        reg_set_sz(hkey, Some("URLInfoAbout"), "https://github.com/kyyyyraa/SelaBar");
        reg_set_dword(hkey, "EstimatedSize", size_kb);
        reg_set_dword(hkey, "NoModify", 1);
        reg_set_dword(hkey, "NoRepair", 1);
        let _ = RegCloseKey(hkey);

        let mut ap_key = HKEY::default();
        let ap_sub = to_wide(APP_PATHS_REG_PATH);
        if RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(ap_sub.as_ptr()), &mut ap_key).0 == 0 {
            reg_set_sz(ap_key, None, &exe_str);
            if !dir_str.is_empty() {
                reg_set_sz(ap_key, Some("Path"), &dir_str);
            }
            let _ = RegCloseKey(ap_key);
        }
    }

    Ok(())
}

/// Removes SelaBar entries from Windows Uninstall, App Paths, and Run startup keys.
pub fn unregister_windows_entries() {
    unsafe {
        let uninst = to_wide(UNINSTALL_REG_PATH);
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(uninst.as_ptr()));
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(uninst.as_ptr()));

        let ap = to_wide(APP_PATHS_REG_PATH);
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(ap.as_ptr()));
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(ap.as_ptr()));

        let run_sub = to_wide(RUN_KEY_PATH);
        let mut run_hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(run_sub.as_ptr()),
            Some(0),
            KEY_SET_VALUE,
            &mut run_hkey,
        )
        .0 == 0
        {
            let v1 = to_wide("SelaBar");
            let v2 = to_wide("DeskNotch");
            let _ = RegDeleteValueW(run_hkey, PCWSTR(v1.as_ptr()));
            let _ = RegDeleteValueW(run_hkey, PCWSTR(v2.as_ptr()));
            let _ = RegCloseKey(run_hkey);
        }
    }
}

pub fn create_shortcut(
    lnk_path: &Path,
    target_exe: &Path,
    args: Option<&str>,
    desc: &str,
) -> Result<(), String> {
    if let Some(parent) = lnk_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("gagal buat folder shortcut: {}", e))?;
    }
    let exe = strip_verbatim_path(target_exe);
    let exe_str = exe.to_string_lossy().into_owned();
    let dir_str = exe
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    unsafe {
        let init_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let res = (|| -> Result<(), String> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("ShellLink COM error: {:?}", e))?;
            let exe_w = to_wide(&exe_str);
            let dir_w = to_wide(&dir_str);
            let desc_w = to_wide(desc);
            link.SetPath(PCWSTR(exe_w.as_ptr()))
                .map_err(|e| format!("SetPath error: {:?}", e))?;
            if let Some(a) = args {
                let args_w = to_wide(a);
                let _ = link.SetArguments(PCWSTR(args_w.as_ptr()));
            }
            if !dir_str.is_empty() {
                let _ = link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()));
            }
            let _ = link.SetDescription(PCWSTR(desc_w.as_ptr()));
            let _ = link.SetIconLocation(PCWSTR(exe_w.as_ptr()), 0);
            let persist: IPersistFile = link
                .cast()
                .map_err(|e| format!("IPersistFile cast error: {:?}", e))?;
            let lnk_w = to_wide(&lnk_path.to_string_lossy());
            persist
                .Save(PCWSTR(lnk_w.as_ptr()), true)
                .map_err(|e| format!("simpan shortcut gagal: {:?}", e))?;
            Ok(())
        })();
        if init_ok {
            CoUninitialize();
        }
        res
    }
}

pub fn remove_shortcuts() {
    if let Some(sm_lnk) = start_menu_shortcut_path() {
        let _ = fs::remove_file(&sm_lnk);
        if let Some(parent) = sm_lnk.parent() {
            let folder = parent.join("SelaBar");
            if folder.exists() {
                let _ = fs::remove_dir_all(&folder);
            }
        }
    }
    if let Some(desk_lnk) = desktop_shortcut_path() {
        let _ = fs::remove_file(desk_lnk);
    }
    if let Some(up) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        let _ = fs::remove_file(PathBuf::from(up).join("Desktop").join("SelaBar.lnk"));
    }
}

pub fn set_run_at_startup(enable: bool, exe_path: Option<&Path>) -> Result<(), String> {
    unsafe {
        let mut hkey = HKEY::default();
        let sub = to_wide(RUN_KEY_PATH);
        let st = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            Some(0),
            KEY_SET_VALUE,
            &mut hkey,
        );
        if st.0 != 0 {
            return Err(format!("buka registry Run gagal (kode {})", st.0));
        }

        let legacy = to_wide("DeskNotch");
        let _ = RegDeleteValueW(hkey, PCWSTR(legacy.as_ptr()));

        let res = if enable {
            let target = match exe_path {
                Some(p) => strip_verbatim_path(p),
                None => strip_verbatim_path(
                    &std::env::current_exe().map_err(|e| format!("current_exe gagal: {}", e))?,
                ),
            };
            let cmd = format!("\"{}\"", target.display());
            reg_set_sz(hkey, Some("SelaBar"), &cmd);
            Ok(())
        } else {
            let name = to_wide("SelaBar");
            let _ = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
            Ok(())
        };
        let _ = RegCloseKey(hkey);
        res
    }
}

fn find_selabar_pids(exclude_pid: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pids;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if name.eq_ignore_ascii_case("SelaBar.exe")
                    && entry.th32ProcessID != 0
                    && entry.th32ProcessID != exclude_pid
                {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

/// Gracefully closes any running `SelaBar.exe` instances (`PID != exclude_pid`) and
/// terminates stragglers so the binary file can be updated or removed cleanly.
pub fn stop_running_selabar(exclude_pid: u32) {
    let class_w = to_wide("SelaBarOverlayWnd");
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR(class_w.as_ptr()), PCWSTR::null()) {
            if !hwnd.0.is_null() {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }

    let deadline = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < deadline {
        if find_selabar_pids(exclude_pid).is_empty() {
            return;
        }
        thread::sleep(Duration::from_millis(120));
    }

    for pid in find_selabar_pids(exclude_pid) {
        unsafe {
            if let Ok(proc_h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(proc_h, 0);
                let _ = CloseHandle(proc_h);
            }
        }
    }
    thread::sleep(Duration::from_millis(350));
}

/// Installs `payload_exe` into `install_dir\SelaBar.exe`, registers Windows Uninstall +
/// App Paths entries, creates requested shortcuts, and sets auto-start.
pub fn install_payload(
    payload_exe: &[u8],
    install_dir: &Path,
    opts: &InstallOptions,
) -> Result<PathBuf, String> {
    if payload_exe.is_empty() {
        return Err("Payload SelaBar.exe kosong".to_string());
    }
    stop_running_selabar(std::process::id());

    let clean_dir = strip_verbatim_path(install_dir);
    fs::create_dir_all(&clean_dir)
        .map_err(|e| format!("Gagal membuat folder '{}': {}", clean_dir.display(), e))?;

    let dest_exe = clean_dir.join("SelaBar.exe");
    let tmp_exe = clean_dir.join("SelaBar.exe.tmp");

    fs::write(&tmp_exe, payload_exe)
        .map_err(|e| format!("Gagal menulis file sementara: {}", e))?;

    let mut installed = false;
    let mut last_err = String::new();
    for _ in 0..8 {
        if dest_exe.exists() {
            let _ = fs::remove_file(&dest_exe);
        }
        match fs::rename(&tmp_exe, &dest_exe) {
            Ok(()) => {
                installed = true;
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if !installed {
        let _ = fs::remove_file(&tmp_exe);
        return Err(format!("Gagal menimpa SelaBar.exe ({})", last_err));
    }

    register_windows_uninstall(&dest_exe)?;

    if opts.start_menu_shortcut {
        if let Some(sm_lnk) = start_menu_shortcut_path() {
            let _ = create_shortcut(
                &sm_lnk,
                &dest_exe,
                None,
                "SelaBar — Dynamic Notch & Desktop AI Companion",
            );
        }
    }
    if opts.desktop_shortcut {
        if let Some(desk_lnk) = desktop_shortcut_path() {
            let _ = create_shortcut(
                &desk_lnk,
                &dest_exe,
                None,
                "SelaBar — Dynamic Notch & Desktop AI Companion",
            );
        }
    }

    let _ = set_run_at_startup(opts.start_on_boot, Some(&dest_exe));
    Ok(dest_exe)
}

fn msg_box(title: &str, text: &str, flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> i32 {
    let tw = to_wide(title);
    let bw = to_wide(text);
    unsafe { MessageBoxW(Some(HWND::default()), PCWSTR(bw.as_ptr()), PCWSTR(tw.as_ptr()), flags).0 }
}

/// Handles `SelaBar.exe --uninstall [--quiet]` triggered from Windows Settings -> Apps.
pub fn run_uninstaller(quiet: bool) -> i32 {
    if !quiet {
        let confirm = msg_box(
            "Uninstall SelaBar",
            "Yakin ingin menghapus SelaBar dari komputer ini?\n\nShortcut Start Menu, Desktop, Startup, dan entri aplikasi Windows akan dihapus.",
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        );
        if confirm != IDYES.0 {
            return 1;
        }
    }

    let mut remove_user_data = false;
    let data_dir = app_data_dir();
    if !quiet && data_dir.exists() {
        let ask_data = msg_box(
            "Data Pengguna SelaBar",
            &format!(
                "Hapus juga pengaturan, API key, dan model Whisper yang tersimpan di:\n{}?",
                data_dir.display()
            ),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        );
        remove_user_data = ask_data == IDYES.0;
    }

    stop_running_selabar(std::process::id());
    unregister_windows_entries();
    remove_shortcuts();

    if remove_user_data && data_dir.exists() {
        let _ = fs::remove_dir_all(&data_dir);
    }

    if let Ok(raw_exe) = std::env::current_exe() {
        let exe = strip_verbatim_path(&raw_exe);
        let dir = exe.parent().unwrap_or_else(|| Path::new("."));
        let is_dev_or_repo = dir.join("Cargo.toml").exists()
            || exe.components().any(|c| c.as_os_str() == "target");

        if !is_dev_or_repo {
            let exe_s = exe.to_string_lossy().replace('"', "");
            let dir_s = dir.to_string_lossy().replace('"', "");
            let del_cmd = if is_safe_uninstall_dir(dir) {
                format!(
                    "ping 127.0.0.1 -n 3 >nul & del /F /Q \"{}\" & rmdir /S /Q \"{}\"",
                    exe_s, dir_s
                )
            } else {
                format!("ping 127.0.0.1 -n 3 >nul & del /F /Q \"{}\"", exe_s)
            };
            let _ = Command::new("cmd.exe")
                .args(["/C", &del_cmd])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }

    if !quiet {
        msg_box(
            "Uninstall SelaBar Selesai",
            "SelaBar berhasil dihapus dari sistem Anda.",
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }

    let _ = HANDLE::default();
    0
}

#[cfg(test)]
mod tests {
    use super::is_safe_uninstall_dir;
    use std::path::PathBuf;

    #[test]
    fn safe_uninstall_dir_rejects_shared_folders_and_workspaces() {
        let dedicated = std::env::temp_dir()
            .join(format!("selabar-inst-safe-{}", std::process::id()))
            .join("Programs")
            .join("SelaBar");
        assert!(is_safe_uninstall_dir(&dedicated));

        let downloads = PathBuf::from(r"C:\Users\Demo\Downloads");
        assert!(!is_safe_uninstall_dir(&downloads));

        let desktop = PathBuf::from(r"C:\Users\Demo\Desktop");
        assert!(!is_safe_uninstall_dir(&desktop));

        let drive_root = PathBuf::from(r"D:\");
        assert!(!is_safe_uninstall_dir(&drive_root));

        let target_dir = PathBuf::from(r"D:\repo\target\release\SelaBar");
        assert!(!is_safe_uninstall_dir(&target_dir));

        let repo_dir = std::env::temp_dir()
            .join(format!("selabar-repo-guard-{}", std::process::id()))
            .join("SelaBar");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::write(repo_dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert!(!is_safe_uninstall_dir(&repo_dir));
        let _ = std::fs::remove_dir_all(repo_dir.parent().unwrap());
    }
}
