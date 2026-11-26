pub mod closed_strip;
pub mod companion;
pub mod d2d;
pub mod icons;
pub mod particles;
pub mod popups;
pub mod tabs;
pub mod top_bar;
pub mod widgets;
pub mod window;

use crate::backend::{approval_gate, display, lrc, privacy, system, BackendServices};
use crate::events::{
    ActiveCallInfo, AiLimitItem, ApprovalRequest, BatteryState, LiveUsage, PrivacySnapshot,
    PttAbortReason, PttMode, SmtcSessionInfo, UiEvent,
};
use chrono::Timelike as _;
use std::collections::{HashSet, VecDeque};
use crate::logic::executor;
use crate::logic::lyrics::LyricsState;
use crate::state::{
    now_unix_ms, CalendarEvent, ClipboardEntry, ConvoEntry, FileHit, FocusLogEntry, NotifEntry,
    ReminderItem, Settings, TodoItem,
};
use closed_strip::{draw_closed_strip, target_closed_width, ClosedStripState, HardwareMoment};
use companion::{CompanionRenderer, CompanionState};
use d2d::{Color, D2dEngine, Rect};
use particles::ParticleBuffers;
use popups::{draw_popup, popup_target_size, PopupState, PopupUiState};
use serde_json::{Map, Value};
use tabs::calendar::{draw_calendar, CalendarTabState};
use tabs::conversation::{draw_conversation, ConvoTabState};
use tabs::files::{draw_files, FilesSubTab, FilesTabState};
use tabs::home::{
    draw_desk, draw_desk_time_picker, draw_glance, target_glance_size, HomeTabState,
};
use tabs::music::{draw_music, target_music_size, MusicTabState};
use tabs::notifs::{draw_notifs, NotifsTabState};
use tabs::settings::{draw_settings, SettingsSection, SettingsTabState};
use top_bar::{draw_top_bar, ActiveTab};
use widgets::{draw_tooltip, HitRegistry, UiInput};
use window::{FrameDirectives, UiApp};

#[derive(Debug, Clone, Copy)]
struct Spring {
    pos: f32,
    vel: f32,
}

impl Spring {
    fn new(v: f32) -> Self {
        Self { pos: v, vel: 0.0 }
    }

    /// Critically damped responsive spring (`stiffness=440, damping=40`) sub-stepped cleanly.
    fn step(&mut self, target: f32, dt: f32) -> bool {
        let mut rem = dt.clamp(0.001, 0.05);
        while rem > 0.0 {
            let h = rem.min(0.004);
            let accel = 440.0 * (target - self.pos) - 40.0 * self.vel;
            self.vel += accel * h;
            self.pos += self.vel * h;
            rem -= h;
        }
        if (target - self.pos).abs() < 0.35 && self.vel.abs() < 1.5 {
            self.pos = target;
            self.vel = 0.0;
            false
        } else {
            true
        }
    }
}

pub struct NotchApp {
    svc: BackendServices,

    // Store slices
    settings: Settings,
    todos: Vec<TodoItem>,
    reminders: Vec<ReminderItem>,
    calendar_events: Vec<CalendarEvent>,
    clips: Vec<ClipboardEntry>,
    pins: Vec<String>,
    shelf: Vec<String>,
    search_hits: Vec<FileHit>,
    convo: Vec<ConvoEntry>,
    notifs: Vec<NotifEntry>,
    focus_log: Vec<FocusLogEntry>,
    custom_photo: String,

    // Live desktop telemetry
    media: Option<SmtcSessionInfo>,
    lyrics: LyricsState,
    privacy: PrivacySnapshot,
    battery: BatteryState,
    live_usage: LiveUsage,
    ai_limits: Vec<AiLimitItem>,
    update_available: Option<crate::backend::updater::UpdateReleaseInfo>,
    hotkey_failures: Vec<String>,
    convo_thinking: bool,
    convo_live_cmd: String,
    convo_live_running: bool,
    convo_live_ok: Option<bool>,
    voice_live: bool,
    files_drag_over: bool,
    tucked: bool,
    hardware_moment: Option<HardwareMoment>,
    active_call: Option<ActiveCallInfo>,
    call_mic_idle_since_ms: Option<u64>,
    call_mic_ever_seen: bool,
    call_ui_absent_hits: u8,
    last_call_ui_probe_ms: u64,
    call_ui_probe_inflight: bool,
    last_meeting_clip: String,
    alerted_cal_agent: HashSet<String>,

    // Shell navigation & interaction state
    active_tab: ActiveTab,
    tb_indicator_x: f32,
    tab_fade_start_ms: u64,
    prev_rendered_tab: Option<ActiveTab>,
    is_open: bool,
    pinned: bool,
    prev_open_before_approval: Option<(bool, ActiveTab)>,
    pending_approvals: VecDeque<(ApprovalRequest, u64)>,
    popup: PopupState,
    close_after_ms: Option<u64>,
    pointer_x: f32,
    pointer_y: f32,
    music_glow_alpha: f32,
    idle_since_ms: Option<u64>,
    idle_trimmed: bool,
    pending_cache_trim: bool,
    last_sec_wall_ms: u64,
    last_now_ms: u64,
    last_interaction_ms: u64,
    wake_until_ms: u64,

    // Focus Pomodoro
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,

    // Spring geometry
    sp_w: Spring,
    sp_h: Spring,
    sp_br: Spring,
    sp_ear: Spring,

    // Renderers & per-tab states
    hits: HitRegistry,
    comp_renderer: CompanionRenderer,
    comp_state: CompanionState,
    particles: ParticleBuffers,
    home_state: HomeTabState,
    files_state: FilesTabState,
    music_state: MusicTabState,
    cal_state: CalendarTabState,
    convo_state: ConvoTabState,
    notifs_state: NotifsTabState,
    settings_state: SettingsTabState,
    popup_state: PopupUiState,
}

impl NotchApp {
    pub fn new(svc: BackendServices) -> Self {
        let mut settings = svc.store.get_settings();
        if let Some(b) = svc.store.get("lyricsOnClosed").as_bool() {
            settings.lyrics_on_closed = b;
        }
        let todos = serde_json::from_value(svc.store.get("todos")).unwrap_or_default();
        let reminders = serde_json::from_value(svc.store.get("reminders")).unwrap_or_default();
        let calendar_events =
            serde_json::from_value(svc.store.get("calendarEvents")).unwrap_or_default();
        let clips = serde_json::from_value(svc.store.get("clipboardHistory")).unwrap_or_default();
        let pins = serde_json::from_value(svc.store.get("pins")).unwrap_or_default();
        let shelf = serde_json::from_value(svc.store.get("shelf")).unwrap_or_default();
        let search_hits =
            serde_json::from_value(svc.store.get("dnSearchResults")).unwrap_or_default();
        let convo = serde_json::from_value(svc.store.get("dnConvo")).unwrap_or_default();
        let notifs = serde_json::from_value(svc.store.get("dnNotifList")).unwrap_or_default();
        let focus_log = serde_json::from_value(svc.store.get("focusLog")).unwrap_or_default();
        let custom_photo = svc
            .store
            .get("photo")
            .as_str()
            .unwrap_or("")
            .to_string();

        let mut lyrics = LyricsState::default();
        lyrics.load_offsets_from_store(&svc.store);
        let focus_total = (settings.focus_minutes.max(1) as u32) * 60;
        display::set_notch_ui_scale(settings.notch_scale());
        svc.voice.set_voice_preference(&settings.tts_voice);

        Self {
            svc,
            settings,
            todos,
            reminders,
            calendar_events,
            clips,
            pins,
            shelf,
            search_hits,
            convo,
            notifs,
            focus_log,
            custom_photo,
            media: None,
            lyrics,
            privacy: PrivacySnapshot::default(),
            battery: BatteryState::default(),
            live_usage: LiveUsage::default(),
            ai_limits: Vec::new(),
            update_available: None,
            hotkey_failures: Vec::new(),
            convo_thinking: false,
            convo_live_cmd: String::new(),
            convo_live_running: false,
            convo_live_ok: None,
            voice_live: false,
            files_drag_over: false,
            tucked: false,
            hardware_moment: None,
            active_call: None,
            call_mic_idle_since_ms: None,
            call_mic_ever_seen: false,
            call_ui_absent_hits: 0,
            last_call_ui_probe_ms: 0,
            call_ui_probe_inflight: false,
            last_meeting_clip: String::new(),
            alerted_cal_agent: HashSet::new(),
            active_tab: ActiveTab::Glance,
            tb_indicator_x: 0.0,
            tab_fade_start_ms: 0,
            prev_rendered_tab: None,
            is_open: false,
            pinned: false,
            prev_open_before_approval: None,
            pending_approvals: VecDeque::new(),
            popup: PopupState::None,
            close_after_ms: None,
            pointer_x: -1000.0,
            pointer_y: -1000.0,
            music_glow_alpha: 0.0,
            idle_since_ms: None,
            idle_trimmed: false,
            pending_cache_trim: false,
            last_sec_wall_ms: 0,
            last_now_ms: 0,
            last_interaction_ms: 0,
            wake_until_ms: 0,
            focus_running: false,
            focus_remaining_sec: focus_total,
            focus_total_sec: focus_total,
            sp_w: Spring::new(240.0),
            sp_h: Spring::new(30.0),
            sp_br: Spring::new(14.0),
            sp_ear: Spring::new(8.0),
            hits: HitRegistry::new(),
            comp_renderer: CompanionRenderer::new(),
            comp_state: CompanionState::default(),
            particles: ParticleBuffers::new(),
            home_state: HomeTabState::default(),
            files_state: FilesTabState::default(),
            music_state: MusicTabState::default(),
            cal_state: CalendarTabState::default(),
            convo_state: ConvoTabState::default(),
            notifs_state: NotifsTabState::default(),
            settings_state: SettingsTabState::default(),
            popup_state: PopupUiState::default(),
        }
    }

    fn is_assistant_overlay_active(&self) -> bool {
        self.voice_live
            || matches!(
                self.popup,
                PopupState::Ask { .. } | PopupState::Voice { .. } | PopupState::Approval { .. }
            )
    }

    fn is_voice_busy(&self) -> bool {
        self.voice_live
            || matches!(self.popup, PopupState::Voice { .. })
            || self.svc.voice.is_recording()
            || self.svc.voice.is_speaking()
    }

    fn present_approval(&mut self, req: ApprovalRequest, arrived_ms: u64, now_ms: u64) {
        if self.settings.dn_extras_ai && !self.settings.dnd {
            std::thread::spawn(approval_gate::play_approval_chime);
        }
        if self.is_open && self.prev_open_before_approval.is_none() {
            self.prev_open_before_approval = Some((true, self.active_tab));
        }
        self.popup = PopupState::Approval {
            req,
            expires_at_ms: arrived_ms
                .saturating_add(115_000)
                .max(now_ms.saturating_add(10_000)),
        };
    }

    fn flush_pending_approval(&mut self, now_ms: u64) {
        if !matches!(self.popup, PopupState::None) || self.is_voice_busy() {
            return;
        }
        while let Some((req, arrived_ms)) = self.pending_approvals.pop_front() {
            if now_ms.saturating_sub(arrived_ms) >= 112_000 {
                continue;
            }
            let still_alive =
                self.svc.approval.has_pending(&req.id) || self.svc.pipe.has_pending(&req.id);
            if !still_alive {
                continue;
            }
            self.present_approval(req, arrived_ms, now_ms);
            return;
        }
    }

    fn reload_store_key(&mut self, key: &str, val: &Value) {
        match key {
            "settings" => {
                let prev_mins = self.settings.focus_minutes;
                let prev_scale_tag = self.settings.notch_size.clone();
                let prev_voice = self.settings.tts_voice.clone();
                self.settings = Settings::from_value(val);
                if let Some(b) = self.svc.store.get("lyricsOnClosed").as_bool() {
                    self.settings.lyrics_on_closed = b;
                }
                if self.settings.notch_size != prev_scale_tag {
                    display::set_notch_ui_scale(self.settings.notch_scale());
                    display::sync_notch_window(self.svc.ui.get_hwnd(), &self.svc.store);
                }
                if self.settings.tts_voice != prev_voice {
                    self.svc.voice.set_voice_preference(&self.settings.tts_voice);
                }
                self.svc.voice.hotkeys.reload_bindings(
                    crate::voice::hotkey::HotkeyBindings::from_settings(&self.settings),
                );
                if !self.focus_running && self.settings.focus_minutes != prev_mins {
                    let next_tot = (self.settings.focus_minutes.max(1) as u32) * 60;
                    self.focus_total_sec = next_tot;
                    self.focus_remaining_sec = next_tot;
                }
            }
            "lyricsOnClosed" => {
                if let Some(b) = val.as_bool() {
                    self.settings.lyrics_on_closed = b;
                }
            }
            "todos" => self.todos = serde_json::from_value(val.clone()).unwrap_or_default(),
            "reminders" => {
                self.reminders = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "calendarEvents" => {
                self.calendar_events = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "clipboardHistory" => {
                self.clips = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "pins" => self.pins = serde_json::from_value(val.clone()).unwrap_or_default(),
            "shelf" => self.shelf = serde_json::from_value(val.clone()).unwrap_or_default(),
            "dnSearchResults" => {
                self.search_hits = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "dnConvo" => self.convo = serde_json::from_value(val.clone()).unwrap_or_default(),
            "dnNotifList" => {
                self.notifs = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "focusLog" => {
                self.focus_log = serde_json::from_value(val.clone()).unwrap_or_default()
            }
            "photo" => {
                self.custom_photo = val.as_str().unwrap_or("").to_string();
            }
            "lrcOffsets" => {
                self.lyrics.load_offsets_from_store(&self.svc.store);
            }
            _ => {}
        }
    }

    fn tab_target_size(&self, d2d: &mut D2dEngine) -> (f32, f32) {
        if self.popup.is_active() {
            return popup_target_size(&self.popup, &self.popup_state, d2d);
        }
        match self.active_tab {
            ActiveTab::Glance => {
                target_glance_size(
                    &self.settings,
                    self.media.as_ref(),
                    &self.lyrics,
                    &self.ai_limits,
                    self.focus_running,
                    self.focus_remaining_sec,
                    self.focus_total_sec,
                )
            }
            ActiveTab::Desk => (720.0, 188.0),
            ActiveTab::Files => (560.0, 190.0),
            ActiveTab::Music => target_music_size(
                d2d,
                self.media.as_ref(),
                &self.lyrics,
                &self.music_state,
            ),
            ActiveTab::Calendar => (520.0, 250.0),
            ActiveTab::Conversation => (560.0, 292.0),
            ActiveTab::Notifs => (430.0, 260.0),
            ActiveTab::Settings => (644.0, 370.0),
        }
    }

    fn tick_1s(&mut self, now_ms: u64) {
        if let Some(m) = self.media.as_ref() {
            self.lyrics.update_active_index(&m.artist, &m.title);
        }

        if self.focus_running {
            if self.focus_remaining_sec > 1 {
                self.focus_remaining_sec -= 1;
            } else {
                self.focus_running = false;
                let mins = (self.focus_total_sec / 60).max(1) as i32;
                self.focus_remaining_sec = self.focus_total_sec;
                let mut log = self.focus_log.clone();
                log.push(FocusLogEntry {
                    at: now_unix_ms(),
                    minutes: mins,
                });
                if let Ok(v) = serde_json::to_value(&log) {
                    self.svc.store.set(Some(&self.svc.ui), "focusLog", v);
                }
                if !self.settings.dnd && !self.is_assistant_overlay_active() {
                    std::thread::spawn(approval_gate::play_done_chime);
                    self.popup = PopupState::Done {
                        duration_sec: self.focus_total_sec,
                        expires_at_ms: now_ms + 6500,
                    };
                }
            }
        }

        // Check due reminders (deferred while Ask/Voice/Approval overlay is active
        // so incoming alerts do not erase typed drafts or interrupt voice).
        let now_epoch = now_unix_ms();
        if !self.is_assistant_overlay_active() {
            if let Some(idx) = self.reminders.iter().position(|r| r.at > 0 && r.at <= now_epoch) {
                let mut next = self.reminders.clone();
                let due = next.remove(idx);
                if let Ok(v) = serde_json::to_value(next) {
                    self.svc.store.set(Some(&self.svc.ui), "reminders", v);
                }
                if !self.settings.dnd {
                    std::thread::spawn(approval_gate::play_reminder_chime);
                }
                // A reminder mirrored from a calendar event with a meeting link
                // becomes a Join banner, not a plain reminder. The link may sit
                // in the message, the event title, or the event note.
                let cal_link = self
                    .calendar_events
                    .iter()
                    .find(|ev| ev.id == due.id)
                    .and_then(|ev| {
                        system::detect_meeting_link(&ev.note)
                            .or_else(|| system::detect_meeting_link(&ev.title))
                    });
                let meeting = system::detect_meeting_link(&due.message).or(cal_link);
                if let Some(link) = meeting {
                    self.popup = PopupState::Meeting {
                        title: due.message,
                        subtitle: "Waktunya rapat — ketuk Join untuk masuk".to_string(),
                        link,
                        expires_at_ms: now_ms + 60_000,
                    };
                } else {
                    self.popup = PopupState::Reminder {
                        message: due.message,
                        at_ms: if due.at > 0 { due.at } else { now_epoch },
                        start_ms: now_ms,
                        expires_at_ms: now_ms + 60_000,
                    };
                }
            }
        }

        // Auto-end active call. Primary signals are `UiEvent::ActiveCallEnded`
        // (incoming ring gone) and live browser UI / CoreAudio mic checks:
        // - hard cap: 3 hours
        // - process closed (`!is_call_process_running`) -> end immediately
        // - non-browser window closed (`!call_window_alive`) after 4s grace
        // - browser call UI ended (`check_browser_call_ui_active == Some(false)` x2)
        // - CoreAudio live mic idle for >= 2s once mic was acquired, or never
        //   acquired after 10s
        if let Some(call) = &self.active_call {
            let age_ms = now_ms.saturating_sub(call.started_at_ms);
            let is_browser_call = system::is_browser_raw_app(&call.raw_app);
            let hard_cap = age_ms > 3 * 60 * 60 * 1000;
            let proc_gone = age_ms > 2_000
                && !system::is_call_process_running(&call.raw_app, &call.app);
            let window_gone = !is_browser_call
                && age_ms > 4_000
                && call.call_hwnd != 0
                && !system::call_window_alive(call.call_hwnd);
            let mic_live = privacy::is_call_mic_live(&self.privacy, &call.raw_app, &call.app);
            if mic_live {
                self.call_mic_ever_seen = true;
                self.call_mic_idle_since_ms = None;
            } else if self.call_mic_ever_seen && self.call_mic_idle_since_ms.is_none() {
                self.call_mic_idle_since_ms = Some(now_ms);
            }
            let mic_idle = self.call_mic_ever_seen
                && self
                    .call_mic_idle_since_ms
                    .map_or(false, |t| now_ms.saturating_sub(t) >= 2_000);
            let never_acquired = !self.call_mic_ever_seen && age_ms > 10_000;

            if hard_cap || proc_gone || window_gone || mic_idle || never_acquired {
                let reason = if hard_cap {
                    "hard-cap"
                } else if proc_gone {
                    "process-closed"
                } else if window_gone {
                    "window-closed"
                } else if mic_idle {
                    "mic-idle"
                } else {
                    "mic-never-acquired"
                };
                crate::backend::logfile::append(&format!(
                    "[call] ended app='{}' reason={}",
                    call.app, reason
                ));
                self.active_call = None;
                self.call_mic_idle_since_ms = None;
                self.call_mic_ever_seen = false;
                self.call_ui_absent_hits = 0;
                self.call_ui_probe_inflight = false;
            } else if is_browser_call
                && call.call_hwnd != 0
                && age_ms >= 8_000
                && !self.call_ui_probe_inflight
                && now_ms.saturating_sub(self.last_call_ui_probe_ms) >= 2_500
            {
                self.call_ui_probe_inflight = true;
                self.last_call_ui_probe_ms = now_ms;
                let started = call.started_at_ms;
                let ch = call.call_hwnd;
                let ca = call.app.clone();
                let ui_tx = self.svc.ui.clone();
                self.svc.rt.spawn(async move {
                    let still_active = tokio::task::spawn_blocking(move || {
                        system::check_browser_call_ui_active(ch, &ca)
                    })
                    .await
                    .ok()
                    .flatten();
                    ui_tx.send(UiEvent::CallUiProbeResult {
                        call_started_ms: started,
                        still_active,
                    });
                });
            }
        } else {
            self.call_mic_idle_since_ms = None;
            self.call_mic_ever_seen = false;
            self.call_ui_absent_hits = 0;
            self.call_ui_probe_inflight = false;
        }

        // Pure AI Agent proactive calendar alert at 10 minutes and 5 minutes before event
        if !self.is_assistant_overlay_active()
            && !matches!(&self.popup, PopupState::Notif { payload, .. } if payload.is_call)
        {
            for ev in &self.calendar_events {
                let Some(ev_at_ms) = calendar_event_epoch_ms(ev) else {
                    continue;
                };
                let ev_key = if ev.id.trim().is_empty() {
                    format!("{}_{}", ev.date, ev.time)
                } else {
                    ev.id.clone()
                };
                let Some(mins) = due_calendar_agent_milestone(
                    ev_at_ms,
                    now_epoch,
                    &self.alerted_cal_agent,
                    &ev_key,
                ) else {
                    continue;
                };
                self.alerted_cal_agent.insert(format!("{}:10", ev_key));
                if mins == 5 {
                    self.alerted_cal_agent.insert(format!("{}:5", ev_key));
                }
                if !self.settings.dnd {
                    std::thread::spawn(approval_gate::play_agent_chime);
                    let link = system::detect_meeting_link(&ev.note)
                        .or_else(|| system::detect_meeting_link(&ev.title));
                    let t_lbl = if ev.time.trim().is_empty() {
                        chrono::DateTime::from_timestamp_millis(ev_at_ms)
                            .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
                            .unwrap_or_default()
                    } else {
                        ev.time.clone()
                    };
                    self.popup = PopupState::AgentCalendarAlert {
                        event_id: ev_key,
                        title: ev.title.clone(),
                        time_label: t_lbl,
                        minutes_before: mins,
                        note: ev.note.clone(),
                        meeting_link: link,
                        start_ms: now_ms,
                        expires_at_ms: now_ms + 22_000,
                    };
                }
                break;
            }
        }
    }
}

pub(crate) fn calendar_event_epoch_ms(ev: &CalendarEvent) -> Option<i64> {
    if ev.at > 0 {
        return Some(ev.at);
    }
    if ev.date.trim().is_empty() || ev.time.trim().is_empty() {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(ev.date.trim(), "%Y-%m-%d").ok()?;
    let clean_t = ev.time.trim().replace('.', ":");
    let (h_s, m_s) = clean_t.split_once(':')?;
    let h: u32 = h_s.parse().ok()?;
    let m: u32 = m_s.parse().ok()?;
    let ndt = date.and_hms_opt(h, m, 0)?;
    chrono::TimeZone::from_local_datetime(&chrono::Local, &ndt)
        .single()
        .map(|dt| dt.timestamp_millis())
}

pub(crate) fn due_calendar_agent_milestone(
    ev_at_ms: i64,
    now_epoch_ms: i64,
    alerted: &HashSet<String>,
    ev_id: &str,
) -> Option<u32> {
    let diff = ev_at_ms - now_epoch_ms;
    if diff <= 0 || diff > 10 * 60_000 {
        return None;
    }
    if diff <= 5 * 60_000 {
        let key5 = format!("{}:5", ev_id);
        if !alerted.contains(&key5) {
            return Some(5);
        }
        return None;
    }
    let key10 = format!("{}:10", ev_id);
    if !alerted.contains(&key10) {
        Some(10)
    } else {
        None
    }
}

impl UiApp for NotchApp {
    fn on_event(&mut self, ev: UiEvent) {
        let now_ms = self.last_now_ms;
        match ev {
            UiEvent::StoreChanged { key, value } => self.reload_store_key(&key, &value),

            UiEvent::CursorMoved { x, y } => {
                self.pointer_x = x as f32;
                self.pointer_y = y as f32;
            }

            UiEvent::NotchTucked(tucked) => {
                self.tucked = tucked;
                if tucked {
                    self.is_open = false;
                }
            }

            UiEvent::PrivacyChanged(snap) => {
                let cam_on = snap.camera && !self.privacy.camera;
                let mic_on = snap.mic && !self.privacy.mic;
                self.privacy = snap.clone();
                if let Some(call) = &self.active_call {
                    if privacy::is_call_mic_live(&self.privacy, &call.raw_app, &call.app) {
                        self.call_mic_ever_seen = true;
                        self.call_mic_idle_since_ms = None;
                    } else if self.call_mic_ever_seen && self.call_mic_idle_since_ms.is_none() {
                        self.call_mic_idle_since_ms = Some(now_ms);
                    }
                }
                if cam_on {
                    let app = snap.camera_apps.first().cloned().unwrap_or_else(|| "Kamera".to_string());
                    self.hardware_moment = Some(HardwareMoment {
                        kind: "cam".to_string(),
                        name: app,
                        detail: "Kamera aktif".to_string(),
                        accent: Color::rgb(48, 209, 88),
                        expires_at_ms: now_ms + 1600,
                    });
                } else if mic_on {
                    let app = snap.mic_apps.first().cloned().unwrap_or_else(|| "Mikrofon".to_string());
                    self.hardware_moment = Some(HardwareMoment {
                        kind: "mic".to_string(),
                        name: app,
                        detail: "Mic aktif".to_string(),
                        accent: Color::rgb(255, 159, 10),
                        expires_at_ms: now_ms + 1600,
                    });
                }
            }

            UiEvent::FilesDragEnter => {
                self.files_drag_over = true;
                self.is_open = true;
                self.active_tab = ActiveTab::Files;
                self.files_state.sub = FilesSubTab::Files;
                self.close_after_ms = None;
            }
            UiEvent::FilesDragLeave => {
                self.files_drag_over = false;
            }
            UiEvent::ShelfDropped(paths) => {
                self.files_drag_over = false;
                let mut next = self.shelf.clone();
                for p in paths.into_iter().rev() {
                    if !p.is_empty() && !next.contains(&p) {
                        next.insert(0, p);
                    }
                }
                if next.len() > 40 {
                    next.truncate(40);
                }
                if let Ok(v) = serde_json::to_value(next) {
                    self.svc.store.set(Some(&self.svc.ui), "shelf", v);
                }
                self.active_tab = ActiveTab::Files;
                self.files_state.sub = FilesSubTab::Files;
                self.is_open = true;
                self.close_after_ms = Some(now_ms + 3000);
            }

            UiEvent::ClipboardNew {
                text,
                image_bmp_data_url,
                kind,
                at_ms,
            } => {
                let detected_meeting = if image_bmp_data_url.is_empty() && !text.is_empty() {
                    system::detect_meeting_link(&text)
                } else {
                    None
                };
                let mut next = self.clips.clone();
                let existing_pos = next.iter().position(|c| {
                    if !image_bmp_data_url.is_empty() {
                        c.image == image_bmp_data_url
                    } else {
                        c.image.is_empty() && !text.is_empty() && c.text == text
                    }
                });
                match existing_pos {
                    Some(0) => {}
                    Some(idx) => {
                        let mut item = next.remove(idx);
                        item.at = at_ms as i64;
                        next.insert(0, item);
                        if let Ok(v) = serde_json::to_value(next) {
                            self.svc.store.set(Some(&self.svc.ui), "clipboardHistory", v);
                        }
                    }
                    None => {
                        next.insert(
                            0,
                            ClipboardEntry {
                                text,
                                image: image_bmp_data_url,
                                kind,
                                at: at_ms as i64,
                                read: false,
                            },
                        );
                        if next.len() > 40 {
                            next.truncate(40);
                        }
                        if let Ok(v) = serde_json::to_value(next) {
                            self.svc.store.set(Some(&self.svc.ui), "clipboardHistory", v);
                        }
                        if !self.settings.dnd && !self.is_assistant_overlay_active() {
                            if let Some(link) = detected_meeting {
                                if link.url != self.last_meeting_clip {
                                    self.last_meeting_clip = link.url.clone();
                                    self.popup = PopupState::Meeting {
                                        title: link.provider.label().to_string(),
                                        subtitle: "Ketuk Join untuk masuk rapat".to_string(),
                                        link,
                                        expires_at_ms: now_ms + 18_000,
                                    };
                                }
                            }
                        }
                    }
                }
            }

            UiEvent::NotifArrived(mut payload) => {
                if payload.is_call && payload.body.trim().is_empty() {
                    payload.body = format!("Panggilan masuk • {}", payload.app);
                }
                let profile = crate::backend::notif::parse_profile(&payload.launch);
                let mut next = self.notifs.clone();
                next.insert(
                    0,
                    NotifEntry {
                        app: payload.app.clone(),
                        raw_app: payload.raw_app.clone(),
                        title: payload.title.clone(),
                        body: payload.body.clone(),
                        launch: payload.launch.clone(),
                        profile,
                        site: payload.site.clone(),
                        chat: payload.chat.clone(),
                        jid: payload.jid.clone(),
                        logo: payload.logo.clone(),
                        at: now_unix_ms(),
                        read: false,
                        actions: payload.actions.clone(),
                        inputs: payload.inputs.clone(),
                    },
                );
                if next.len() > 50 {
                    next.truncate(50);
                }
                if let Ok(v) = serde_json::to_value(next) {
                    self.svc.store.set(Some(&self.svc.ui), "dnNotifList", v);
                }
                if !self.settings.dnd && !self.is_assistant_overlay_active() {
                    self.popup = PopupState::Notif {
                        payload,
                        replying: false,
                        expires_at_ms: now_ms + 12_000,
                    };
                }
            }

            UiEvent::ScreenshotNew(path) => {
                if self.settings.catch_screenshots {
                    let mut next = self.shelf.clone();
                    if !next.contains(&path) {
                        next.insert(0, path.clone());
                        if next.len() > 40 {
                            next.truncate(40);
                        }
                        if let Ok(v) = serde_json::to_value(next) {
                            self.svc.store.set(Some(&self.svc.ui), "shelf", v);
                        }
                    }
                    if !self.is_assistant_overlay_active() {
                        self.popup = PopupState::Capture {
                            path,
                            renaming: false,
                            flash_start_ms: now_ms,
                            status_msg: String::new(),
                            expires_at_ms: now_ms + 15_000,
                        };
                    }
                }
            }

            UiEvent::SmtcNowPlaying(info) => {
                self.media = info.clone();
                if let Some((key, title, artist, dur)) =
                    self.lyrics.on_smtc_session(info.as_ref())
                {
                    let u = self.svc.ui.clone();
                    self.svc.rt.spawn(async move {
                        let res = lrc::fetch_lyrics(key, &title, &artist, dur).await;
                        u.send(UiEvent::LrcLoaded(res));
                    });
                }
            }

            UiEvent::SmtcPosition {
                pos_ms,
                playing,
                raw_pos_ms,
                last_updated_ticks,
                dur_ms,
            } => {
                self.lyrics.on_position_tick(
                    pos_ms,
                    playing,
                    raw_pos_ms,
                    last_updated_ticks,
                    dur_ms,
                );
                if let Some(m) = self.media.as_ref() {
                    self.lyrics.update_active_index(&m.artist, &m.title);
                }
            }

            UiEvent::LrcLoaded(res) => {
                self.lyrics.on_lrc_loaded(res);
                if let Some(m) = self.media.as_ref() {
                    self.lyrics.update_active_index(&m.artist, &m.title);
                }
            }
            UiEvent::AudioOutputsUpdated(snap) => {
                self.music_state.audio_snapshot = snap;
            }

            UiEvent::AiApprovalRequested(req) => {
                self.last_interaction_ms = now_ms;
                let is_internal = self.svc.approval.has_pending(&req.id);
                let defer = should_defer_approval(
                    matches!(self.popup, PopupState::Approval { .. }),
                    is_internal,
                    self.is_voice_busy(),
                    matches!(self.popup, PopupState::Ask { .. }),
                );
                if defer {
                    self.pending_approvals.retain(|(r, _)| r.id != req.id);
                    self.pending_approvals.push_back((req, now_ms));
                } else {
                    self.present_approval(req, now_ms, now_ms);
                }
            }
            UiEvent::AiApprovalDismissed { id } => {
                self.pending_approvals.retain(|(r, _)| r.id != id);
                match &mut self.popup {
                    PopupState::Approval { req, .. } if req.id == id => {
                        self.popup = PopupState::None;
                        self.flush_pending_approval(now_ms);
                        if matches!(self.popup, PopupState::None) {
                            if let Some((open, tab)) = self.prev_open_before_approval.take() {
                                self.is_open = open;
                                self.active_tab = tab;
                            }
                        }
                    }
                    PopupState::Ask {
                        inline_approval, ..
                    } if inline_approval.as_ref().is_some_and(|r| r.id == id) => {
                        *inline_approval = None;
                    }
                    _ => {}
                }
            }

            UiEvent::HotkeyPttStart { mode } => {
                self.last_interaction_ms = now_ms;
                self.svc.voice.cancel_all_voice();
                if !crate::voice::whisper::is_available() {
                    self.voice_live = false;
                    self.popup = PopupState::WhisperSetup {
                        expires_at_ms: now_ms + 15_000,
                    };
                    return;
                }
                let svc = self.svc.clone();
                let is_live = matches!(mode, PttMode::Command);
                self.voice_live = is_live;
                executor::start_voice_session(svc, mode, is_live);
                self.popup = PopupState::Voice {
                    status: "listening".to_string(),
                    text: String::new(),
                    transcript: String::new(),
                    rms: [0.0; 5],
                    steps: Vec::new(),
                    live: is_live,
                    expires_at_ms: now_ms + 90_000,
                };
            }
            UiEvent::HotkeyPttAbort { mode } => {
                if matches!(mode, PttAbortReason::User) {
                    if self.settings_state.recording_hotkey.is_some() {
                        self.settings_state.recording_hotkey = None;
                        self.settings_state.hotkey_err.clear();
                        self.hits.focused_input = None;
                        self.svc.voice.hotkeys.set_paused(false);
                        return;
                    }
                    self.voice_live = false;
                    self.svc.voice.cancel_all_voice();
                    self.svc.voice.release_idle_memory();
                    self.pending_cache_trim = true;
                    if let PopupState::Approval { req, .. } = &self.popup {
                        if !self.svc.approval.respond(&req.id, "deny") {
                            self.svc.pipe.respond(&req.id, "dismiss");
                        }
                    }
                    if let PopupState::Ask {
                        inline_approval,
                        ..
                    } = &mut self.popup
                    {
                        if let Some(req) = inline_approval.take() {
                            if !self.svc.approval.respond(&req.id, "deny") {
                                self.svc.pipe.respond(&req.id, "deny");
                            }
                            self.svc.ask_ctrl.set_pending_approval(None);
                        } else {
                            self.svc.ask_ctrl.cancel_active(&self.svc.approval, &self.svc.pipe);
                            self.popup_state.next_run_id += 1;
                            self.popup = PopupState::None;
                            self.hits.focused_input = None;
                        }
                    } else if self.popup.is_active() {
                        self.popup = PopupState::None;
                        self.hits.focused_input = None;
                    } else if self.is_open {
                        self.is_open = false;
                        self.pinned = false;
                    }
                    self.flush_pending_approval(now_ms);
                    if matches!(self.popup, PopupState::None) {
                        if let Some((open, tab)) = self.prev_open_before_approval.take() {
                            self.is_open = open;
                            self.active_tab = tab;
                        }
                    }
                    std::thread::spawn(system::trim_process_working_set);
                } else {
                    self.svc.voice.abort_recording();
                }
            }
            UiEvent::HotkeyVoiceStop => {
                self.voice_live = false;
                self.svc.voice.cancel_all_voice();
                self.svc.voice.release_idle_memory();
                self.pending_cache_trim = true;
                std::thread::spawn(system::trim_process_working_set);
                if let PopupState::Voice { .. } = self.popup {
                    self.popup = PopupState::None;
                }
                self.flush_pending_approval(now_ms);
            }
            UiEvent::HotkeyOpenChat => {
                self.last_interaction_ms = now_ms;
                self.popup = PopupState::None;
                self.is_open = true;
                self.active_tab = ActiveTab::Conversation;
                self.convo_state.input.multiline = true;
                self.convo_state.input.rich_paste = true;
                self.hits.focused_input = Some("input:convo_inp".to_string());
                self.close_after_ms = None;
            }
            UiEvent::HotkeyOpenAsk => {
                self.last_interaction_ms = now_ms;
                self.svc.ask_ctrl.cancel_active(&self.svc.approval, &self.svc.pipe);
                self.popup_state.ask_input.clear();
                self.popup_state.ask_input.multiline = true;
                self.popup_state.ask_input.rich_paste = true;
                self.popup_state.ask_attachments.clear();
                self.popup_state.ask_scroll.offset = 0.0;
                self.hits.focused_input = Some("input:popup_ask_inp".to_string());
                self.popup = PopupState::Ask {
                    run_id: 0,
                    query: String::new(),
                    loading: false,
                    ok: true,
                    answer: String::new(),
                    proposal: None,
                    proposal_saved: false,
                    harness_cmd: String::new(),
                    harness_running: false,
                    harness_ok: None,
                    inline_approval: None,
                };
            }
            UiEvent::HotkeyRegistrationFailures(list) => {
                self.hotkey_failures = list;
            }

            UiEvent::VoiceState { gen, status, text, steps } => {
                if gen != 0 && gen != self.svc.voice.current_generation() {
                    return;
                }
                let (prev_rms, prev_transcript) = if let PopupState::Voice {
                    rms,
                    transcript,
                    ..
                } = &self.popup
                {
                    (*rms, transcript.clone())
                } else {
                    ([0.0; 5], String::new())
                };
                if status == "idle" {
                    self.svc.voice.release_idle_memory();
                    self.pending_cache_trim = true;
                    std::thread::spawn(system::trim_process_working_set);
                    if let PopupState::Voice { .. } = self.popup {
                        self.popup = PopupState::None;
                    }
                    self.flush_pending_approval(now_ms);
                } else {
                    let is_listen_or_transcribe = status.eq_ignore_ascii_case("listening")
                        || status.eq_ignore_ascii_case("Mendengarkan")
                        || status.eq_ignore_ascii_case("transcribing")
                        || status.eq_ignore_ascii_case("Mentranskripsi");
                    let is_thinking =
                        status.eq_ignore_ascii_case("thinking") || status.eq_ignore_ascii_case("Menjawab");
                    let is_terminal = status.eq_ignore_ascii_case("done")
                        || status.eq_ignore_ascii_case("Selesai")
                        || status.eq_ignore_ascii_case("error")
                        || status.eq_ignore_ascii_case("Gagal");

                    let next_transcript = if is_listen_or_transcribe {
                        String::new()
                    } else if is_thinking && steps.is_none() && !text.trim().is_empty() {
                        text.clone()
                    } else {
                        prev_transcript
                    };
                    let done_ttl = executor::voice_done_display_ms(
                        &text,
                        self.settings.dn_voice_enabled,
                    );
                    self.popup = PopupState::Voice {
                        status: status.clone(),
                        text,
                        transcript: next_transcript,
                        rms: prev_rms,
                        steps: steps.unwrap_or_default(),
                        live: self.voice_live,
                        expires_at_ms: now_ms
                            + if is_terminal {
                                if self.voice_live {
                                    done_ttl + 2_000
                                } else {
                                    done_ttl
                                }
                            } else if self.voice_live {
                                90_000
                            } else if is_listen_or_transcribe {
                                120_000
                            } else {
                                45_000
                            },
                    };
                }
            }
            UiEvent::VoiceRms(bars) => {
                if let PopupState::Voice { rms, live, .. } = &mut self.popup {
                    *rms = bars;
                    *live = self.voice_live;
                }
            }
            UiEvent::VoiceLiveActive(live) => {
                self.voice_live = live;
                if let PopupState::Voice { live: l, .. } = &mut self.popup {
                    *l = live;
                }
                if !live {
                    self.svc.voice.release_idle_memory();
                    self.pending_cache_trim = true;
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        system::trim_process_working_set();
                    });
                }
            }
            UiEvent::VoiceMicTestResult { ok: _, text } => {
                self.settings_state.mic_testing = false;
                self.settings_state.mic_test_msg = text;
            }
            UiEvent::WhisperDownloadPrompt => {
                self.voice_live = false;
                self.popup = PopupState::WhisperSetup {
                    expires_at_ms: now_ms + 15_000,
                };
            }
            UiEvent::WhisperDownloadProgress {
                stage,
                downloaded_mb,
                total_mb,
                pct,
            } => {
                self.settings_state.whisper_dl_active = true;
                self.settings_state.whisper_dl_stage = stage;
                self.settings_state.whisper_dl_mb = downloaded_mb;
                self.settings_state.whisper_dl_total_mb = total_mb;
                self.settings_state.whisper_dl_pct = pct;
                self.settings_state.whisper_dl_msg.clear();
            }
            UiEvent::WhisperDownloadDone { ok, msg } => {
                self.settings_state.whisper_dl_active = false;
                self.settings_state.whisper_dl_stage.clear();
                self.settings_state.whisper_dl_pct = if ok { 1.0 } else { 0.0 };
                self.settings_state.whisper_dl_msg = msg;
                if ok {
                    let mut m = Map::new();
                    m.insert("dnSttEnabled".to_string(), Value::Bool(true));
                    self.svc.store.patch_settings(Some(&self.svc.ui), m);
                }
            }

            UiEvent::AskResponse {
                run_id,
                ok,
                text,
                proposal,
            } => {
                if let PopupState::Ask {
                    run_id: cur_id,
                    loading,
                    ok: cur_ok,
                    answer,
                    proposal: cur_prop,
                    ..
                } = &mut self.popup
                {
                    if *cur_id == run_id {
                        *loading = false;
                        *cur_ok = ok;
                        *answer = text;
                        *cur_prop = proposal;
                    }
                }
            }

            UiEvent::ConvoThinking(t) => {
                self.convo_thinking = t;
                if t || !self.convo_live_running {
                    self.convo_live_cmd.clear();
                    self.convo_live_running = false;
                    self.convo_live_ok = None;
                }
            }
            UiEvent::ConvoHarnessStep { cmd, running, ok } => {
                self.convo_live_cmd = cmd;
                self.convo_live_running = running;
                self.convo_live_ok = ok;
            }
            UiEvent::NetworkChanged { connected } => {
                if !connected && !self.settings.dnd && !self.is_assistant_overlay_active() {
                    self.popup = PopupState::Offline {
                        expires_at_ms: now_ms + 10_000,
                    };
                } else if connected && matches!(self.popup, PopupState::Offline { .. }) {
                    // Back online: retire the offline banner right away.
                    self.popup = PopupState::None;
                }
            }

            UiEvent::ConvoAiDoneNotification { title, body } => {
                if !self.settings.dnd && !self.is_assistant_overlay_active() {
                    self.popup = PopupState::Notif {
                        payload: crate::events::NotifPayload {
                            app: "SelaBar AI".to_string(),
                            title,
                            body,
                            ..Default::default()
                        },
                        replying: false,
                        expires_at_ms: now_ms + 8000,
                    };
                }
            }

            UiEvent::RouterModelsResult { ok: _, models, msg } => {
                self.settings_state.router_models = models;
                self.settings_state.router_msg = msg;
            }
            UiEvent::UpdateCheckDone { manual, outcome } => {
                self.settings_state.update_checking = false;
                match outcome {
                    crate::backend::updater::UpdateCheckOutcome::UpdateAvailable(info) => {
                        crate::backend::logfile::append(&format!(
                            "[updater] update available: v{} ({})",
                            info.version, info.asset_name
                        ));
                        self.settings_state.update_status =
                            format!("Versi baru v{} tersedia di GitHub.", info.version);
                        self.settings_state.update_info = Some(info.clone());
                        self.update_available = Some(info);
                    }
                    crate::backend::updater::UpdateCheckOutcome::UpToDate { current, latest } => {
                        self.update_available = None;
                        self.settings_state.update_info = None;
                        if manual {
                            self.settings_state.update_status =
                                format!("Sudah versi terbaru (v{}, rilis v{}).", current, latest);
                        } else {
                            self.settings_state.update_status.clear();
                        }
                    }
                    crate::backend::updater::UpdateCheckOutcome::Offline => {
                        if manual {
                            self.settings_state.update_status =
                                "Tidak ada koneksi internet untuk mengecek update.".to_string();
                        }
                    }
                    crate::backend::updater::UpdateCheckOutcome::Error(err) => {
                        crate::backend::logfile::append(&format!("[updater] check failed: {}", err));
                        if manual {
                            self.settings_state.update_status = err;
                        }
                    }
                }
            }
            UiEvent::UpdateDownloadDone { ok, msg } => {
                self.settings_state.update_downloading = false;
                self.settings_state.update_status = msg;
                if ok {
                    crate::backend::logfile::append("[updater] installer spawned cleanly");
                }
            }

            UiEvent::BatteryUpdated(b) => self.battery = b,
            UiEvent::LiveUsageUpdated(u) => self.live_usage = u,
            UiEvent::PhotoPicked(val) => {
                self.custom_photo = val.clone();
                self.svc.store.set(Some(&self.svc.ui), "photo", Value::String(val));
                let mut m = Map::new();
                m.insert("avatar".to_string(), Value::String("photo".to_string()));
                self.svc.store.patch_settings(Some(&self.svc.ui), m);
            }
            UiEvent::CaptureStatus(msg) => {
                if let PopupState::Capture { status_msg, .. } = &mut self.popup {
                    *status_msg = msg;
                }
            }
            UiEvent::SetClickThrough(_) => {}
            UiEvent::HotkeyFinishPtt => {
                if self.svc.voice.is_recording() {
                    self.svc.voice.finish_recording();
                } else if self.voice_live && self.svc.voice.is_speaking() {
                    self.svc.voice.stop_speech();
                    executor::start_voice_session(self.svc.clone(), PttMode::Command, true);
                }
            }
            UiEvent::AskHarnessStep {
                run_id,
                step_label,
                cmd_display,
                done,
                ok,
            } => {
                if let PopupState::Ask {
                    run_id: cur_id,
                    harness_cmd,
                    harness_running,
                    harness_ok,
                    ..
                } = &mut self.popup
                {
                    if *cur_id == run_id {
                        *harness_cmd = cmd_display.unwrap_or(step_label);
                        *harness_running = !done;
                        *harness_ok = if done { Some(ok) } else { None };
                    }
                }
            }
            UiEvent::AskInlineApproval { run_id, req } => {
                if let PopupState::Ask {
                    run_id: cur_id,
                    inline_approval,
                    ..
                } = &mut self.popup
                {
                    if *cur_id == run_id {
                        *inline_approval = Some(req);
                    }
                }
            }
            UiEvent::IncomingCallDetected(mut payload) => {
                if !self.settings.dnd
                    && self.active_call.is_none()
                    && !self.is_assistant_overlay_active()
                    && !matches!(&self.popup, PopupState::Notif { payload: p, .. } if p.is_call)
                {
                    payload.is_call = true;
                    if payload.body.trim().is_empty() {
                        payload.body = format!("Panggilan masuk • {}", payload.app);
                    }
                    self.popup = PopupState::Notif {
                        payload,
                        replying: false,
                        expires_at_ms: now_ms + 12_000,
                    };
                }
            }
            UiEvent::NotifRemoved(id) => {
                if let PopupState::Notif {
                    payload,
                    replying: false,
                    ..
                } = &self.popup
                {
                    if payload.id == id {
                        self.popup = PopupState::None;
                    }
                }
            }
            UiEvent::IncomingCallDismissed => {
                if let PopupState::Notif {
                    payload,
                    replying: false,
                    ..
                } = &self.popup
                {
                    if payload.is_call {
                        self.popup = PopupState::None;
                    }
                }
            }
            UiEvent::ActiveCallStarted(mut call) => {
                call.started_at_ms = now_ms;
                self.call_mic_idle_since_ms = None;
                self.call_mic_ever_seen = false;
                self.call_ui_absent_hits = 0;
                self.last_call_ui_probe_ms = now_ms;
                self.call_ui_probe_inflight = false;
                crate::backend::logfile::append(&format!(
                    "[call] active call started app='{}' caller='{}' hwnd={}",
                    call.app, call.caller, call.call_hwnd
                ));
                self.active_call = Some(call);
            }
            UiEvent::CallUiProbeResult {
                call_started_ms,
                still_active,
            } => {
                self.call_ui_probe_inflight = false;
                if self
                    .active_call
                    .as_ref()
                    .is_some_and(|c| c.started_at_ms == call_started_ms)
                {
                    match still_active {
                        Some(true) => {
                            self.call_ui_absent_hits = 0;
                        }
                        Some(false) => {
                            self.call_ui_absent_hits = self.call_ui_absent_hits.saturating_add(1);
                            if self.call_ui_absent_hits >= 2 {
                                crate::backend::logfile::append("[call] ended: browser call UI gone");
                                self.active_call = None;
                                self.call_mic_idle_since_ms = None;
                                self.call_mic_ever_seen = false;
                                self.call_ui_absent_hits = 0;
                            }
                        }
                        None => {}
                    }
                }
            }
            UiEvent::ActiveCallEnded => {
                self.active_call = None;
                self.call_mic_idle_since_ms = None;
                self.call_mic_ever_seen = false;
                self.call_ui_absent_hits = 0;
                self.call_ui_probe_inflight = false;
                if let PopupState::Notif {
                    payload,
                    replying: false,
                    ..
                } = &self.popup
                {
                    if payload.is_call {
                        self.popup = PopupState::None;
                    }
                }
            }
            UiEvent::WindowBoundsResynced | UiEvent::RequestRedraw => {}
        }
    }

    fn render(
        &mut self,
        d2d: &mut D2dEngine,
        input: &UiInput,
        canvas_w: f32,
        _canvas_h: f32,
        _scale: f32,
    ) -> FrameDirectives {
        self.last_now_ms = input.now_ms;
        if input.mouse_inside {
            self.pointer_x = input.mouse_x;
            self.pointer_y = input.mouse_y;
        }
        let interacted_now = input.left_pressed
            || input.right_pressed
            || !input.keys.is_empty()
            || !input.chars.is_empty()
            || input.wheel_y.abs() > 0.01;
        if interacted_now {
            self.last_interaction_ms = input.now_ms;
        }

        // Expire transient hardware moments & modal popup TTLs
        if let Some(m) = &self.hardware_moment {
            if input.now_ms >= m.expires_at_ms {
                self.hardware_moment = None;
            }
        }
        if input.mouse_inside {
            if let PopupState::Voice {
                status,
                expires_at_ms,
                ..
            } = &mut self.popup
            {
                let is_term = status.eq_ignore_ascii_case("done")
                    || status.eq_ignore_ascii_case("Selesai")
                    || status.eq_ignore_ascii_case("error")
                    || status.eq_ignore_ascii_case("Gagal");
                if is_term {
                    *expires_at_ms = (*expires_at_ms).max(input.now_ms + 3_000);
                }
            }
        }
        if self.popup.is_expired(input.now_ms) {
            self.popup = PopupState::None;
        }
        self.flush_pending_approval(input.now_ms);

        // 1-second timer tick for Focus countdown, lyrics index, and due calendar reminders
        if input.now_ms.saturating_sub(self.last_sec_wall_ms) >= 1000 {
            self.last_sec_wall_ms = input.now_ms;
            self.tick_1s(input.now_ms);
        }

        let is_popup = self.popup.is_active();
        let has_song = self.media.as_ref().map(|m| !m.title.is_empty()).unwrap_or(false);
        let playing_song = has_song && self.media.as_ref().map(|m| m.is_playing).unwrap_or(false);
        let companion_busy = playing_song
            || self.focus_running
            || self.voice_live
            || self.convo_thinking
            || self.convo_live_running
            || self.active_call.is_some()
            || self.svc.voice.is_recording()
            || self.svc.voice.is_speaking()
            || matches!(
                self.popup,
                PopupState::Ask { .. } | PopupState::Voice { .. } | PopupState::Approval { .. }
            );
        let local_hour = chrono::Local::now().hour();
        let idle_ms = input.now_ms.saturating_sub(self.last_interaction_ms);
        let awake_override = input.now_ms < self.wake_until_ms;
        let companion_sleeping = should_companion_sleep(
            &self.settings.companion_mode,
            &self.settings.companion_sleeps,
            local_hour,
            idle_ms,
            awake_override,
            companion_busy,
        );
        let accent = if self.settings.album_tint {
            self.media
                .as_ref()
                .and_then(|m| m.accent_tint)
                .map(|(r, g, b)| Color::rgb(r, g, b))
                .unwrap_or_else(|| Color::from_hex(self.settings.accent_hex()))
        } else {
            Color::from_hex(self.settings.accent_hex())
        };
        let accent_rgb = [
            (accent.r * 255.0).round() as u8,
            (accent.g * 255.0).round() as u8,
            (accent.b * 255.0).round() as u8,
        ];

        // Compute closed strip snapshot + dynamic closed width
        let bat_pct = if self.battery.supported {
            Some((self.battery.level * 100.0).round() as u32)
        } else {
            None
        };
        let focus_progress = if self.focus_total_sec > 0 {
            1.0 - (self.focus_remaining_sec.min(self.focus_total_sec) as f32
                / self.focus_total_sec as f32)
        } else {
            0.0
        };
        let avatar_color = Color::from_hex(crate::state::avatar_hex(&self.settings.avatar));
        let closed_state = ClosedStripState {
            tucked: self.tucked,
            hovered: false,
            focus_running: self.focus_running,
            focus_remaining_sec: self.focus_remaining_sec,
            focus_total_sec: self.focus_total_sec,
            focus_progress,
            avatar_color,
            media: self
                .media
                .as_ref()
                .filter(|_| self.lyrics.show_player_state(has_song)),
            lyrics: &self.lyrics,
            closed_lyrics_enabled: self.settings.lyrics_on_closed,
            privacy: &self.privacy,
            active_call: self.active_call.as_ref(),
            battery_pct: bat_pct,
            battery_charging: self.battery.charging,
            hardware_moment: self.hardware_moment.as_ref(),
            moment_now_ms: input.now_ms,
        };

        // Target geometry dimensions
        let (target_w, target_h, target_br, target_ear) = if is_popup {
            let (pw, ph) = popup_target_size(&self.popup, &self.popup_state, d2d);
            let ear = match &self.popup {
                PopupState::Ask { .. } | PopupState::Voice { .. } => 8.0,
                _ => 0.0,
            };
            (pw, ph, 30.0, ear)
        } else if self.is_open {
            let (tw, th) = self.tab_target_size(d2d);
            (tw.min((canvas_w - 32.0).max(560.0)), th, 30.0, 0.0)
        } else if self.tucked {
            (240.0, 6.0, 4.0, 0.0)
        } else {
            let cw = target_closed_width(d2d, &closed_state);
            (cw, 30.0, 14.0, 8.0)
        };

        // Animate springs
        let a1 = self.sp_w.step(target_w, input.dt);
        let a2 = self.sp_h.step(target_h, input.dt);
        let a3 = self.sp_br.step(target_br, input.dt);
        let a4 = self.sp_ear.step(target_ear, input.dt);
        let spring_anim = a1 || a2 || a3 || a4;

        let cur_w = self.sp_w.pos.max(120.0);
        let cur_h = self.sp_h.pos.max(6.0);
        let notch_rect = Rect::new((canvas_w - cur_w) * 0.5, 0.0, cur_w, cur_h);

        self.hits.begin(input);
        self.hits.add_shell_rect(Rect::new(
            notch_rect.x - self.sp_ear.pos.max(0.0),
            notch_rect.y,
            notch_rect.w + self.sp_ear.pos.max(0.0) * 2.0,
            notch_rect.h,
        ));

        let is_glass = self.settings.notch_style == "glass";
        let shown_open = is_popup || self.is_open;
        let is_playing_song = has_song && self.media.as_ref().map(|m| m.is_playing).unwrap_or(false);
        let want_music_glow = self.settings.ambient_video
            && self.settings.show_music
            && self.is_open
            && !is_popup
            && is_playing_song
            && matches!(self.active_tab, ActiveTab::Glance | ActiveTab::Music);
        let target_music_glow = if want_music_glow {
            1.0
        } else if self.is_open
            && !is_popup
            && has_song
            && self.active_tab == ActiveTab::Music
            && (self.settings.ambient_video || self.settings.album_tint)
        {
            if is_playing_song { 0.85 } else { 0.55 }
        } else if self.settings.ambient_video
            && self.settings.show_music
            && self.is_open
            && !is_popup
            && has_song
            && matches!(self.active_tab, ActiveTab::Glance | ActiveTab::Music)
        {
            0.38
        } else {
            0.0
        };
        let glow_step = (input.dt * 1.4).clamp(0.01, 0.25);
        if self.music_glow_alpha < target_music_glow {
            self.music_glow_alpha = (self.music_glow_alpha + glow_step).min(target_music_glow);
        } else if self.music_glow_alpha > target_music_glow {
            self.music_glow_alpha = (self.music_glow_alpha - glow_step).max(target_music_glow);
        }
        let is_ai_popup = matches!(
            self.popup,
            PopupState::Ask { .. }
                | PopupState::Voice { .. }
                | PopupState::WhisperSetup { .. }
                | PopupState::AgentCalendarAlert { .. }
                | PopupState::Approval { .. }
        );
        let is_chat_tab = self.is_open && !is_popup && self.active_tab == ActiveTab::Conversation;
        let glow = if self.voice_live || self.convo_thinking {
            0.75
        } else if is_ai_popup {
            0.62
        } else if self.settings.ambient_video && self.settings.album_tint && shown_open && has_song {
            0.68
        } else {
            0.0
        };
        d2d.draw_notch_shell(
            notch_rect,
            self.sp_br.pos.max(2.0),
            self.sp_ear.pos.max(0.0),
            is_glass,
            shown_open,
            accent,
            glow,
        );
        if self.music_glow_alpha > 0.004 {
            let glow_accent = self
                .media
                .as_ref()
                .and_then(|m| m.accent_tint)
                .map(|(r, g, b)| Color::rgb(r, g, b))
                .unwrap_or(accent);
            d2d.draw_music_ambient_glow(
                notch_rect,
                self.sp_br.pos.max(2.0),
                self.sp_ear.pos.max(0.0),
                glow_accent,
                input.now_ms as f32 / 1000.0,
                self.music_glow_alpha,
            );
        }
        if is_ai_popup {
            d2d.draw_ai_aurora_rim(
                notch_rect,
                self.sp_br.pos.max(2.0),
                self.sp_ear.pos.max(0.0),
                input.now_ms as f32 / 1000.0,
                false,
            );
        } else if is_chat_tab {
            d2d.draw_ai_aurora_rim(
                notch_rect,
                self.sp_br.pos.max(2.0),
                self.sp_ear.pos.max(0.0),
                input.now_ms as f32 / 1000.0,
                true,
            );
        }

        let mut want_escape = is_popup;

        if is_popup {
            d2d.push_notch_clip(
                notch_rect,
                self.sp_br.pos.max(2.0),
                self.sp_ear.pos.max(0.0),
            );
            let pact = draw_popup(
                d2d,
                &mut self.hits,
                notch_rect,
                &mut self.popup,
                &mut self.popup_state,
                &self.settings,
                &self.custom_photo,
                accent,
                &self.privacy,
                &mut self.comp_renderer,
                &mut self.comp_state,
                &self.svc,
                input,
            );
            d2d.pop_layer();
            if pact.restart_focus {
                self.focus_total_sec = (self.settings.focus_minutes.max(1) as u32) * 60;
                self.focus_remaining_sec = self.focus_total_sec;
                self.focus_running = true;
            }
            if let Some(img_path) = pact.ask_ai_image_path {
                self.popup = PopupState::None;
                self.hits.focused_input = None;
                self.is_open = true;
                self.active_tab = ActiveTab::Conversation;
                self.convo_state.scroll_pending_bottom = true;
                let ui_prompt = "Lihat screenshot ini, jelaskan / bantu selesaikan.";
                crate::logic::router::convo_remember(
                    &self.svc.store,
                    Some(&self.svc.ui),
                    "user",
                    ui_prompt,
                );
                self.convo_thinking = true;
                let svc_c = self.svc.clone();
                self.svc.rt.spawn(async move {
                    let p_clone = img_path.clone();
                    let ocr_opt = svc_c.ocr.clone();
                    let (data_url, ocr_txt) = tokio::task::spawn_blocking(move || {
                        let durl = image::open(&p_clone).ok().and_then(|img| {
                            let scaled = if img.width() > 1280 || img.height() > 1280 {
                                img.resize(1280, 1280, image::imageops::FilterType::Triangle)
                            } else {
                                img
                            };
                            let mut buf = Vec::new();
                            let mut cur = std::io::Cursor::new(&mut buf);
                            scaled
                                .write_to(&mut cur, image::ImageFormat::Jpeg)
                                .ok()?;
                            use base64::Engine as _;
                            Some(format!(
                                "data:image/jpeg;base64,{}",
                                base64::engine::general_purpose::STANDARD.encode(&buf)
                            ))
                        });
                        let txt = ocr_opt
                            .and_then(|eng| eng.recognize_file(&p_clone).ok())
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        (durl, txt)
                    })
                    .await
                    .unwrap_or((None, String::new()));

                    let full_prompt = if ocr_txt.is_empty() {
                        "Lihat screenshot terlampir ini, jelaskan isinya atau bantu selesaikan masalahnya dalam Bahasa Indonesia.".to_string()
                    } else {
                        format!(
                            "Lihat screenshot terlampir ini, jelaskan isinya atau bantu selesaikan masalahnya dalam Bahasa Indonesia.\n\n[Teks OCR dari screenshot]:\n{}",
                            ocr_txt
                        )
                    };
                    let reply = match crate::logic::router::ask(
                        &svc_c.store,
                        Some(&svc_c.ui),
                        &full_prompt,
                        data_url.as_deref(),
                        crate::logic::router::RouterSurface::Conversation,
                        true,
                    )
                    .await
                    {
                        Ok(res) if !res.text.trim().is_empty() => res.text,
                        Ok(_) => "AI tidak mengembalikan jawaban untuk screenshot ini.".to_string(),
                        Err(e) => format!("Gagal menganalisis screenshot: {}", e),
                    };
                    crate::logic::router::convo_remember(
                        &svc_c.store,
                        Some(&svc_c.ui),
                        "assistant",
                        &reply,
                    );
                    svc_c.ui.send(UiEvent::ConvoThinking(false));
                    let clean: String = reply.split_whitespace().collect::<Vec<_>>().join(" ");
                    let preview: String = clean.chars().take(140).collect();
                    svc_c.ui.send(UiEvent::ConvoAiDoneNotification {
                        title: "Analisis Screenshot".to_string(),
                        body: if clean.chars().count() > 140 {
                            format!("{}...", preview)
                        } else {
                            preview
                        },
                    });
                });
            } else if pact.open_chat {
                self.popup = PopupState::None;
                self.is_open = true;
                self.active_tab = ActiveTab::Conversation;
                self.convo_state.input.multiline = true;
                self.convo_state.input.rich_paste = true;
                self.hits.focused_input = Some("input:convo_inp".to_string());
                self.close_after_ms = None;
            } else if pact.open_voice_settings {
                self.popup = PopupState::None;
                self.hits.focused_input = None;
                self.is_open = true;
                self.active_tab = ActiveTab::Settings;
                self.settings_state.section = SettingsSection::Voice;
                self.settings_state.scroll.offset = 0.0;
                self.settings_state.scroll.target = 0.0;
                self.close_after_ms = None;
            } else if pact.open_calendar {
                self.popup = PopupState::None;
                self.hits.focused_input = None;
                self.is_open = true;
                self.active_tab = ActiveTab::Calendar;
                self.close_after_ms = None;
            } else if pact.switch_to_ask {
                self.popup_state.ask_input.clear();
                self.popup_state.ask_input.multiline = true;
                self.popup_state.ask_input.rich_paste = true;
                self.popup_state.ask_attachments.clear();
                self.popup_state.ask_scroll.offset = 0.0;
                self.hits.focused_input = Some("input:popup_ask_inp".to_string());
                self.popup = PopupState::Ask {
                    run_id: 0,
                    query: String::new(),
                    loading: false,
                    ok: true,
                    answer: String::new(),
                    proposal: None,
                    proposal_saved: false,
                    harness_cmd: String::new(),
                    harness_running: false,
                    harness_ok: None,
                    inline_approval: None,
                };
            } else if pact.dismiss {
                self.popup = PopupState::None;
                self.hits.focused_input = None;
                self.flush_pending_approval(input.now_ms);
                if matches!(self.popup, PopupState::None) {
                    if let Some((open, tab)) = self.prev_open_before_approval.take() {
                        self.is_open = open;
                        self.active_tab = tab;
                    }
                }
            }
        } else if self.is_open && cur_h > 48.0 {
            d2d.push_notch_clip(notch_rect, self.sp_br.pos.max(2.0), 0.0);

            if self.prev_rendered_tab != Some(self.active_tab) {
                self.prev_rendered_tab = Some(self.active_tab);
                self.tab_fade_start_ms = input.now_ms;
            }
            let fade_t = ((input.now_ms.saturating_sub(self.tab_fade_start_ms)) as f32 / 140.0).clamp(0.0, 1.0);
            let tab_alpha = 1.0 - (1.0 - fade_t).powi(2);

            let unread = self.notifs.iter().filter(|n| !n.read).count();
            let tb = draw_top_bar(
                d2d,
                &mut self.hits,
                notch_rect,
                self.active_tab,
                &mut self.tb_indicator_x,
                &self.settings.hidden_views,
                self.pinned,
                self.settings.dnd,
                unread,
                self.focus_running,
                self.media.as_ref().map(|m| m.is_playing).unwrap_or(false),
                self.update_available.is_some(),
                &self.privacy,
                self.active_call.as_ref(),
                accent,
                input,
            );
            if let Some(tab) = tb.select_tab {
                self.active_tab = tab;
                if tab == ActiveTab::Music {
                    crate::backend::audio_out::refresh_outputs_async(&self.svc.rt, &self.svc.ui);
                }
            }
            if tb.toggle_pin {
                self.pinned = !self.pinned;
            }
            if tb.toggle_dnd {
                let mut m = Map::new();
                m.insert("dnd".to_string(), Value::Bool(!self.settings.dnd));
                self.svc.store.patch_settings(Some(&self.svc.ui), m);
            }
            if tb.end_call {
                let decline = self
                    .active_call
                    .as_ref()
                    .filter(|c| !system::is_browser_raw_app(&c.raw_app))
                    .map(|c| (c.raw_app.clone(), c.decline_args.clone()));
                self.active_call = None;
                self.call_mic_idle_since_ms = None;
                self.call_mic_ever_seen = false;
                self.call_ui_absent_hits = 0;
                self.call_ui_probe_inflight = false;
                if let Some((raw_app, args)) = decline {
                    if !args.is_empty() {
                        self.svc.rt.spawn_blocking(move || {
                            let _ =
                                crate::backend::system::activate_toast_com(&raw_app, &args, None);
                        });
                    }
                }
            }

            let tab_bounds = Rect::new(
                notch_rect.x + 14.0,
                notch_rect.y + 40.0,
                notch_rect.w - 28.0,
                notch_rect.h - 52.0,
            );
            let pushed_fade = d2d.push_opacity_layer(tab_bounds, tab_alpha);

            match self.active_tab {
                ActiveTab::Glance => {
                    let act = draw_glance(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        &self.custom_photo,
                        accent,
                        accent_rgb,
                        self.media.as_ref(),
                        &mut self.lyrics,
                        &self.todos,
                        &self.calendar_events,
                        &self.live_usage,
                        &self.battery,
                        &self.ai_limits,
                        self.focus_running,
                        self.focus_remaining_sec,
                        self.focus_total_sec,
                        companion_sleeping,
                        &mut self.home_state,
                        &mut self.comp_renderer,
                        &mut self.comp_state,
                        &mut self.particles,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.rt,
                        input,
                    );
                    if let Some(tab) = act.switch_tab {
                        self.active_tab = tab;
                    }
                    if act.toggle_focus {
                        self.focus_running = !self.focus_running;
                    }
                    if act.reset_focus {
                        self.focus_running = false;
                        self.focus_remaining_sec = self.focus_total_sec;
                    }
                    if act.woke_companion {
                        self.wake_until_ms = input.now_ms + 120_000;
                        self.last_interaction_ms = input.now_ms;
                    }
                }
                ActiveTab::Desk => {
                    let act = draw_desk(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        &self.custom_photo,
                        accent,
                        accent_rgb,
                        &self.todos,
                        &self.reminders,
                        &self.focus_log,
                        self.focus_running,
                        self.focus_remaining_sec,
                        self.focus_total_sec,
                        companion_sleeping,
                        &mut self.home_state,
                        &mut self.comp_renderer,
                        &mut self.comp_state,
                        &mut self.particles,
                        &self.svc.store,
                        &self.svc.ui,
                        input,
                    );
                    if act.toggle_focus {
                        self.focus_running = !self.focus_running;
                    }
                    if act.reset_focus {
                        self.focus_running = false;
                        self.focus_remaining_sec = self.focus_total_sec;
                    }
                    if act.woke_companion {
                        self.wake_until_ms = input.now_ms + 120_000;
                        self.last_interaction_ms = input.now_ms;
                    }
                    if let Some(secs) = act.set_focus_seconds {
                        let clamped = secs.clamp(10, 120 * 60);
                        self.focus_running = false;
                        self.focus_total_sec = clamped;
                        self.focus_remaining_sec = clamped;
                        if clamped % 60 == 0 {
                            let next_m = ((clamped / 60).clamp(1, 120)) as i32;
                            let mut p = Map::new();
                            p.insert("focusMinutes".to_string(), Value::from(next_m));
                            self.svc.store.patch_settings(Some(&self.svc.ui), p);
                        }
                    } else if act.adjust_focus_minutes != 0 && !self.focus_running {
                        let next_m = (self.settings.focus_minutes + act.adjust_focus_minutes)
                            .clamp(1, 120);
                        let mut p = Map::new();
                        p.insert("focusMinutes".to_string(), Value::from(next_m));
                        self.svc.store.patch_settings(Some(&self.svc.ui), p);
                        self.focus_total_sec = (next_m as u32) * 60;
                        self.focus_remaining_sec = self.focus_total_sec;
                    }
                }
                ActiveTab::Files => {
                    draw_files(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.clips,
                        &self.pins,
                        &self.shelf,
                        &self.search_hits,
                        self.files_drag_over,
                        accent,
                        &mut self.files_state,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.rt,
                        input,
                    );
                }
                ActiveTab::Music => {
                    draw_music(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        self.media.as_ref(),
                        &mut self.lyrics,
                        accent,
                        &mut self.music_state,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.rt,
                        input,
                    );
                }
                ActiveTab::Calendar => {
                    draw_calendar(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.calendar_events,
                        &self.reminders,
                        &mut self.cal_state,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.rt,
                        input,
                    );
                }
                ActiveTab::Conversation => {
                    draw_conversation(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        &self.custom_photo,
                        &self.convo,
                        self.convo_thinking,
                        &self.convo_live_cmd,
                        self.convo_live_running,
                        self.convo_live_ok,
                        self.voice_live,
                        &mut self.convo_state,
                        &mut self.comp_renderer,
                        &mut self.comp_state,
                        &self.svc,
                        input,
                    );
                }
                ActiveTab::Notifs => {
                    draw_notifs(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        &self.notifs,
                        &mut self.notifs_state,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.rt,
                        input,
                    );
                }
                ActiveTab::Settings => {
                    draw_settings(
                        d2d,
                        &mut self.hits,
                        notch_rect,
                        &self.settings,
                        &self.custom_photo,
                        accent,
                        &self.hotkey_failures,
                        &mut self.settings_state,
                        &mut self.comp_renderer,
                        &self.svc.store,
                        &self.svc.ui,
                        &self.svc.voice,
                        &self.svc.rt,
                        input,
                    );
                }
            }

            if pushed_fade {
                d2d.pop_layer();
            }

            d2d.pop_layer();

            if self.active_tab == ActiveTab::Desk {
                draw_desk_time_picker(d2d, &mut self.hits, &mut self.home_state, input);
            }
        } else {
            let closed_act = draw_closed_strip(
                d2d,
                &mut self.hits,
                notch_rect,
                self.sp_br.pos.max(2.0),
                self.sp_ear.pos.max(0.0),
                &closed_state,
                input,
            );
            if closed_act.expand_clicked {
                self.is_open = true;
                self.close_after_ms = None;
                self.last_interaction_ms = input.now_ms;
            }
        }

        draw_tooltip(d2d, &self.hits, input.now_ms, canvas_w);
        self.hits.end(input);

        let wants_focus = self.hits.focused_input.is_some();
        if wants_focus || self.pinned || self.is_open {
            want_escape = is_popup || wants_focus || self.svc.voice.is_speaking();
        }

        // Hover-to-open & 48px proximity + 700ms auto-close logic
        if !is_popup {
            let hover_trigger = Rect::new(
                notch_rect.x - 4.0,
                0.0,
                notch_rect.w + 8.0,
                if self.tucked { 12.0 } else { 32.0 },
            );
            if !self.is_open {
                if input.mouse_inside && hover_trigger.contains(self.pointer_x, self.pointer_y) {
                    self.is_open = true;
                    self.close_after_ms = None;
                    self.last_interaction_ms = input.now_ms;
                }
            } else if !self.pinned && !wants_focus && !self.files_drag_over {
                let pad = 48.0;
                let near_shell = Rect::new(
                    notch_rect.x - pad,
                    0.0,
                    notch_rect.w + pad * 2.0,
                    notch_rect.h + pad,
                )
                .contains(self.pointer_x, self.pointer_y);
                let near_picker = self.active_tab == ActiveTab::Desk
                    && self.home_state.rem_drawer_open
                    && self.home_state.time_picker_open
                    && self.home_state.tp_anchor.w > 0.0
                    && Rect::new(
                        self.home_state.tp_anchor.x - pad,
                        self.home_state.tp_anchor.bottom() - pad,
                        114.0 + pad * 2.0,
                        172.0 + pad * 2.0,
                    )
                    .contains(self.pointer_x, self.pointer_y);

                if near_shell || near_picker {
                    self.close_after_ms = None;
                } else {
                    let deadline = *self.close_after_ms.get_or_insert(input.now_ms + 700);
                    if input.now_ms >= deadline {
                        self.is_open = false;
                        self.close_after_ms = None;
                    }
                }
            } else {
                self.close_after_ms = None;
            }
        }

        if self.settings_state.recording_hotkey.is_some()
            && (!self.is_open
                || is_popup
                || self.active_tab != ActiveTab::Settings
                || self.settings_state.section != tabs::settings::SettingsSection::Voice)
        {
            self.settings_state.recording_hotkey = None;
            self.settings_state.hotkey_err.clear();
            if self.hits.focused_input.as_deref() == Some("hotkey_capture") {
                self.hits.focused_input = None;
            }
            self.svc.voice.hotkeys.set_paused(false);
        }

        if self.pending_cache_trim {
            d2d.trim_idle_caches();
            self.pending_cache_trim = false;
        }
        let fully_idle = !self.is_open
            && !is_popup
            && !self.voice_live
            && !self.svc.voice.is_recording()
            && !self.svc.voice.is_speaking()
            && !self.convo_thinking;
        if fully_idle {
            let idle_start = *self.idle_since_ms.get_or_insert(input.now_ms);
            if !self.idle_trimmed && input.now_ms.saturating_sub(idle_start) >= 10_000 {
                d2d.trim_idle_caches();
                self.svc.voice.release_idle_memory();
                std::thread::spawn(system::trim_process_working_set);
                self.idle_trimmed = true;
            }
        } else {
            self.idle_since_ms = None;
            self.idle_trimmed = false;
        }

        let any_active_anim = spring_anim
            || shown_open
            || (self.music_glow_alpha > 0.004 && self.music_glow_alpha < 0.996)
            || self.hardware_moment.is_some()
            || (self.is_open && self.focus_running)
            || (self.is_open
                && self.active_tab == ActiveTab::Files
                && self.files_state.search_anim > 0.01
                && self.files_state.search_anim < 0.99)
            || (has_song && self.media.as_ref().map(|m| m.is_playing).unwrap_or(false))
            || self.close_after_ms.is_some()
            || self.hits.any_switch_animating();

        let ask_active_anim = matches!(
            self.popup,
            PopupState::Ask { loading: true, .. } | PopupState::Ask { harness_running: true, .. }
        );
        FrameDirectives {
            cursor: self.hits.active_cursor(),
            wants_focus,
            capture_escape: want_escape,
            interactive_bounds: self.hits.shell_rects().to_vec(),
            hide_in_screenshots: self.settings.hide_in_screenshots || is_glass,
            is_animating: any_active_anim || ask_active_anim,
            target_display_id: self.settings.selected_display_id.clone(),
            request_redraw: self.convo_thinking || self.voice_live || ask_active_anim,
        }
    }
}

pub fn should_companion_sleep(
    companion_mode: &str,
    companion_sleeps: &str,
    hour: u32,
    idle_ms: u64,
    awake_override: bool,
    is_busy: bool,
) -> bool {
    if companion_mode == "sleep" {
        return true;
    }
    if is_busy || awake_override {
        return false;
    }
    let is_night = hour >= 22 || hour < 6;
    let is_idle_long = idle_ms >= 300_000;
    match companion_sleeps {
        "never" | "off" => false,
        "night" | "time" => is_night,
        "idle" => is_idle_long,
        _ => is_night || is_idle_long,
    }
}

pub fn should_defer_approval(
    popup_is_approval: bool,
    is_internal: bool,
    voice_busy: bool,
    popup_is_ask: bool,
) -> bool {
    popup_is_approval || (!is_internal && (voice_busy || popup_is_ask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_agent_milestones_fire_at_10m_and_5m_without_duplicates() {
        let mut alerted = HashSet::new();
        let ev_at = 1_000_000_000i64;

        // > 10 minutes before: nothing
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at - 11 * 60_000, &alerted, "ev1"),
            None
        );

        // Exactly 10 minutes before -> fires 10
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at - 10 * 60_000, &alerted, "ev1"),
            Some(10)
        );
        alerted.insert("ev1:10".to_string());

        // Still between 5m and 10m -> no duplicate
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at - 6 * 60_000, &alerted, "ev1"),
            None
        );

        // At 5 minutes before -> fires 5
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at - 5 * 60_000, &alerted, "ev1"),
            Some(5)
        );
        alerted.insert("ev1:5".to_string());

        // No further duplicates
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at - 60_000, &alerted, "ev1"),
            None
        );
        // Past event -> nothing
        assert_eq!(
            due_calendar_agent_milestone(ev_at, ev_at, &alerted, "ev1"),
            None
        );
    }

    #[test]
    fn calendar_event_epoch_parses_date_time_fallback() {
        let ev = CalendarEvent {
            id: "e1".into(),
            title: "Standup".into(),
            at: 0,
            date: "2026-11-21".into(),
            time: "14:30".into(),
            end_time: String::new(),
            end_at: 0,
            note: String::new(),
        };
        assert!(calendar_event_epoch_ms(&ev).is_some());
    }

    #[test]
    fn companion_auto_sleep_respects_night_idle_and_busy() {
        // Explicit sleep mode always sleeps even during activity
        assert!(should_companion_sleep("sleep", "never", 14, 0, false, true));

        // Night >= 22:00 or < 06:00 sleeps when idle & not busy
        assert!(should_companion_sleep("time", "auto", 22, 1_000, false, false));
        assert!(should_companion_sleep("time", "auto", 3, 1_000, false, false));
        assert!(should_companion_sleep("time", "night", 23, 0, false, false));

        // Daytime only sleeps in auto/idle mode after 5 min (300_000 ms) idle
        assert!(!should_companion_sleep("time", "auto", 14, 120_000, false, false));
        assert!(should_companion_sleep("time", "auto", 14, 300_000, false, false));
        assert!(should_companion_sleep("time", "idle", 14, 310_000, false, false));
        assert!(!should_companion_sleep("time", "night", 14, 600_000, false, false));

        // Music, focus, AI, or recent click wakes/keeps Companion awake
        assert!(!should_companion_sleep("time", "auto", 23, 600_000, false, true));
        assert!(!should_companion_sleep("time", "auto", 23, 600_000, true, false));
        assert!(!should_companion_sleep("time", "never", 23, 600_000, false, false));
    }

    #[test]
    fn approval_queue_defers_external_requests_during_voice_or_ask() {
        // External OpenCode/ZCode approval defers when Voice or Ask or another Approval is open
        assert!(should_defer_approval(false, false, true, false));
        assert!(should_defer_approval(false, false, false, true));
        assert!(should_defer_approval(true, false, false, false));
        assert!(!should_defer_approval(false, false, false, false));

        // Internal Voice approval displays immediately even while Voice overlay is active
        assert!(!should_defer_approval(false, true, true, false));
        // But queues if another ApprovalCard is currently on screen
        assert!(should_defer_approval(true, true, false, false));
    }
}
