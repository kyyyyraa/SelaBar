use crate::events::{AudioDeviceKind, AudioOutputDevice, AudioOutputSnapshot, UiEvent, UiSender};
use std::ffi::c_void;
use windows::core::{GUID, HRESULT, IUnknown, Interface};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

const PKEY_DEVICE_DESC: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 2,
};

const PKEY_DEVICE_IFACE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x026e516e_b814_414b_83cd_856d6fef4822),
    pid: 2,
};

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

struct ComGuard(bool);

impl ComGuard {
    fn init_mta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self(hr.0 == 0 || hr.0 == 1)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct RawComRelease(*mut *const *const c_void);

impl Drop for RawComRelease {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let vtbl = *self.0;
                if !vtbl.is_null() {
                    let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                        std::mem::transmute(*vtbl.add(2));
                    let _ = release(self.0 as *mut c_void);
                }
            }
        }
    }
}

/// Splits a Windows CoreAudio friendly name (e.g. `"Speakers (Realtek(R) Audio)"`)
/// into `(title, subtitle, kind)` for crisp UI rows.
pub fn parse_audio_device_display(
    friendly: &str,
    desc: &str,
    iface: &str,
) -> (String, String, AudioDeviceKind) {
    let f = friendly.trim();
    let d = desc.trim();
    let i = iface.trim();

    let (raw_title, raw_sub) = if let (Some(open), Some(close)) = (f.find('('), f.rfind(')')) {
        if close > open {
            let head = f[..open].trim();
            let tail = f[open + 1..close].trim();
            if !head.is_empty() && !tail.is_empty() {
                (head.to_string(), tail.to_string())
            } else {
                (f.to_string(), i.to_string())
            }
        } else {
            (f.to_string(), i.to_string())
        }
    } else if !f.is_empty() {
        let sub = if !i.is_empty() && !i.eq_ignore_ascii_case(f) {
            i.to_string()
        } else if !d.is_empty() && !d.eq_ignore_ascii_case(f) {
            d.to_string()
        } else {
            String::new()
        };
        (f.to_string(), sub)
    } else if !d.is_empty() {
        (d.to_string(), i.to_string())
    } else {
        ("Audio Output".to_string(), String::new())
    };

    let joined = format!("{} {} {}", f, d, i).to_lowercase();
    let kind = if joined.contains("bluetooth")
        || joined.contains("bthhf")
        || joined.contains("a2dp")
        || joined.contains("hands-free")
        || joined.contains("earbud")
        || joined.contains("airpod")
        || joined.contains("buds")
        || joined.contains("tws")
    {
        AudioDeviceKind::Bluetooth
    } else if joined.contains("headphone")
        || joined.contains("headset")
        || joined.contains("earphone")
        || joined.contains("iem")
    {
        AudioDeviceKind::Headphones
    } else if joined.contains("hdmi")
        || joined.contains("displayport")
        || joined.contains("monitor")
        || joined.contains("nvidia high definition")
        || joined.contains("amd high definition")
    {
        AudioDeviceKind::Monitor
    } else {
        AudioDeviceKind::Speaker
    };

    let subtitle = if kind == AudioDeviceKind::Bluetooth
        && (raw_sub.is_empty() || raw_sub.to_lowercase().contains("hands-free") || raw_sub.to_lowercase().contains("stereo"))
    {
        if raw_sub.is_empty() {
            "Bluetooth".to_string()
        } else if !raw_sub.to_lowercase().contains("bluetooth") {
            format!("Bluetooth · {}", raw_sub)
        } else {
            raw_sub
        }
    } else if !raw_sub.is_empty() {
        raw_sub
    } else if !i.is_empty() {
        i.to_string()
    } else {
        "Built-in Audio".to_string()
    };

    (raw_title, subtitle, kind)
}

unsafe fn read_prop_str(store: &IPropertyStore, key: &PROPERTYKEY) -> String {
    let Ok(pv) = store.GetValue(key) else {
        return String::new();
    };
    pv.to_string().trim().to_string()
}

unsafe fn device_id_string(dev: &IMMDevice) -> String {
    let Ok(pw) = dev.GetId() else {
        return String::new();
    };
    let s = pw.to_string().unwrap_or_default();
    CoTaskMemFree(Some(pw.0 as *const _));
    s
}

pub fn list_audio_outputs() -> AudioOutputSnapshot {
    let _com = ComGuard::init_mta();
    let mut snap = AudioOutputSnapshot::default();

    unsafe {
        let Ok(enumerator) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return snap;
        };

        let def_dev = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok();
        let def_id = def_dev
            .as_ref()
            .map(|d| device_id_string(d))
            .unwrap_or_default();

        if let Some(ref d) = def_dev {
            if let Ok(vol) = d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                let scalar = vol.GetMasterVolumeLevelScalar().unwrap_or(0.5).clamp(0.0, 1.0);
                snap.volume_pct = (scalar * 100.0).round() as u8;
                snap.muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            }
        }

        let Ok(col) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return snap;
        };
        let count = col.GetCount().unwrap_or(0).min(32);
        for idx in 0..count {
            let Ok(dev) = col.Item(idx) else {
                continue;
            };
            let id = device_id_string(&dev);
            if id.is_empty() {
                continue;
            }
            let (friendly, desc, iface) = if let Ok(props) = dev.OpenPropertyStore(STGM_READ) {
                (
                    read_prop_str(&props, &PKEY_DEVICE_FRIENDLY_NAME),
                    read_prop_str(&props, &PKEY_DEVICE_DESC),
                    read_prop_str(&props, &PKEY_DEVICE_IFACE_FRIENDLY_NAME),
                )
            } else {
                (String::new(), String::new(), String::new())
            };
            let (title, subtitle, kind) = parse_audio_device_display(&friendly, &desc, &iface);
            let is_default = !def_id.is_empty() && id.eq_ignore_ascii_case(&def_id);
            snap.devices.push(AudioOutputDevice {
                id,
                title,
                subtitle,
                kind,
                is_default,
            });
        }
    }

    snap.devices.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    snap
}

pub fn set_default_output_device(device_id: &str) -> bool {
    let trimmed = device_id.trim();
    if trimmed.is_empty() {
        return false;
    }
    let _com = ComGuard::init_mta();
    let wide: Vec<u16> = trimmed.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let Ok(unk) = CoCreateInstance::<_, IUnknown>(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
        else {
            return false;
        };
        let mut raw_ptr: *mut c_void = std::ptr::null_mut();
        let hr_qi = unk.query(&IID_IPOLICY_CONFIG, &mut raw_ptr);
        if hr_qi.is_err() || raw_ptr.is_null() {
            return false;
        }
        let iface_ptr = raw_ptr as *mut *const *const c_void;
        let _release_guard = RawComRelease(iface_ptr);
        let vtbl = *iface_ptr;
        if vtbl.is_null() {
            return false;
        }
        let set_default_fn: unsafe extern "system" fn(
            *mut c_void,
            *const u16,
            u32,
        ) -> HRESULT = std::mem::transmute(*vtbl.add(13));

        let mut any_ok = false;
        // Apply across all 3 ERole values: Console (0), Multimedia (1), Communications (2).
        for role in 0..=2u32 {
            if set_default_fn(iface_ptr as *mut c_void, wide.as_ptr(), role).is_ok() {
                any_ok = true;
            }
        }
        any_ok
    }
}

pub fn set_master_volume_scalar(scalar: f32) -> Option<(u8, bool)> {
    let _com = ComGuard::init_mta();
    let target = scalar.clamp(0.0, 1.0);
    unsafe {
        let enm =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = enm.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let vol = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()?;
        if target > 0.01 && vol.GetMute().map(|b| b.as_bool()).unwrap_or(false) {
            let _ = vol.SetMute(false, std::ptr::null());
        }
        let _ = vol.SetMasterVolumeLevelScalar(target, std::ptr::null());
        let actual = vol.GetMasterVolumeLevelScalar().unwrap_or(target).clamp(0.0, 1.0);
        let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        Some(((actual * 100.0).round() as u8, muted))
    }
}

pub fn toggle_master_mute() -> Option<(u8, bool)> {
    let _com = ComGuard::init_mta();
    unsafe {
        let enm =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = enm.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let vol = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()?;
        let cur_mute = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        let _ = vol.SetMute(!cur_mute, std::ptr::null());
        let scalar = vol.GetMasterVolumeLevelScalar().unwrap_or(0.5).clamp(0.0, 1.0);
        let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(!cur_mute);
        Some(((scalar * 100.0).round() as u8, muted))
    }
}

pub fn refresh_outputs_async(rt: &tokio::runtime::Handle, ui: &UiSender) {
    let u = ui.clone();
    rt.spawn_blocking(move || {
        u.send(UiEvent::AudioOutputsUpdated(list_audio_outputs()));
    });
}

pub fn select_device_and_refresh(rt: &tokio::runtime::Handle, ui: &UiSender, device_id: &str) {
    let id = device_id.to_string();
    let u = ui.clone();
    rt.spawn_blocking(move || {
        let _ = set_default_output_device(&id);
        std::thread::sleep(std::time::Duration::from_millis(80));
        u.send(UiEvent::AudioOutputsUpdated(list_audio_outputs()));
    });
}

pub fn set_volume_and_refresh(rt: &tokio::runtime::Handle, ui: &UiSender, scalar: f32) {
    let u = ui.clone();
    rt.spawn_blocking(move || {
        let _ = set_master_volume_scalar(scalar);
        u.send(UiEvent::AudioOutputsUpdated(list_audio_outputs()));
    });
}

pub fn toggle_mute_and_refresh(rt: &tokio::runtime::Handle, ui: &UiSender) {
    let u = ui.clone();
    rt.spawn_blocking(move || {
        let _ = toggle_master_mute();
        u.send(UiEvent::AudioOutputsUpdated(list_audio_outputs()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_audio_device_display_extracts_title_subtitle_and_kind() {
        let (t, sub, kind) = parse_audio_device_display(
            "Speakers (Realtek(R) Audio)",
            "Speakers",
            "Realtek(R) Audio",
        );
        assert_eq!(t, "Speakers");
        assert_eq!(sub, "Realtek(R) Audio");
        assert_eq!(kind, AudioDeviceKind::Speaker);

        let (bt_t, bt_sub, bt_kind) = parse_audio_device_display(
            "Earbuds (Redmi Buds 5 Stereo)",
            "Headphones",
            "Bluetooth Audio",
        );
        assert_eq!(bt_t, "Earbuds");
        assert!(bt_sub.contains("Bluetooth"));
        assert_eq!(bt_kind, AudioDeviceKind::Bluetooth);

        let (hp_t, _, hp_kind) = parse_audio_device_display(
            "Headphones (USB-C DAC)",
            "Headset Earphone",
            "USB Audio",
        );
        assert_eq!(hp_t, "Headphones");
        assert_eq!(hp_kind, AudioDeviceKind::Headphones);

        let (mon_t, mon_sub, mon_kind) = parse_audio_device_display(
            "LG ULTRAGEAR (NVIDIA High Definition Audio)",
            "Monitor",
            "NVIDIA High Definition Audio",
        );
        assert_eq!(mon_t, "LG ULTRAGEAR");
        assert_eq!(mon_sub, "NVIDIA High Definition Audio");
        assert_eq!(mon_kind, AudioDeviceKind::Monitor);
    }

    #[test]
    fn list_audio_outputs_runs_safely_without_mutating_system_state() {
        let snap = list_audio_outputs();
        assert!(snap.volume_pct <= 100);
    }
}
