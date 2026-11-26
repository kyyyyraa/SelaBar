use crate::events::PrivacySnapshot;
use std::sync::{Mutex, OnceLock};
use windows::core::{implement, Interface, Ref, Result as WinResult};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eCapture, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::Media::MediaFoundation::{
    IMFSensorActivitiesReport, IMFSensorActivitiesReportCallback,
    IMFSensorActivitiesReportCallback_Impl, IMFSensorActivityMonitor,
    MFCreateSensorActivityMonitor, MFStartup, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE,
};

const KEY_READ_VALUE: u32 = 0x20019;
const ERROR_SUCCESS: u32 = 0;
const ERROR_NO_MORE_ITEMS: u32 = 259;

#[derive(Debug, Clone, Default)]
pub struct DeviceAccess {
    pub in_use: bool,
    pub apps: Vec<String>,
}

fn process_stem_from_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            hp,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(hp);
        if !ok || len == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let stem = std::path::Path::new(&full)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if stem.is_empty()
            || matches!(
                stem.to_lowercase().as_str(),
                "audiodg" | "svchost" | "system" | "idle" | "frameserver" | "selabar" | "desknotch"
            )
        {
            None
        } else {
            Some(stem)
        }
    }
}

fn query_coreaudio_capture_apps() -> DeviceAccess {
    let mut apps = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(enm) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return DeviceAccess::default();
        };
        let Ok(col) = enm.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
            return DeviceAccess::default();
        };
        let count = col.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(dev) = col.Item(i) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sess_enum) = mgr.GetSessionEnumerator() else {
                continue;
            };
            let scount = sess_enum.GetCount().unwrap_or(0);
            for j in 0..scount {
                let Ok(ctrl) = sess_enum.GetSession(j) else {
                    continue;
                };
                if ctrl.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }
                let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let pid = ctrl2.GetProcessId().unwrap_or(0);
                if let Some(stem) = process_stem_from_pid(pid) {
                    if !apps.iter().any(|x: &String| x.eq_ignore_ascii_case(&stem)) {
                        apps.push(stem);
                    }
                }
            }
        }
    }
    DeviceAccess {
        in_use: !apps.is_empty(),
        apps,
    }
}

static MF_CAM_APPS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
fn cam_apps_slot() -> &'static Mutex<Vec<String>> {
    MF_CAM_APPS.get_or_init(|| Mutex::new(Vec::new()))
}

#[implement(IMFSensorActivitiesReportCallback)]
struct CameraSensorCallback;

impl IMFSensorActivitiesReportCallback_Impl for CameraSensorCallback_Impl {
    fn OnActivitiesReport(
        &self,
        report: Ref<'_, IMFSensorActivitiesReport>,
    ) -> WinResult<()> {
        let mut active = Vec::new();
        if let Some(rep) = report.as_ref() {
            unsafe {
                let n = rep.GetCount().unwrap_or(0);
                for i in 0..n {
                    let Ok(dev_rep) = rep.GetActivityReport(i) else {
                        continue;
                    };
                    let pcount = dev_rep.GetProcessCount().unwrap_or(0);
                    for j in 0..pcount {
                        let Ok(pa) = dev_rep.GetProcessActivity(j) else {
                            continue;
                        };
                        if pa.GetStreamingState().map(|b| b.as_bool()).unwrap_or(false) {
                            let pid = pa.GetProcessId().unwrap_or(0);
                            if let Some(stem) = process_stem_from_pid(pid) {
                                if !active.iter().any(|x: &String| x.eq_ignore_ascii_case(&stem)) {
                                    active.push(stem);
                                }
                            } else if !active.iter().any(|x| x == "Camera") {
                                active.push("Camera".to_string());
                            }
                        }
                    }
                }
            }
        }
        if let Ok(mut g) = cam_apps_slot().lock() {
            *g = active;
        }
        Ok(())
    }
}

struct MonitorHolder(#[allow(dead_code)] IMFSensorActivityMonitor);
unsafe impl Send for MonitorHolder {}
unsafe impl Sync for MonitorHolder {}

static MF_MONITOR: OnceLock<Option<MonitorHolder>> = OnceLock::new();

fn ensure_camera_monitor_started() {
    MF_MONITOR.get_or_init(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            return None;
        }
        let cb: IMFSensorActivitiesReportCallback = CameraSensorCallback.into();
        let mon = MFCreateSensorActivityMonitor(&cb).ok()?;
        let _ = mon.Start();
        Some(MonitorHolder(mon))
    });
}

fn read_reg_i64(key: HKEY, name: &str) -> Option<i64> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut value: i64 = 0;
    let mut size: u32 = std::mem::size_of::<i64>() as u32;
    let mut kind: u32 = 0;
    let ok = unsafe {
        RegQueryValueExW(
            key,
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            &mut value as *mut i64 as *mut u8,
            &mut size,
        )
    };
    if ok != ERROR_SUCCESS {
        None
    } else {
        Some(value)
    }
}

fn filetime_to_unix(ft: i64) -> i64 {
    if ft <= 0 {
        return 0;
    }
    ft / 10_000_000 - 11_644_473_600
}

fn pretty_app_name(mangled: &str, packaged: bool) -> String {
    if packaged {
        let base = mangled.split('_').next().unwrap_or(mangled);
        let tail = base.rsplit('.').next().unwrap_or(base).trim();
        return tail.to_string();
    }
    let restored = mangled.replace('#', "\\");
    let file = restored.rsplit('\\').next().unwrap_or(mangled).trim();
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_string()
}

/// Classify one ConsentStore entry: (device currently open, last-use unix secs).
/// LastUsedTimeStop == 0 means "still open", but browsers (Zen/Chrome running
/// WhatsApp Web) sometimes never write the stop timestamp after a call ends.
/// Such an entry only counts as open for 2h after its start; after that it is
/// treated as stale so mic-based call auto-end is not blocked forever.
fn classify_registry_use(stop: Option<i64>, start: Option<i64>, now: i64) -> (bool, i64) {
    let stop_at = stop.map(filetime_to_unix).unwrap_or(0);
    let start_at = start.map(filetime_to_unix).unwrap_or(0);
    let active = stop == Some(0) && start_at > 0 && (now - start_at).abs() <= 7200;
    let last_use = if active { now } else { stop_at.max(start_at) };
    (active, last_use)
}

fn scan_key_subentries(
    root: HKEY,
    path: &str,
    packaged: bool,
    now: i64,
    recent_secs: i64,
    in_use: &mut bool,
    entries: &mut Vec<(i64, String)>,
) {
    let wide_path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hkey: HKEY = std::ptr::null_mut();
    let ok = unsafe { RegOpenKeyExW(root, wide_path.as_ptr(), 0, KEY_READ_VALUE, &mut hkey) };
    if ok != ERROR_SUCCESS || hkey.is_null() {
        return;
    }

    let mut index: u32 = 0;
    loop {
        let mut name_buf = [0u16; 512];
        let mut name_len: u32 = name_buf.len() as u32;
        let ok = unsafe {
            RegEnumKeyExW(
                hkey,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == ERROR_NO_MORE_ITEMS || ok != ERROR_SUCCESS {
            break;
        }
        index += 1;

        let raw_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        if packaged && raw_name.eq_ignore_ascii_case("NonPackaged") {
            continue;
        }

        let mut sub: HKEY = std::ptr::null_mut();
        let ok = unsafe { RegOpenKeyExW(hkey, name_buf.as_ptr(), 0, KEY_READ_VALUE, &mut sub) };
        if ok != ERROR_SUCCESS || sub.is_null() {
            continue;
        }
        let stop = read_reg_i64(sub, "LastUsedTimeStop");
        let start = read_reg_i64(sub, "LastUsedTimeStart");
        unsafe { RegCloseKey(sub) };

        let (active, last_use) = classify_registry_use(stop, start, now);
        if active {
            *in_use = true;
        }
        if last_use > 0 && now - last_use <= recent_secs {
            let pretty = pretty_app_name(&raw_name, packaged);
            if !pretty.is_empty() {
                entries.push((last_use, pretty));
            }
        }
    }
    unsafe { RegCloseKey(hkey) };
}

fn scan_device_registry(device: &str, recent_secs: i64) -> DeviceAccess {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let base = format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{}",
        device
    );
    let non_packaged = format!(r"{}\NonPackaged", base);

    let mut in_use = false;
    let mut entries: Vec<(i64, String)> = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        scan_key_subentries(
            root,
            &non_packaged,
            false,
            now,
            recent_secs,
            &mut in_use,
            &mut entries,
        );
        scan_key_subentries(root, &base, true, now, recent_secs, &mut in_use, &mut entries);
    }

    entries.sort_by(|a, b| b.0.cmp(&a.0));
    let mut apps = Vec::new();
    for (_, name) in entries {
        if !apps.iter().any(|x: &String| x.eq_ignore_ascii_case(&name)) {
            apps.push(name);
            if apps.len() >= 4 {
                break;
            }
        }
    }

    DeviceAccess { in_use, apps }
}

fn merge_device(live: DeviceAccess, reg: DeviceAccess) -> DeviceAccess {
    let mut apps = live.apps;
    for a in reg.apps {
        if !apps.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            apps.push(a);
            if apps.len() >= 4 {
                break;
            }
        }
    }
    DeviceAccess {
        in_use: live.in_use || reg.in_use,
        apps,
    }
}

pub fn read_state(recent_secs: i64) -> PrivacySnapshot {
    ensure_camera_monitor_started();
    let live_mic = query_coreaudio_capture_apps();
    let live_mic_apps = live_mic.apps.clone();
    let mf_cam_apps = cam_apps_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let live_cam = DeviceAccess {
        in_use: !mf_cam_apps.is_empty(),
        apps: mf_cam_apps,
    };

    let reg_mic = scan_device_registry("microphone", recent_secs);
    let reg_cam = scan_device_registry("webcam", recent_secs);

    // CoreAudio is the ground truth for active capture sessions: browsers often
    // leave ConsentStore LastUsedTimeStop == 0 after a WebRTC call ends, so do
    // not let a stale browser ConsentStore entry keep `mic.in_use` stuck true.
    let mic_apps = if live_mic.in_use {
        live_mic.apps.clone()
    } else if reg_mic.in_use && recent_secs > 0 {
        reg_mic
            .apps
            .iter()
            .filter(|a| {
                let low = a.to_lowercase();
                !matches!(
                    low.as_str(),
                    "zen" | "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc"
                )
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let mic_in_use = live_mic.in_use || !mic_apps.is_empty();
    let cam = merge_device(live_cam, reg_cam);
    PrivacySnapshot {
        mic: mic_in_use,
        camera: cam.in_use,
        mic_apps,
        camera_apps: cam.apps,
        live_mic_apps,
    }
}

pub fn is_voip_mic_active(snap: &PrivacySnapshot) -> bool {
    let apps = if !snap.live_mic_apps.is_empty() {
        &snap.live_mic_apps
    } else if snap.mic {
        &snap.mic_apps
    } else {
        return false;
    };
    apps.iter().any(|a| {
        let low = a.to_lowercase();
        [
            "whatsapp", "discord", "telegram", "zoom", "teams", "ms-teams", "zen", "chrome",
            "msedge", "brave", "firefox", "skype",
        ]
        .iter()
        .any(|v| low.contains(v))
    })
}

/// Real-time call mic signal backed ONLY by CoreAudio live capture sessions
/// (never ConsentStore registry history) and isolated per target app/browser
/// so another app's mic activity (e.g. Discord) never keeps a WhatsApp Web call
/// stuck active.
pub fn is_call_mic_live(snap: &PrivacySnapshot, raw_app: &str, app_name: &str) -> bool {
    if snap.live_mic_apps.is_empty() {
        return false;
    }
    let raw_low = raw_app.to_lowercase();
    let app_low = app_name.to_lowercase();

    let expected_stems: Option<&[&str]> = if raw_low.contains("zen") {
        Some(&["zen", "zenbrowser", "firefox"])
    } else if raw_low.contains("chrome") {
        Some(&["chrome"])
    } else if raw_low.contains("msedge") || raw_low.contains("edge") {
        Some(&["msedge"])
    } else if raw_low.contains("firefox") {
        Some(&["firefox"])
    } else if raw_low.contains("brave") {
        Some(&["brave"])
    } else if raw_low.contains("opera") {
        Some(&["opera"])
    } else if raw_low.contains("vivaldi") {
        Some(&["vivaldi"])
    } else if raw_low.contains("arc") {
        Some(&["arc"])
    } else if raw_low.contains("whatsapp") || app_low.contains("whatsapp") {
        Some(&["whatsapp"])
    } else if raw_low.contains("discord") || app_low.contains("discord") {
        Some(&["discord"])
    } else if raw_low.contains("telegram") || app_low.contains("telegram") {
        Some(&["telegram"])
    } else if raw_low.contains("zoom") || app_low.contains("zoom") {
        Some(&["zoom"])
    } else if raw_low.contains("ms-teams") || raw_low.contains("teams") || app_low.contains("teams") {
        Some(&["teams", "ms-teams"])
    } else if raw_low.contains("skype") || app_low.contains("skype") {
        Some(&["skype"])
    } else if raw_low.is_empty() && app_low.is_empty() {
        Some(&[
            "whatsapp", "discord", "telegram", "zoom", "teams", "ms-teams", "zen", "chrome",
            "msedge", "brave", "firefox", "opera", "vivaldi", "arc", "skype",
        ])
    } else {
        None
    };

    snap.live_mic_apps.iter().any(|a| {
        let low = a.to_lowercase();
        if let Some(stems) = expected_stems {
            stems
                .iter()
                .any(|s| low == *s || low.contains(s) || s.contains(&low.as_str()))
        } else {
            (!app_low.is_empty() && (low.contains(&app_low) || app_low.contains(&low)))
                || (!raw_low.is_empty() && raw_low.contains(&low))
        }
    })
}

pub fn changed(a: &PrivacySnapshot, b: &PrivacySnapshot) -> bool {
    a.mic != b.mic
        || a.camera != b.camera
        || a.mic_apps != b.mic_apps
        || a.camera_apps != b.camera_apps
        || a.live_mic_apps != b.live_mic_apps
}

pub fn poll_changed(last: &mut PrivacySnapshot) -> Option<PrivacySnapshot> {
    let now = read_state(0);
    if changed(last, &now) {
        *last = now.clone();
        Some(now)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft(unix: i64) -> i64 {
        (unix + 11_644_473_600) * 10_000_000
    }

    #[test]
    fn registry_open_entry_counts_as_in_use() {
        let now = 1_800_000_000;
        let (active, last_use) = classify_registry_use(Some(0), Some(ft(now - 60)), now);
        assert!(active);
        assert_eq!(last_use, now);
    }

    #[test]
    fn registry_stale_open_entry_expires_after_2h() {
        let now = 1_800_000_000;
        // Browser left LastUsedTimeStop == 0 after the call ended 3h ago.
        let (active, last_use) = classify_registry_use(Some(0), Some(ft(now - 3 * 3600)), now);
        assert!(!active, "stale open entry must not keep mic 'in use' forever");
        assert_eq!(last_use, now - 3 * 3600);
        // Missing stop value entirely is not "open" either.
        let (active, _) = classify_registry_use(None, Some(ft(now - 60)), now);
        assert!(!active);
    }

    #[test]
    fn registry_closed_entry_uses_stop_time() {
        let now = 1_800_000_000;
        let (active, last_use) =
            classify_registry_use(Some(ft(now - 30)), Some(ft(now - 300)), now);
        assert!(!active);
        assert_eq!(last_use, now - 30);
    }

    #[test]
    fn voip_mic_match_covers_browser_stems() {
        let snap = PrivacySnapshot {
            mic: true,
            camera: false,
            mic_apps: vec!["zen".into()],
            camera_apps: vec![],
            live_mic_apps: vec!["zen".into()],
        };
        assert!(is_voip_mic_active(&snap));
        assert!(is_call_mic_live(&snap, "ZenToast-F0DC", "WhatsApp"));
        // Cross-app isolation: Discord live mic must NOT match a Zen/WhatsApp Web call.
        let discord_only = PrivacySnapshot {
            mic_apps: vec!["discord".into()],
            live_mic_apps: vec!["discord".into()],
            ..snap.clone()
        };
        assert!(!is_call_mic_live(&discord_only, "ZenToast-F0DC", "WhatsApp"));
        assert!(is_call_mic_live(&discord_only, "Discord", "Discord"));

        // Even if ConsentStore (`mic_apps`) stayed stale after hangup, once CoreAudio
        // drops `live_mic_apps` `is_call_mic_live` returns false immediately.
        let hung_up = PrivacySnapshot {
            mic: true,
            mic_apps: vec!["zen".into()],
            live_mic_apps: vec![],
            ..snap.clone()
        };
        assert!(!is_call_mic_live(&hung_up, "ZenToast-F0DC", "WhatsApp"));

        let idle = PrivacySnapshot {
            mic: false,
            live_mic_apps: vec![],
            ..snap.clone()
        };
        assert!(!is_voip_mic_active(&idle));
        let other = PrivacySnapshot {
            mic_apps: vec!["spotify".into()],
            live_mic_apps: vec!["spotify".into()],
            ..snap
        };
        assert!(!is_voip_mic_active(&other));
        assert!(!is_call_mic_live(&other, "ZenToast-F0DC", "WhatsApp"));
    }
}
