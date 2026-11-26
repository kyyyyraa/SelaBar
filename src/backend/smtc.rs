use crate::events::{SmtcSessionInfo, UiEvent, UiSender};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties,
};
use windows::Storage::Streams::{DataReader, IInputStream};

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
    fn CoUninitialize();
}

struct ComApartmentGuard(bool);

impl ComApartmentGuard {
    fn init_mta() -> Self {
        let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), 0) };
        // S_OK (0) and S_FALSE (1) both increment the COM init refcount and require CoUninitialize.
        Self(hr == 0 || hr == 1)
    }
}

impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

static LAST_PLAYPAUSE_MS: AtomicU64 = AtomicU64::new(0);
static PLAYPAUSE_INFLIGHT: AtomicBool = AtomicBool::new(false);

struct InflightGuard(&'static AtomicBool);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn now_monotonic_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let start = EPOCH.get_or_init(Instant::now);
    1_000_u64.saturating_add(start.elapsed().as_millis() as u64)
}

pub(crate) fn should_debounce_media_key(last_ms: u64, now_ms: u64, window_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) < window_ms
}

fn sniff_mime(buf: &[u8]) -> &'static str {
    if buf.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if buf.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if buf.starts_with(b"GIF8") {
        "image/gif"
    } else if buf.starts_with(b"BM") {
        "image/bmp"
    } else if buf.len() > 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

fn compute_average_tint(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.resize_exact(12, 12, image::imageops::FilterType::Triangle);
    let rgba = small.to_rgba8();
    let mut sr = 0u64;
    let mut sg = 0u64;
    let mut sb = 0u64;
    let mut cnt = 0u64;
    for p in rgba.pixels() {
        if p[3] > 32 {
            sr += p[0] as u64;
            sg += p[1] as u64;
            sb += p[2] as u64;
            cnt += 1;
        }
    }
    if cnt == 0 {
        return None;
    }
    Some((
        (sr / cnt).min(255) as u8,
        (sg / cnt).min(255) as u8,
        (sb / cnt).min(255) as u8,
    ))
}

fn extract_thumbnail(
    props: &GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<(Vec<u8>, String, Option<(u8, u8, u8)>)> {
    for attempt in 0..4 {
        let res = (|| -> Option<(Vec<u8>, String, Option<(u8, u8, u8)>)> {
            let thumb_ref = props.Thumbnail().ok()?;
            let stream_op = thumb_ref.OpenReadAsync().ok()?;
            let stream = stream_op.get().ok()?;
            let size = stream.Size().ok()?;
            if size == 0 || size >= 5 * 1024 * 1024 {
                return None;
            }
            let input_stream: IInputStream = stream.cast().ok()?;
            let reader = DataReader::CreateDataReader(&input_stream).ok()?;
            let mut loaded = 0u32;
            for _ in 0..10 {
                loaded = reader.LoadAsync(size as u32).and_then(|op| op.get()).ok()?;
                if loaded as u64 >= size {
                    break;
                }
            }
            if (loaded as u64) < size {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            reader.ReadBytes(&mut buf).ok()?;
            let mime = sniff_mime(&buf).to_string();
            let tint = compute_average_tint(&buf);
            Some((buf, mime, tint))
        })();

        if res.is_some() {
            return res;
        }
        if attempt < 3 {
            std::thread::sleep(Duration::from_millis(200 + attempt * 150));
        }
    }
    None
}

fn foreground_title() -> String {
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> *mut core::ffi::c_void;
        fn GetWindowTextW(hwnd: *mut core::ffi::c_void, s: *mut u16, n: i32) -> i32;
    }
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), 512);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize]).to_lowercase()
    }
}

fn is_playing_session(s: &GlobalSystemMediaTransportControlsSession) -> bool {
    s.GetPlaybackInfo()
        .ok()
        .and_then(|p| p.PlaybackStatus().ok())
        .map(|st| st.0 == 4)
        .unwrap_or(false)
}

fn session_title_lower(s: &GlobalSystemMediaTransportControlsSession) -> String {
    s.TryGetMediaPropertiesAsync()
        .ok()
        .and_then(|o| o.get().ok())
        .and_then(|pr| pr.Title().ok())
        .map(|h| h.to_string().to_lowercase())
        .unwrap_or_default()
}

fn now_filetime_ticks() -> i64 {
    let unix_100ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_nanos() / 100) as i64)
        .unwrap_or(0);
    unix_100ns.saturating_add(116_444_736_000_000_000)
}

/// Computes `(effective_pos_sec, duration_sec, raw_pos_ms, last_updated_ticks)`.
/// Extrapolates using `LastUpdatedTime()` while playing if the player updates
/// SMTC timeline sparsely (common in browsers/desktop players).
fn compute_timeline_pos(
    session: &GlobalSystemMediaTransportControlsSession,
    is_playing: bool,
) -> (f64, f64, u64, i64) {
    let timeline = session.GetTimelineProperties().ok();
    let start_ticks = timeline
        .as_ref()
        .and_then(|t| t.StartTime().ok())
        .map(|s| s.Duration)
        .unwrap_or(0);
    let end_ticks = timeline
        .as_ref()
        .and_then(|t| t.EndTime().ok())
        .map(|e| e.Duration)
        .unwrap_or(0);
    let pos_ticks = timeline
        .as_ref()
        .and_then(|t| t.Position().ok())
        .map(|p| p.Duration)
        .unwrap_or(0);
    let last_updated_ticks = timeline
        .as_ref()
        .and_then(|t| t.LastUpdatedTime().ok())
        .map(|d| d.UniversalTime)
        .unwrap_or(0);

    let duration = if end_ticks > start_ticks {
        (end_ticks - start_ticks) as f64 / 10_000_000.0
    } else {
        0.0
    };
    let raw_pos = if end_ticks > start_ticks {
        (pos_ticks - start_ticks).max(0) as f64 / 10_000_000.0
    } else {
        pos_ticks.max(0) as f64 / 10_000_000.0
    };
    let raw_pos_ms = (raw_pos.max(0.0) * 1000.0) as u64;

    let mut effective_pos = raw_pos;
    if is_playing && last_updated_ticks > 0 {
        let elapsed_secs = (now_filetime_ticks() - last_updated_ticks).max(0) as f64 / 10_000_000.0;
        let reasonable_extrap =
            (raw_pos_ms > 0 && elapsed_secs < 1800.0)
                || (duration > 0.0 && elapsed_secs <= duration && elapsed_secs < 1800.0);
        if reasonable_extrap {
            effective_pos += elapsed_secs;
            if duration > 0.0 && effective_pos > duration {
                effective_pos = duration;
            }
        }
    }

    (effective_pos.max(0.0), duration.max(0.0), raw_pos_ms, last_updated_ticks)
}

fn session_app_lower(s: &GlobalSystemMediaTransportControlsSession) -> String {
    s.SourceAppUserModelId()
        .map(|h| h.to_string().to_lowercase())
        .unwrap_or_default()
}

/// Prefer session matching UI's current app_id / title, else active session.
fn pick_session_for(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    preferred_app_id: Option<&str>,
    preferred_title: Option<&str>,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    let app_pref = preferred_app_id.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    let title_pref = preferred_title.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if !app_pref.is_empty() || !title_pref.is_empty() {
        if let Ok(sessions) = manager.GetSessions() {
            if let Ok(count) = sessions.Size() {
                let mut by_both = None;
                let mut by_app = None;
                let mut by_title = None;
                for i in 0..count {
                    if let Ok(s) = sessions.GetAt(i) {
                        let a_ok = !app_pref.is_empty() && session_app_lower(&s) == app_pref;
                        let t_ok = !title_pref.is_empty() && session_title_lower(&s) == title_pref;
                        if a_ok && t_ok {
                            by_both = Some(s);
                            break;
                        }
                        if a_ok && by_app.is_none() {
                            by_app = Some(s);
                        } else if t_ok && by_title.is_none() {
                            by_title = Some(s);
                        }
                    }
                }
                if let Some(s) = by_both.or(by_app).or(by_title) {
                    return Some(s);
                }
            }
        }
    }
    pick_active_session(manager)
}

/// Shared smart session selector used across metadata, timeline, seek, and media keys.
fn pick_active_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    let mut session = match manager.GetCurrentSession() {
        Ok(s) => s,
        Err(_) => {
            let sessions = manager.GetSessions().ok()?;
            let count = sessions.Size().ok()?;
            if count == 0 {
                return None;
            }
            let mut picked = None;
            for i in 0..count {
                if let Ok(s) = sessions.GetAt(i) {
                    if is_playing_session(&s) {
                        picked = Some(s);
                        break;
                    }
                    if picked.is_none() {
                        picked = Some(s);
                    }
                }
            }
            picked?
        }
    };

    let fg = foreground_title();
    if !fg.is_empty() {
        let cur_title = session_title_lower(&session);
        if !cur_title.is_empty() && !fg.contains(&cur_title) {
            if let Ok(sessions) = manager.GetSessions() {
                if let Ok(count) = sessions.Size() {
                    for k in 0..count {
                        if let Ok(cand) = sessions.GetAt(k) {
                            if !is_playing_session(&cand) {
                                continue;
                            }
                            let ctitle = session_title_lower(&cand);
                            if !ctitle.is_empty() && fg.contains(&ctitle) {
                                session = cand;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    if !is_playing_session(&session) {
        if let Ok(sessions) = manager.GetSessions() {
            if let Ok(count) = sessions.Size() {
                for k in 0..count {
                    if let Ok(cand) = sessions.GetAt(k) {
                        if is_playing_session(&cand) {
                            session = cand;
                            break;
                        }
                    }
                }
            }
        }
    }

    Some(session)
}

pub struct SmtcWatcher {
    running: Arc<AtomicBool>,
}

impl SmtcWatcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, ui: UiSender) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = Arc::clone(&self.running);
        let ui_pos = ui.clone();

        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 0);
            }
            let mut last_sig = String::new();
            let mut art_pending = false;
            let mut art_tries = 0u32;

            while running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1200));

                let info_opt = get_current_session_info();
                let current_sig = match &info_opt {
                    Some(v) => format!(
                        "{}_{}_{}_{}_{}",
                        v.title, v.artist, v.is_playing, v.source_app_id, v.duration_sec
                    ),
                    None => String::new(),
                };
                let has_art = info_opt
                    .as_ref()
                    .and_then(|v| v.thumbnail_bytes.as_ref())
                    .is_some();

                if current_sig != last_sig {
                    last_sig = current_sig;
                    art_pending = !has_art;
                    art_tries = 0;
                    ui.send(UiEvent::SmtcNowPlaying(info_opt));
                } else if art_pending && art_tries < 15 {
                    art_tries += 1;
                    if has_art {
                        art_pending = false;
                        if let Some(v) = info_opt {
                            ui.send(UiEvent::SmtcNowPlaying(Some(v)));
                        }
                    }
                }
            }
        });

        let running_pos = Arc::clone(&self.running);
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(std::ptr::null_mut(), 0);
            }
            while running_pos.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let Ok(op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                else {
                    continue;
                };
                let Ok(manager) = op.get() else { continue };
                if let Some((pos_ms, playing, raw_pos_ms, last_updated_ticks, dur_ms)) =
                    read_position_once(&manager)
                {
                    ui_pos.send(UiEvent::SmtcPosition {
                        pos_ms,
                        playing,
                        raw_pos_ms,
                        last_updated_ticks,
                        dur_ms,
                    });
                }
            }
        });
    }
}

pub fn get_current_session_info() -> Option<SmtcSessionInfo> {
    unsafe {
        let _ = CoInitializeEx(std::ptr::null_mut(), 0);
    }
    let manager_op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?;
    let manager = manager_op.get().ok()?;
    let session = pick_active_session(&manager)?;

    let app_id = session
        .SourceAppUserModelId()
        .map(|h| h.to_string())
        .unwrap_or_default();

    let props_op = session.TryGetMediaPropertiesAsync().ok()?;
    let props = props_op.get().ok()?;

    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
    let is_playing = is_playing_session(&session);

    let (position, duration, _, _) = compute_timeline_pos(&session, is_playing);

    if title.is_empty() && artist.is_empty() && !is_playing {
        return None;
    }

    let thumb = extract_thumbnail(&props);
    let (thumbnail_bytes, thumbnail_mime, accent_tint) = match thumb {
        Some((b, m, t)) => (Some(b), Some(m), t),
        None => (None, None, None),
    };

    Some(SmtcSessionInfo {
        source_app_id: app_id,
        title,
        artist,
        album,
        is_playing,
        position_sec: position,
        duration_sec: duration,
        thumbnail_bytes,
        thumbnail_mime,
        accent_tint,
    })
}

pub fn read_position_once(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Option<(u64, bool, u64, i64, u64)> {
    let session = pick_active_session(manager)?;
    let playing = is_playing_session(&session);
    let (pos_sec, dur_sec, raw_pos_ms, last_updated_ticks) =
        compute_timeline_pos(&session, playing);
    let pos_ms = (pos_sec.max(0.0) * 1000.0) as u64;
    let dur_ms = (dur_sec.max(0.0) * 1000.0) as u64;
    Some((pos_ms, playing, raw_pos_ms, last_updated_ticks, dur_ms))
}

pub fn try_seek(pos_secs: f64) -> bool {
    try_seek_for(pos_secs, None, None)
}

pub fn try_seek_for(
    pos_secs: f64,
    preferred_app_id: Option<&str>,
    preferred_title: Option<&str>,
) -> bool {
    let app = preferred_app_id.map(str::to_string);
    let title = preferred_title.map(str::to_string);
    std::thread::spawn(move || {
        let _com = ComApartmentGuard::init_mta();
        let Ok(manager_op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() else {
            return false;
        };
        let Ok(manager) = manager_op.get() else {
            return false;
        };
        let Some(session) = pick_session_for(&manager, app.as_deref(), title.as_deref()) else {
            return false;
        };
        let timeline = session.GetTimelineProperties().ok();
        let start_ticks = timeline
            .as_ref()
            .and_then(|t| t.StartTime().ok())
            .map(|s| s.Duration)
            .unwrap_or(0);
        let end_ticks = timeline
            .as_ref()
            .and_then(|t| t.EndTime().ok())
            .map(|e| e.Duration)
            .unwrap_or(0);
        let mut ts = start_ticks.saturating_add((pos_secs.max(0.0) * 10_000_000.0) as i64);
        if end_ticks > start_ticks {
            ts = ts.min(end_ticks);
        }
        session
            .TryChangePlaybackPositionAsync(ts)
            .ok()
            .and_then(|op| op.get().ok())
            .unwrap_or(false)
    })
    .join()
    .unwrap_or(false)
}

fn send_vk_media(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    #[link(name = "user32")]
    extern "system" {
        fn MapVirtualKeyW(u_code: u32, u_map_type: u32) -> u32;
    }
    let scan = unsafe { MapVirtualKeyW(vk as u32, 0) } as u16;
    let down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: KEYEVENTF_EXTENDEDKEY,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32) == 2 }
}

pub fn send_media_key(action: &str) -> bool {
    send_media_key_for(action, None, None)
}

pub fn send_media_key_for(
    action: &str,
    preferred_app_id: Option<&str>,
    preferred_title: Option<&str>,
) -> bool {
    let norm = action.trim().to_ascii_lowercase();
    let vk_fallback: u16 = match norm.as_str() {
        "play-pause" | "playpause" | "toggle" | "play" | "pause" => 0xB3,
        "next" => 0xB0,
        "previous" | "prev" => 0xB1,
        _ => return false,
    };

    let is_toggle = matches!(norm.as_str(), "play-pause" | "playpause" | "toggle");
    let _inflight_guard = if is_toggle {
        if PLAYPAUSE_INFLIGHT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let guard = InflightGuard(&PLAYPAUSE_INFLIGHT);
        let now_ms = now_monotonic_ms();
        let last_ms = LAST_PLAYPAUSE_MS.load(Ordering::SeqCst);
        if should_debounce_media_key(last_ms, now_ms, 380) {
            return false;
        }
        LAST_PLAYPAUSE_MS.store(now_ms, Ordering::SeqCst);
        Some(guard)
    } else {
        None
    };

    let app = preferred_app_id.map(str::to_string);
    let title = preferred_title.map(str::to_string);
    let norm_owned = norm.clone();
    let winrt_ok = std::thread::spawn(move || -> bool {
        let _com = ComApartmentGuard::init_mta();
        let Ok(manager_op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        else {
            return false;
        };
        let Ok(manager) = manager_op.get() else {
            return false;
        };
        let Some(session) = pick_session_for(&manager, app.as_deref(), title.as_deref()) else {
            return false;
        };

        let controls = session.GetPlaybackInfo().ok().and_then(|p| p.Controls().ok());
        let was_playing = is_playing_session(&session);

        match norm_owned.as_str() {
            "play-pause" | "playpause" | "toggle" => {
                let can_pause = controls
                    .as_ref()
                    .and_then(|c| c.IsPauseEnabled().ok())
                    .unwrap_or(false);
                let can_play = controls
                    .as_ref()
                    .and_then(|c| c.IsPlayEnabled().ok())
                    .unwrap_or(false);
                let can_toggle = controls
                    .as_ref()
                    .and_then(|c| c.IsPlayPauseToggleEnabled().ok())
                    .unwrap_or(true);

                // Try explicit Pause/Play first when supported; NEVER chain Toggle or VK fallback
                // once a WinRT transport call returned true, so delayed SMTC status updates
                // cannot trigger a double-toggle that immediately resumes playback.
                let explicit_accepted = if was_playing && can_pause {
                    session
                        .TryPauseAsync()
                        .ok()
                        .and_then(|op| op.get().ok())
                        .unwrap_or(false)
                } else if !was_playing && can_play {
                    session
                        .TryPlayAsync()
                        .ok()
                        .and_then(|op| op.get().ok())
                        .unwrap_or(false)
                } else {
                    false
                };

                if explicit_accepted {
                    for _ in 0..8 {
                        std::thread::sleep(Duration::from_millis(60));
                        if is_playing_session(&session) != was_playing {
                            break;
                        }
                    }
                    return true;
                }

                if can_toggle
                    && session
                        .TryTogglePlayPauseAsync()
                        .ok()
                        .and_then(|op| op.get().ok())
                        .unwrap_or(false)
                {
                    for _ in 0..8 {
                        std::thread::sleep(Duration::from_millis(60));
                        if is_playing_session(&session) != was_playing {
                            break;
                        }
                    }
                    return true;
                }

                false
            }
            "play" => session
                .TryPlayAsync()
                .ok()
                .and_then(|op| op.get().ok())
                .unwrap_or(false),
            "pause" => session
                .TryPauseAsync()
                .ok()
                .and_then(|op| op.get().ok())
                .unwrap_or(false),
            "next" => session
                .TrySkipNextAsync()
                .ok()
                .and_then(|op| op.get().ok())
                .unwrap_or(false),
            "previous" | "prev" => session
                .TrySkipPreviousAsync()
                .ok()
                .and_then(|op| op.get().ok())
                .unwrap_or(false),
            _ => false,
        }
    })
    .join()
    .unwrap_or(false);

    if winrt_ok {
        true
    } else {
        send_vk_media(vk_fallback)
    }
}

fn read_current_position_once() -> Option<(u64, bool, u64, i64, u64)> {
    let _com = ComApartmentGuard::init_mta();
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .ok()?
        .get()
        .ok()?;
    read_position_once(&manager)
}

pub fn seek_and_refresh(
    rt: &tokio::runtime::Handle,
    ui: &UiSender,
    pos_secs: f64,
    app_id: &str,
    title: &str,
) {
    let app = app_id.to_string();
    let t = title.to_string();
    let u = ui.clone();
    rt.spawn_blocking(move || {
        try_seek_for(pos_secs, Some(&app), Some(&t));
        std::thread::sleep(Duration::from_millis(200));
        u.send(UiEvent::SmtcNowPlaying(get_current_session_info()));
    });
}

pub fn media_key_and_refresh(
    rt: &tokio::runtime::Handle,
    ui: &UiSender,
    action: &str,
    app_id: &str,
    title: &str,
) {
    let act = action.to_string();
    let app = app_id.to_string();
    let t = title.to_string();
    let u = ui.clone();
    rt.spawn_blocking(move || {
        let is_toggle = matches!(
            act.trim().to_ascii_lowercase().as_str(),
            "play-pause" | "playpause" | "toggle"
        );
        let ok = send_media_key_for(&act, Some(&app), Some(&t));
        if is_toggle && !ok {
            return;
        }
        std::thread::sleep(Duration::from_millis(160));
        u.send(UiEvent::SmtcNowPlaying(get_current_session_info()));
        if let Some((pos_ms, playing, raw_pos_ms, last_updated_ticks, dur_ms)) =
            read_current_position_once()
        {
            u.send(UiEvent::SmtcPosition {
                pos_ms,
                playing,
                raw_pos_ms,
                last_updated_ticks,
                dur_ms,
            });
        }
        std::thread::sleep(Duration::from_millis(350));
        u.send(UiEvent::SmtcNowPlaying(get_current_session_info()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_key_debounce_window_blocks_rapid_double_clicks() {
        assert!(!should_debounce_media_key(0, 1_050, 380));
        assert!(should_debounce_media_key(1_000, 1_000, 380));
        assert!(should_debounce_media_key(1_000, 1_379, 380));
        assert!(!should_debounce_media_key(1_000, 1_380, 380));
        assert!(!should_debounce_media_key(1_000, 2_000, 380));
        assert!(should_debounce_media_key(2_000, 1_900, 380));
    }
}
