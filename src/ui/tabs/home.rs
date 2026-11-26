use crate::backend::{smtc, store::StoreManager, system};
use crate::events::{AiLimitItem, BatteryState, LiveUsage, SmtcSessionInfo, UiSender};
use crate::logic::ask::format_tanggal_id;
use crate::logic::lyrics::LyricsState;
use crate::state::{now_unix_ms, CalendarEvent, FocusLogEntry, ReminderItem, Settings, TodoItem};
use crate::ui::companion::{CompanionMood, CompanionRenderer, CompanionState};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::particles::ParticleBuffers;
use crate::ui::top_bar::ActiveTab;
use crate::ui::widgets::{
    draw_frost_card, draw_icon_btn, draw_pill_btn, CursorKind, HitRegistry, ScrollState,
    TextInputState, TextInputStyle, UiInput,
};
use chrono::{DateTime, Datelike, Duration as ChronoDur, Local, NaiveDate, TimeZone, Timelike, Weekday};
use serde_json::{Map, Value};
use std::f32::consts::PI;

pub fn indonesian_greeting(hour: u32) -> &'static str {
    match hour {
        5..=10 => "Selamat pagi",
        11..=14 => "Selamat siang",
        15..=17 => "Selamat sore",
        _ => "Selamat malam",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeskCardMode {
    #[default]
    Focus,
    Reminder,
}

pub struct HomeTabState {
    pub todo_input: TextInputState,
    pub todo_scroll: ScrollState,
    pub desk_mode: DeskCardMode,
    pub focus_drawer_open: bool,
    pub min_input: TextInputState,
    pub sec_input: TextInputState,
    pub rem_drawer_open: bool,
    pub rem_time_input: TextInputState,
    pub rem_msg_input: TextInputState,
    pub rem_list_scroll: ScrollState,
    pub time_picker_open: bool,
    pub tp_hr_scroll: ScrollState,
    pub tp_min_scroll: ScrollState,
    pub tp_anchor: Rect,
    pub card_w_anim: f32,
    pub music_scrub_frac: Option<f32>,
    pub clock_last_hr: String,
    pub clock_prev_hr: String,
    pub clock_hr_anim: f32,
    pub clock_last_min: String,
    pub clock_prev_min: String,
    pub clock_min_anim: f32,
}

impl Default for HomeTabState {
    fn default() -> Self {
        Self {
            todo_input: TextInputState::default(),
            todo_scroll: ScrollState::default(),
            desk_mode: DeskCardMode::Focus,
            focus_drawer_open: false,
            min_input: TextInputState::default(),
            sec_input: TextInputState::default(),
            rem_drawer_open: false,
            rem_time_input: TextInputState::default(),
            rem_msg_input: TextInputState::default(),
            rem_list_scroll: ScrollState::default(),
            time_picker_open: false,
            tp_hr_scroll: ScrollState::default(),
            tp_min_scroll: ScrollState::default(),
            tp_anchor: Rect::default(),
            card_w_anim: 244.0,
            music_scrub_frac: None,
            clock_last_hr: String::new(),
            clock_prev_hr: String::new(),
            clock_hr_anim: 1.0,
            clock_last_min: String::new(),
            clock_prev_min: String::new(),
            clock_min_anim: 1.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HomeAction {
    pub switch_tab: Option<ActiveTab>,
    pub toggle_focus: bool,
    pub reset_focus: bool,
    pub adjust_focus_minutes: i32,
    pub set_focus_seconds: Option<u32>,
    pub woke_companion: bool,
}

enum HomeCardKind {
    Companion(f32),
    Music(f32),
    Tasks(f32),
    Status(f32),
    AiUsage(f32),
    FallbackClock(f32),
}

impl HomeCardKind {
    fn width(&self) -> f32 {
        match *self {
            HomeCardKind::Companion(w)
            | HomeCardKind::Music(w)
            | HomeCardKind::Tasks(w)
            | HomeCardKind::Status(w)
            | HomeCardKind::AiUsage(w)
            | HomeCardKind::FallbackClock(w) => w,
        }
    }
}

fn build_glance_cards(
    settings: &Settings,
    has_media_card: bool,
    ai_limits: &[AiLimitItem],
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,
) -> Vec<HomeCardKind> {
    let mut others: Vec<HomeCardKind> = Vec::new();
    if has_media_card && settings.show_music {
        others.push(HomeCardKind::Music(328.0));
    }
    if settings.show_tasks {
        others.push(HomeCardKind::Tasks(200.0));
    }
    if settings.show_status {
        others.push(HomeCardKind::Status(186.0));
    }
    if settings.show_ai_usage {
        let visible_cnt = ai_limits
            .iter()
            .filter(|l| !settings.hidden_limits.iter().any(|h| h == &l.id))
            .count()
            .clamp(1, 4);
        others.push(HomeCardKind::AiUsage(24.0 + visible_cnt as f32 * 72.0));
    }

    let mut cards: Vec<HomeCardKind> = Vec::new();
    if settings.show_avatar {
        let timer_dirty = focus_running || focus_remaining_sec < focus_total_sec;
        let focus_active = settings.companion_mode == "focus"
            || (timer_dirty && settings.companion_mode != "calendar");
        let comp_w = if focus_active {
            244.0
        } else {
            match others.len() {
                0 => 468.0,
                1 => 330.0,
                _ => 236.0,
            }
        };
        cards.push(HomeCardKind::Companion(comp_w));
    }
    for c in others {
        if cards.len() >= 4 {
            break;
        }
        cards.push(c);
    }
    if cards.is_empty() {
        cards.push(HomeCardKind::FallbackClock(160.0));
    }
    cards
}

pub fn target_glance_size(
    settings: &Settings,
    media: Option<&SmtcSessionInfo>,
    lyrics: &LyricsState,
    ai_limits: &[AiLimitItem],
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,
) -> (f32, f32) {
    let has_song = media.map(|m| !m.title.is_empty()).unwrap_or(false);
    let has_media_card = lyrics.show_player_state(has_song);
    let cards = build_glance_cards(
        settings,
        has_media_card,
        ai_limits,
        focus_running,
        focus_remaining_sec,
        focus_total_sec,
    );
    let n = cards.len();
    let sum_w: f32 = cards.iter().map(|c| c.width()).sum();
    let total_w = (24.0 + sum_w + (n.saturating_sub(1) as f32) * 10.0).max(320.0);
    (total_w, 188.0)
}

#[allow(clippy::too_many_arguments)]
pub fn draw_glance(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    custom_photo: &str,
    accent: Color,
    accent_rgb: [u8; 3],
    media: Option<&SmtcSessionInfo>,
    lyrics: &mut LyricsState,
    todos: &[TodoItem],
    calendar_events: &[CalendarEvent],
    live_usage: &LiveUsage,
    battery: &BatteryState,
    ai_limits: &[AiLimitItem],
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,
    companion_sleeping: bool,
    state: &mut HomeTabState,
    comp_renderer: &mut CompanionRenderer,
    comp_state: &mut CompanionState,
    particles: &mut ParticleBuffers,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) -> HomeAction {
    let mut act = HomeAction::default();
    let has_song = media.map(|m| !m.title.is_empty()).unwrap_or(false);
    let has_media_card = lyrics.show_player_state(has_song);
    let cards = build_glance_cards(
        settings,
        has_media_card,
        ai_limits,
        focus_running,
        focus_remaining_sec,
        focus_total_sec,
    );

    let mut x = notch_rect.x + 12.0;
    let y = notch_rect.y + 40.0;
    let h = 136.0;

    for card in cards {
        let cw = card.width();
        let rect = Rect::new(x, y, cw, h);
        match card {
            HomeCardKind::Companion(_) => {
                draw_companion_card(
                    d2d,
                    hits,
                    rect,
                    settings,
                    custom_photo,
                    accent,
                    accent_rgb,
                    focus_running,
                    focus_remaining_sec,
                    focus_total_sec,
                    companion_sleeping,
                    calendar_events,
                    state,
                    comp_renderer,
                    comp_state,
                    particles,
                    store,
                    ui,
                    rt,
                    input,
                    &mut act,
                );
            }
            HomeCardKind::Music(_) => {
                if let Some(m) = media {
                    draw_music_card(d2d, hits, rect, accent, m, lyrics, state, rt, ui, input, &mut act);
                }
            }
            HomeCardKind::Tasks(_) => {
                draw_tasks_preview_card(d2d, hits, rect, accent, todos, store, ui, input, &mut act);
            }
            HomeCardKind::Status(_) => {
                draw_status_card(d2d, hits, rect, live_usage, battery, particles, input);
            }
            HomeCardKind::AiUsage(_) => {
                draw_ai_usage_card(
                    d2d,
                    hits,
                    rect,
                    settings,
                    accent_rgb,
                    ai_limits,
                    particles,
                    input,
                    &mut act,
                );
            }
            HomeCardKind::FallbackClock(_) => {
                draw_fallback_clock_card(d2d, hits, rect, input, &mut act);
            }
        }
        x += cw + 10.0;
    }

    act
}

fn nearest_upcoming<'a>(events: &'a [CalendarEvent], now_ms: i64) -> Option<&'a CalendarEvent> {
    events
        .iter()
        .filter(|e| {
            if e.end_at > e.at {
                e.end_at >= now_ms
            } else {
                e.at >= now_ms - 300_000
            }
        })
        .min_by_key(|e| e.at)
}

fn format_short_date_id(date: NaiveDate) -> String {
    let short_m = [
        "Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des",
    ];
    let mi = (date.month().saturating_sub(1) as usize).min(11);
    let dow = match date.weekday() {
        Weekday::Mon => "Senin",
        Weekday::Tue => "Selasa",
        Weekday::Wed => "Rabu",
        Weekday::Thu => "Kamis",
        Weekday::Fri => "Jumat",
        Weekday::Sat => "Sabtu",
        Weekday::Sun => "Minggu",
    };
    format!("{}, {} {} {}", dow, date.day(), short_m[mi], date.year())
}

fn format_event_compact_day(ev: &CalendarEvent) -> String {
    let today = Local::now().date_naive();
    if let Ok(d) = NaiveDate::parse_from_str(&ev.date, "%Y-%m-%d") {
        if d == today {
            return "Hari ini".to_string();
        }
        if d == today + ChronoDur::days(1) {
            return "Besok".to_string();
        }
        let short_m = [
            "Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des",
        ];
        let mi = (d.month().saturating_sub(1) as usize).min(11);
        return format!("{} {}", d.day(), short_m[mi]);
    }
    ev.date.clone()
}

fn format_event_day_rel(ev: &CalendarEvent) -> String {
    let today = Local::now().date_naive();
    if let Ok(d) = NaiveDate::parse_from_str(&ev.date, "%Y-%m-%d") {
        let short_m = [
            "Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des",
        ];
        let mi = (d.month().saturating_sub(1) as usize).min(11);
        if d == today {
            return format!("Hari ini · {} {}", d.day(), short_m[mi]);
        }
        if d == today + ChronoDur::days(1) {
            return format!("Besok · {} {}", d.day(), short_m[mi]);
        }
        return format_short_date_id(d);
    }
    ev.date.clone()
}

#[allow(clippy::too_many_arguments)]
fn draw_companion_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    settings: &Settings,
    custom_photo: &str,
    accent: Color,
    accent_rgb: [u8; 3],
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,
    companion_sleeping: bool,
    calendar_events: &[CalendarEvent],
    state: &mut HomeTabState,
    comp_renderer: &mut CompanionRenderer,
    comp_state: &mut CompanionState,
    particles: &mut ParticleBuffers,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
    act: &mut HomeAction,
) {
    let card_hit = hits.register("card_comp", rect, CursorKind::Arrow, None, input);
    draw_frost_card(d2d, rect, card_hit.hovered, None);

    let mode = settings.companion_mode.as_str();
    let timer_active = focus_running || focus_remaining_sec < focus_total_sec;
    let show_focus = mode == "focus" || (timer_active && mode != "calendar");

    let (comp_r, size) = if show_focus {
        (Rect::new(rect.x + 12.0, rect.y + 18.0, 84.0, 100.0), 66.0)
    } else {
        let comp_w = if rect.w > 300.0 { 112.0 } else { 84.0 };
        (
            Rect::new(rect.x + 6.0, rect.y + 8.0, comp_w, rect.h - 16.0),
            if rect.w > 300.0 { 72.0 } else { 64.0 },
        )
    };
    if show_focus && timer_active {
        d2d.push_rounded_clip(rect, 24.0);
        let prog = if focus_total_sec > 0 {
            let p = (1.0 - (focus_remaining_sec as f32 / focus_total_sec as f32)).clamp(0.0, 1.0);
            if focus_running && p < 0.015 { 0.015 } else { p }
        } else {
            0.0
        };
        let ring_r = Rect::new(comp_r.center_x() - 50.0, comp_r.center_y() - 50.0, 100.0, 100.0);
        let tick = if focus_running {
            input.now_ms / 16
        } else {
            (focus_remaining_sec as u64).saturating_mul(100) + 1
        };
        particles.draw_focus_ring(
            d2d,
            "home_focus_ring",
            ring_r,
            prog,
            accent_rgb,
            input.now_ms as f32 / 1000.0,
            focus_running,
            tick,
        );
        d2d.pop_layer();
    }

    comp_state.mood = if focus_running {
        CompanionMood::Focus
    } else if companion_sleeping || settings.companion_mode == "sleep" {
        CompanionMood::Sleep
    } else {
        CompanionMood::Idle
    };
    let comp_clicked = comp_renderer.draw(
        d2d,
        hits,
        "home_comp",
        comp_r,
        size,
        &settings.avatar,
        settings.accent_hex(),
        custom_photo,
        comp_state,
        input,
    );
    if comp_clicked {
        act.woke_companion = true;
    }
    CompanionRenderer::cycle_avatar_on_click(comp_clicked, settings, custom_photo, store, ui);

    let rx = if show_focus {
        rect.x + 108.0
    } else {
        comp_r.right() + 2.0
    };
    let rw = (rect.right() - rx - 10.0).max(60.0);

    // Mode switcher dots top-right: Clock, Focus, Upcoming Calendar
    let modes = [("time", "Jam"), ("focus", "Fokus"), ("calendar", "Jadwal Terdekat")];
    let mut mx = rect.right() - 14.0;
    for (m_key, m_tip) in modes.iter().rev() {
        mx -= 14.0;
        let mr = Rect::new(mx, rect.y + 12.0, 12.0, 12.0);
        let mh = hits.register(
            format!("comp_mode_{}", m_key),
            mr,
            CursorKind::Pointer,
            Some((*m_tip).to_string()),
            input,
        );
        let is_cur = if show_focus { *m_key == "focus" } else { mode == *m_key };
        d2d.fill_circle(
            mr.center_x(),
            mr.center_y(),
            if is_cur { 3.5 } else { 2.5 },
            if is_cur {
                accent
            } else if mh.hovered {
                Color::white(0.65)
            } else {
                Color::white(0.22)
            },
        );
        if mh.clicked {
            let mut patch = Map::new();
            patch.insert("companionMode".to_string(), Value::String((*m_key).to_string()));
            store.patch_settings(Some(ui), patch);
        }
    }

    if show_focus {
        let f_pill_w = (rw - 4.0).clamp(64.0, 108.0);
        let f_pill = Rect::new(rx, rect.y + 24.0, f_pill_w, 19.0);
        let fh = hits.register(
            "home_focus_pill",
            f_pill,
            CursorKind::Pointer,
            Some("Buka tab Focus".to_string()),
            input,
        );
        d2d.fill_rounded_rect(
            f_pill,
            9.5,
            accent.with_alpha(if fh.hovered { 1.0 } else { 0.92 }),
        );
        d2d.draw_text(
            "FOCUS",
            f_pill,
            &TextStyle::default()
                .size(8.5)
                .bold()
                .color(Color::rgb(12, 12, 16))
                .center()
                .vcenter(),
        );
        if fh.clicked {
            act.switch_tab = Some(ActiveTab::Desk);
        }

        let mm = focus_remaining_sec / 60;
        let ss = focus_remaining_sec % 60;
        let t_r = Rect::new(rx, rect.y + 47.0, rw, 34.0);
        let th = hits.register(
            "home_focus_time_open",
            t_r,
            CursorKind::Pointer,
            Some("Buka tab Focus".to_string()),
            input,
        );
        d2d.draw_text(
            &format!("{}:{:02}", mm, ss),
            t_r,
            &TextStyle::default()
                .size(26.0)
                .bold()
                .color(if timer_active {
                    accent
                } else {
                    Color::white(if th.hovered { 1.0 } else { 0.95 })
                })
                .vcenter()
                .nowrap(),
        );
        if th.clicked {
            act.switch_tab = Some(ActiveTab::Desk);
        }
        let by = rect.y + 88.0;
        let btn_r = Rect::new(rx, by, 68.0, 26.0);
        let bh = hits.register("home_focus_toggle", btn_r, CursorKind::Pointer, None, input);
        let (btn_bg, btn_fg, btn_lbl) = if focus_running {
            (
                Color::white(if bh.hovered { 0.16 } else { 0.10 }),
                Color::white(0.95),
                "Pause",
            )
        } else {
            (
                accent.with_alpha(if bh.hovered { 1.0 } else { 0.92 }),
                Color::rgb(12, 12, 16),
                if timer_active { "Resume" } else { "Start" },
            )
        };
        d2d.fill_rounded_rect(btn_r, 13.0, btn_bg);
        d2d.draw_text(
            btn_lbl,
            btn_r,
            &TextStyle::default()
                .size(11.0)
                .semibold()
                .color(btn_fg)
                .center()
                .vcenter(),
        );
        if bh.clicked {
            act.toggle_focus = true;
        }
        if timer_active {
            let rr = Rect::new(rx + 74.0, by, 26.0, 26.0);
            d2d.fill_circle(rr.center_x(), rr.center_y(), 13.0, Color::white(0.08));
            if draw_icon_btn(
                d2d,
                hits,
                "home_focus_reset",
                rr,
                IconId::Refresh,
                12.0,
                Color::white(0.60),
                Color::white(0.14),
                Some("Reset timer"),
                input,
            ) {
                act.reset_focus = true;
            }
        } else {
            let rr = Rect::new(rx + 74.0, by, 26.0, 26.0);
            d2d.fill_circle(rr.center_x(), rr.center_y(), 13.0, Color::white(0.08));
            if draw_icon_btn(
                d2d,
                hits,
                "home_focus_open",
                rr,
                IconId::ExternalLink,
                11.5,
                Color::white(0.55),
                Color::white(0.14),
                Some("Atur durasi"),
                input,
            ) {
                act.switch_tab = Some(ActiveTab::Desk);
            }
        }
    } else if mode == "calendar" {
        let up_zone = Rect::new(rx, rect.y + 24.0, rw, rect.h - 32.0);
        let up_hit = hits.register(
            "home_comp_upcoming_open",
            up_zone,
            CursorKind::Pointer,
            Some("Buka Calendar".to_string()),
            input,
        );
        d2d.draw_text(
            "UPCOMING",
            Rect::new(rx, rect.y + 20.0, rw, 14.0),
            &TextStyle::default()
                .size(9.5)
                .bold()
                .color(Color::white(0.45)),
        );
        if let Some(ev) = nearest_upcoming(calendar_events, now_unix_ms()) {
            let t_txt = if !ev.end_time.is_empty() {
                format!("{} - {}", ev.time, ev.end_time)
            } else {
                ev.time.clone()
            };
            let t_sz = if ev.end_time.is_empty() { 22.0 } else { 17.5 };
            d2d.draw_text(
                &t_txt,
                Rect::new(rx, rect.y + 38.0, rw, 28.0),
                &TextStyle::default()
                    .size(t_sz)
                    .bold()
                    .color(Color::rgb(251, 191, 36))
                    .vcenter()
                    .nowrap(),
            );
            d2d.draw_text(
                &ev.title,
                Rect::new(rx, rect.y + 70.0, rw, 20.0),
                &TextStyle::default()
                    .size(12.5)
                    .semibold()
                    .color(Color::white(if up_hit.hovered { 0.98 } else { 0.90 }))
                    .ellipsis(),
            );
            let meeting = system::detect_meeting_link(&ev.note)
                .or_else(|| system::detect_meeting_link(&ev.title));
            let rel_w = if meeting.is_some() { (rw - 48.0).max(40.0) } else { rw };
            d2d.draw_text(
                &format_event_day_rel(ev),
                Rect::new(rx, rect.y + 94.0, rel_w, 16.0),
                &TextStyle::default().size(10.5).color(Color::white(0.55)).ellipsis(),
            );
            let mut join_clicked = false;
            if let Some(lk) = meeting {
                let jr = Rect::new(rect.right() - 52.0, rect.y + 92.0, 42.0, 18.0);
                let jh = hits.register(
                    "home_up_join",
                    jr,
                    CursorKind::Pointer,
                    Some(format!("Join {}", lk.provider.label())),
                    input,
                );
                d2d.fill_rounded_rect(
                    jr,
                    6.0,
                    if jh.hovered {
                        Color::rgb(59, 130, 246)
                    } else {
                        Color::rgb(37, 99, 235)
                    },
                );
                d2d.draw_text(
                    "Join",
                    jr,
                    &TextStyle::default()
                        .size(9.5)
                        .bold()
                        .color(Color::white(0.98))
                        .center()
                        .vcenter(),
                );
                if jh.clicked {
                    join_clicked = true;
                    let lc = lk.clone();
                    rt.spawn(async move {
                        let _ = system::join_meeting_smart(lc).await;
                    });
                }
            }
            if up_hit.clicked && !join_clicked {
                act.switch_tab = Some(ActiveTab::Calendar);
            }
        } else {
            d2d.draw_icon(
                IconId::Calendar,
                Rect::new(rx, rect.y + 44.0, 15.0, 15.0),
                Color::white(0.45),
                1.9,
                1.0,
            );
            d2d.draw_text(
                "Belum ada jadwal\nterdekat",
                Rect::new(rx + 22.0, rect.y + 41.0, rw - 22.0, 36.0),
                &TextStyle::default()
                    .size(11.5)
                    .color(Color::white(if up_hit.hovered { 0.80 } else { 0.50 }))
                    .line_height(15.0),
            );
            if up_hit.clicked {
                act.switch_tab = Some(ActiveTab::Calendar);
            }
        }
    } else {
        // Default "time" mode: Greeting + Tauri-style animated 12h clock + Indonesian date + optional Upcoming calendar event
        let now = Local::now();
        let hr24 = now.hour();
        let greet = indonesian_greeting(hr24);
        d2d.draw_text(
            greet,
            Rect::new(rx, rect.y + 15.0, rw, 16.0),
            &TextStyle::default().size(11.0).medium().color(Color::white(0.54)),
        );

        let hr12 = match hr24 % 12 {
            0 => 12,
            h => h,
        };
        let hr_s = format!("{}", hr12);
        let min_s = format!("{:02}", now.minute());
        let meridiem = if hr24 < 12 { "AM" } else { "PM" };

        if state.clock_last_hr.is_empty() {
            state.clock_last_hr = hr_s.clone();
            state.clock_hr_anim = 1.0;
        } else if state.clock_last_hr != hr_s {
            state.clock_prev_hr = std::mem::replace(&mut state.clock_last_hr, hr_s.clone());
            state.clock_hr_anim = 0.0;
        }
        if state.clock_last_min.is_empty() {
            state.clock_last_min = min_s.clone();
            state.clock_min_anim = 1.0;
        } else if state.clock_last_min != min_s {
            state.clock_prev_min = std::mem::replace(&mut state.clock_last_min, min_s.clone());
            state.clock_min_anim = 0.0;
        }
        state.clock_hr_anim = (state.clock_hr_anim + input.dt * 3.4).min(1.0);
        state.clock_min_anim = (state.clock_min_anim + input.dt * 3.4).min(1.0);

        let ease_out = |p: f32| -> f32 {
            let t = p.clamp(0.0, 1.0);
            1.0 - (1.0 - t).powi(3)
        };

        let clk_y = rect.y + 34.0;
        let clk_h = 34.0;
        let clk_st = TextStyle::default()
            .size(28.0)
            .semibold()
            .color(Color::white(0.96))
            .vcenter()
            .nowrap();
        let (hr_w, _) = d2d.measure_text(&hr_s, 64.0, &clk_st);
        let (colon_w, _) = d2d.measure_text(":", 24.0, &clk_st);
        let (min_w, _) = d2d.measure_text(&min_s, 64.0, &clk_st);

        let draw_flip_part = |d2d: &mut D2dEngine, x: f32, w: f32, cur: &str, prev: &str, anim: f32| {
            let clip_r = Rect::new(x - 1.0, clk_y, w + 2.0, clk_h);
            d2d.push_clip_rect(clip_r);
            if anim < 0.999 && !prev.is_empty() {
                let p = ease_out(anim);
                d2d.draw_text(
                    prev,
                    Rect::new(x, clk_y + p * 14.0, w + 2.0, clk_h),
                    &clk_st.clone().color(Color::white(0.96 * (1.0 - p))),
                );
                d2d.draw_text(
                    cur,
                    Rect::new(x, clk_y - (1.0 - p) * 14.0, w + 2.0, clk_h),
                    &clk_st.clone().color(Color::white(0.96 * p)),
                );
            } else {
                d2d.draw_text(cur, Rect::new(x, clk_y, w + 2.0, clk_h), &clk_st);
            }
            d2d.pop_clip_rect();
        };

        let mut cx = rx;
        draw_flip_part(d2d, cx, hr_w, &hr_s, &state.clock_prev_hr, state.clock_hr_anim);
        cx += hr_w + 1.0;

        let sec_f = (input.now_ms % 2000) as f32 / 1000.0;
        let pulse = 0.22 + 0.78 * (0.5 + 0.5 * (sec_f * PI).cos());
        d2d.draw_text(
            ":",
            Rect::new(cx, clk_y - 1.5, colon_w + 2.0, clk_h),
            &clk_st.clone().color(accent.with_alpha(pulse)),
        );
        cx += colon_w + 1.0;

        draw_flip_part(d2d, cx, min_w, &min_s, &state.clock_prev_min, state.clock_min_anim);
        cx += min_w + 5.0;

        let mer_w = (rx + rw - cx).max(24.0);
        d2d.draw_text(
            meridiem,
            Rect::new(cx, clk_y + 13.0, mer_w, 18.0),
            &TextStyle::default()
                .size(11.0)
                .semibold()
                .color(Color::white(0.46))
                .nowrap(),
        );

        let date_full = format_tanggal_id(now.date_naive());
        let date_st = TextStyle::default().size(10.5).medium().color(Color::white(0.50)).ellipsis();
        let (dw, _) = d2d.measure_text(&date_full, 300.0, &date_st);
        let date_s = if dw <= rw {
            date_full
        } else {
            format_short_date_id(now.date_naive())
        };
        d2d.draw_text(&date_s, Rect::new(rx, rect.y + 73.0, rw, 18.0), &date_st);

        if let Some(ev) = nearest_upcoming(calendar_events, now_unix_ms()) {
            let up_r = Rect::new(rx, rect.bottom() - 31.0, rw, 20.0);
            let uh = hits.register(
                "home_comp_cal_line",
                up_r,
                CursorKind::Pointer,
                Some("Buka Calendar".to_string()),
                input,
            );
            d2d.fill_circle(up_r.x + 3.0, up_r.center_y(), 2.5, Color::rgb(251, 191, 36));
            d2d.draw_text(
                &format!("{} {} · {}", format_event_compact_day(ev), ev.time, ev.title),
                Rect::new(up_r.x + 10.0, up_r.y, (up_r.w - 10.0).max(20.0), up_r.h),
                &TextStyle::default()
                    .size(10.5)
                    .medium()
                    .color(Color::white(if uh.hovered { 0.96 } else { 0.72 }))
                    .vcenter()
                    .ellipsis(),
            );
            if uh.clicked {
                act.switch_tab = Some(ActiveTab::Calendar);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_music_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    accent: Color,
    m: &SmtcSessionInfo,
    lyrics: &mut LyricsState,
    state: &mut HomeTabState,
    rt: &tokio::runtime::Handle,
    ui: &UiSender,
    input: &UiInput,
    act: &mut HomeAction,
) {
    let ch = hits.register("card_music", rect, CursorKind::Arrow, None, input);
    let tint_col = m
        .accent_tint
        .map(|(r, g, b)| Color::rgb(r, g, b))
        .unwrap_or(accent);
    draw_frost_card(d2d, rect, ch.hovered, None);

    d2d.push_rounded_clip(rect, 24.0);
    d2d.fill_radial_glow(
        rect.x + 62.0,
        rect.center_y(),
        118.0,
        88.0,
        tint_col.with_alpha(0.32),
        tint_col.with_alpha(0.0),
    );
    d2d.pop_layer();

    let art_r = Rect::new(rect.x + 14.0, rect.y + 22.0, 92.0, 92.0);
    let art_hit = hits.register(
        "home_art",
        art_r,
        CursorKind::Pointer,
        Some("Buka pemutar lirik".to_string()),
        input,
    );
    if let Some(thumb) = m.thumbnail_bytes.as_deref().filter(|b| !b.is_empty()) {
        let key = format!("smtc:{}:{}:{}", m.artist, m.album, thumb.len());
        d2d.draw_image_bytes(&key, thumb, art_r, 16.0, 1.0, true);
    } else {
        d2d.fill_rounded_rect(art_r, 16.0, Color::white(0.08));
        d2d.draw_icon(IconId::Music, art_r.inset(28.0, 28.0), Color::white(0.65), 1.8, 1.0);
    }
    if art_hit.clicked {
        act.switch_tab = Some(ActiveTab::Music);
    }

    let rx = art_r.right() + 14.0;
    let rw = (rect.right() - rx - 14.0).max(80.0);

    d2d.draw_text(
        &m.title,
        Rect::new(rx, rect.y + 18.0, rw, 20.0),
        &TextStyle::default()
            .size(13.0)
            .semibold()
            .color(Color::white(0.95))
            .ellipsis(),
    );
    let sub = if m.artist.is_empty() { "Media" } else { &m.artist };
    d2d.draw_text(
        sub,
        Rect::new(rx, rect.y + 38.0, rw, 16.0),
        &TextStyle::default()
            .size(11.0)
            .color(Color::white(0.54))
            .ellipsis(),
    );

    if lyrics.status == "synced" && !lyrics.current_line_text().is_empty() {
        let lyr_col = tint_col.lerp(Color::white(1.0), 0.62);
        d2d.draw_text(
            lyrics.current_line_text(),
            Rect::new(rx, rect.y + 56.0, rw, 16.0),
            &TextStyle::default()
                .size(11.0)
                .bold()
                .color(lyr_col)
                .ellipsis(),
        );
    }

    let eff_dur_ms = if m.duration_sec > 1.0 {
        (m.duration_sec * 1000.0) as u64
    } else if lyrics.dur_ms > 1000 {
        lyrics.dur_ms
    } else {
        lyrics
            .lines
            .last()
            .map(|l| l.t_ms.saturating_add(4_000))
            .unwrap_or(0)
    };
    let bar_hit_r = Rect::new(rx, rect.y + 74.0, rw, 14.0);
    let seek_h = hits.register("home_seek", bar_hit_r, CursorKind::Pointer, None, input);
    if eff_dur_ms > 0 && (seek_h.down || (state.music_scrub_frac.is_some() && input.left_down)) {
        let p = ((input.mouse_x - rx) / rw.max(1.0)).clamp(0.0, 1.0);
        state.music_scrub_frac = Some(p);
    } else if let Some(p) = state.music_scrub_frac.take() {
        if eff_dur_ms > 0 {
            let target_ms = (p as f64 * eff_dur_ms as f64) as u64;
            let sec = target_ms as f64 / 1000.0;
            lyrics.seek_local(target_ms);
            smtc::seek_and_refresh(rt, ui, sec, &m.source_app_id, &m.title);
        }
    } else if seek_h.clicked && eff_dur_ms > 0 {
        let p = ((input.mouse_x - rx) / rw.max(1.0)).clamp(0.0, 1.0);
        let target_ms = (p as f64 * eff_dur_ms as f64) as u64;
        let sec = target_ms as f64 / 1000.0;
        lyrics.seek_local(target_ms);
        smtc::seek_and_refresh(rt, ui, sec, &m.source_app_id, &m.title);
    }
    let frac = if let Some(p) = state.music_scrub_frac {
        p
    } else if eff_dur_ms > 0 {
        ((lyrics.cur_ms() as f64 / eff_dur_ms as f64) as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let bar_y = bar_hit_r.center_y() - 2.0;
    d2d.fill_rounded_rect(Rect::new(rx, bar_y, rw, 4.0), 2.0, Color::white(0.14));
    if frac > 0.001 {
        d2d.fill_rounded_rect(
            Rect::new(rx, bar_y, rw * frac, 4.0),
            2.0,
            tint_col,
        );
    }
    if seek_h.hovered || state.music_scrub_frac.is_some() {
        d2d.fill_circle(rx + rw * frac, bar_hit_r.center_y(), 4.8, Color::white(0.98));
    }

    let ty = rect.bottom() - 38.0;
    if draw_icon_btn(
        d2d,
        hits,
        "home_prev",
        Rect::new(rx, ty, 26.0, 26.0),
        IconId::SkipBack,
        14.0,
        Color::white(0.82),
        Color::white(0.10),
        Some("Sebelumnya"),
        input,
    ) {
        smtc::media_key_and_refresh(rt, ui, "prev", &m.source_app_id, &m.title);
    }
    let pp_r = Rect::new(rx + 32.0, ty - 1.0, 28.0, 28.0);
    let pp_hit = hits.register("home_pp", pp_r, CursorKind::Pointer, None, input);
    d2d.fill_circle(
        pp_r.center_x(),
        pp_r.center_y(),
        14.0,
        if pp_hit.hovered {
            Color::white(0.98)
        } else {
            Color::white(0.90)
        },
    );
    d2d.draw_icon(
        if m.is_playing { IconId::Pause } else { IconId::Play },
        pp_r.inset(7.0, 7.0),
        Color::black(0.95),
        1.8,
        1.0,
    );
    if pp_hit.clicked {
        smtc::media_key_and_refresh(rt, ui, "playpause", &m.source_app_id, &m.title);
    }
    if draw_icon_btn(
        d2d,
        hits,
        "home_next",
        Rect::new(rx + 66.0, ty, 26.0, 26.0),
        IconId::SkipForward,
        14.0,
        Color::white(0.82),
        Color::white(0.10),
        Some("Berikutnya"),
        input,
    ) {
        smtc::media_key_and_refresh(rt, ui, "next", &m.source_app_id, &m.title);
    }
    if draw_pill_btn(
        d2d,
        hits,
        "home_lyrics_open",
        Rect::new(rect.right() - 64.0, ty + 2.0, 50.0, 22.0),
        "Lirik",
        false,
        None,
        input,
    ) {
        act.switch_tab = Some(ActiveTab::Music);
    }
}

fn draw_tasks_preview_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    accent: Color,
    todos: &[TodoItem],
    store: &StoreManager,
    ui: &UiSender,
    input: &UiInput,
    act: &mut HomeAction,
) {
    let ch = hits.register("card_tasks", rect, CursorKind::Arrow, None, input);
    draw_frost_card(d2d, rect, ch.hovered, None);

    let open_items: Vec<(usize, &TodoItem)> =
        todos.iter().enumerate().filter(|(_, t)| !t.done).collect();
    d2d.draw_text(
        &format!("Tasks ({})", open_items.len()),
        Rect::new(rect.x + 14.0, rect.y + 12.0, 120.0, 18.0),
        &TextStyle::default().size(11.5).semibold().color(Color::white(0.86)),
    );
    if draw_icon_btn(
        d2d,
        hits,
        "home_tasks_plus",
        Rect::new(rect.right() - 34.0, rect.y + 10.0, 22.0, 22.0),
        IconId::Plus,
        12.5,
        Color::white(0.70),
        Color::white(0.10),
        Some("Kelola Tasks"),
        input,
    ) {
        act.switch_tab = Some(ActiveTab::Desk);
    }

    if open_items.is_empty() {
        d2d.draw_text(
            "Semua task selesai.\nKlik + untuk tambah.",
            Rect::new(rect.x + 14.0, rect.y + 44.0, rect.w - 28.0, 50.0),
            &TextStyle::default().size(11.0).color(Color::white(0.38)),
        );
        return;
    }

    let mut y = rect.y + 36.0;
    for (orig_idx, item) in open_items.into_iter().take(4) {
        let row_r = Rect::new(rect.x + 10.0, y, rect.w - 20.0, 22.0);
        let rh = hits.register(
            format!("home_todo_{}", orig_idx),
            row_r,
            CursorKind::Pointer,
            None,
            input,
        );
        if rh.hovered {
            d2d.fill_rounded_rect(row_r, 6.0, Color::white(0.06));
        }
        let cx = row_r.x + 10.0;
        let cy = row_r.center_y();
        d2d.stroke_circle(
            cx,
            cy,
            6.0,
            1.25,
            if rh.hovered { accent } else { Color::white(0.35) },
        );
        d2d.draw_text(
            &item.label,
            Rect::new(row_r.x + 23.0, row_r.y, row_r.w - 26.0, row_r.h),
            &TextStyle::default()
                .size(11.0)
                .color(Color::white(0.90))
                .vcenter()
                .ellipsis(),
        );
        if rh.clicked {
            let mut next = todos.to_vec();
            if let Some(t) = next.get_mut(orig_idx) {
                t.done = true;
            }
            if let Ok(v) = serde_json::to_value(next) {
                store.set(Some(ui), "todos", v);
            }
        }
        y += 23.0;
    }
}

fn draw_status_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    usage: &LiveUsage,
    battery: &BatteryState,
    particles: &mut ParticleBuffers,
    input: &UiInput,
) {
    let ch = hits.register("card_status", rect, CursorKind::Arrow, None, input);
    draw_frost_card(d2d, rect, ch.hovered, None);

    d2d.draw_text(
        "Status",
        Rect::new(rect.x + 14.0, rect.y + 12.0, 90.0, 18.0),
        &TextStyle::default().size(11.5).semibold().color(Color::white(0.86)),
    );
    if battery.supported {
        let pct = (battery.level * 100.0).round() as u32;
        let bcol = if pct <= 20 && !battery.charging {
            Color::rgb(255, 69, 58)
        } else if battery.charging {
            Color::rgb(52, 211, 153)
        } else {
            Color::white(0.55)
        };
        d2d.draw_text(
            &format!("{}%{}", pct, if battery.charging { " ⚡" } else { "" }),
            Rect::new(rect.right() - 68.0, rect.y + 12.0, 54.0, 18.0),
            &TextStyle::default().size(10.5).semibold().color(bcol).right(),
        );
    }

    let t = input.now_ms as f32 / 1000.0;
    let tick = input.now_ms / 33;

    let cpu_r = Rect::new(rect.x + 22.0, rect.y + 38.0, 56.0, 56.0);
    particles.draw_liquid_sphere(d2d, "status_cpu", cpu_r, usage.cpu, [56, 189, 248], t, tick);
    d2d.draw_text(
        &format!("CPU {:.0}%", usage.cpu),
        Rect::new(cpu_r.x - 6.0, cpu_r.bottom() + 6.0, 68.0, 16.0),
        &TextStyle::default()
            .size(10.5)
            .medium()
            .color(Color::white(0.74))
            .center(),
    );

    let ram_r = Rect::new(rect.right() - 78.0, rect.y + 38.0, 56.0, 56.0);
    particles.draw_liquid_sphere(d2d, "status_ram", ram_r, usage.mem, [52, 211, 153], t + 1.3, tick);
    d2d.draw_text(
        &format!("RAM {:.0}%", usage.mem),
        Rect::new(ram_r.x - 6.0, ram_r.bottom() + 6.0, 68.0, 16.0),
        &TextStyle::default()
            .size(10.5)
            .medium()
            .color(Color::white(0.74))
            .center(),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_ai_usage_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    settings: &Settings,
    accent_rgb: [u8; 3],
    ai_limits: &[AiLimitItem],
    particles: &mut ParticleBuffers,
    input: &UiInput,
    act: &mut HomeAction,
) {
    let ch = hits.register("card_ai", rect, CursorKind::Pointer, None, input);
    draw_frost_card(d2d, rect, ch.hovered, None);
    if ch.clicked {
        act.switch_tab = Some(ActiveTab::Conversation);
    }

    let visible: Vec<&AiLimitItem> = ai_limits
        .iter()
        .filter(|l| !settings.hidden_limits.iter().any(|h| h == &l.id))
        .take(4)
        .collect();

    if visible.is_empty() {
        let ring_r = Rect::new(rect.center_x() - 25.0, rect.y + 22.0, 50.0, 50.0);
        let ready = !settings.dn_router_base_url.is_empty();
        particles.draw_quota_ring(
            d2d,
            "ai_ring_default",
            ring_r,
            if ready { 0.85 } else { 0.25 },
            false,
            accent_rgb,
            1,
        );
        d2d.draw_text(
            "9router AI",
            Rect::new(rect.x + 8.0, ring_r.bottom() + 8.0, rect.w - 16.0, 16.0),
            &TextStyle::default()
                .size(10.5)
                .semibold()
                .color(Color::white(0.86))
                .center()
                .ellipsis(),
        );
        let mod_name = if !settings.dn_router_model.is_empty() {
            settings
                .dn_router_model
                .rsplit('/')
                .next()
                .unwrap_or(&settings.dn_router_model)
        } else {
            "Claude Haiku"
        };
        d2d.draw_text(
            mod_name,
            Rect::new(rect.x + 8.0, ring_r.bottom() + 24.0, rect.w - 16.0, 14.0),
            &TextStyle::default()
                .size(9.5)
                .color(Color::white(0.45))
                .center()
                .ellipsis(),
        );
        return;
    }

    let mut x = rect.x + 12.0;
    for item in visible {
        let cell = Rect::new(x, rect.y + 12.0, 68.0, rect.h - 24.0);
        let ring_r = Rect::new(cell.center_x() - 24.0, cell.y + 6.0, 48.0, 48.0);
        let used = (item.used_pct / 100.0).clamp(0.0, 1.0);
        let warn = item.used_pct >= 80.0;
        let gen = (item.used_pct * 10.0) as u64 + if warn { 10000 } else { 0 };
        particles.draw_quota_ring(
            d2d,
            &format!("ai_ring_{}", item.id),
            ring_r,
            used,
            warn,
            accent_rgb,
            gen,
        );
        d2d.draw_text(
            &format!("{:.0}%", item.used_pct),
            ring_r,
            &TextStyle::default()
                .size(10.5)
                .bold()
                .color(Color::white(0.92))
                .center()
                .vcenter(),
        );
        d2d.draw_text(
            &item.label,
            Rect::new(cell.x, ring_r.bottom() + 6.0, cell.w, 16.0),
            &TextStyle::default()
                .size(10.0)
                .semibold()
                .color(Color::white(0.84))
                .center()
                .ellipsis(),
        );
        d2d.draw_text(
            if item.reset_label.is_empty() {
                &item.window
            } else {
                &item.reset_label
            },
            Rect::new(cell.x, ring_r.bottom() + 22.0, cell.w, 14.0),
            &TextStyle::default()
                .size(9.0)
                .color(Color::white(0.45))
                .center()
                .ellipsis(),
        );
        x += 72.0;
    }
}

fn draw_fallback_clock_card(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    rect: Rect,
    input: &UiInput,
    act: &mut HomeAction,
) {
    let ch = hits.register("card_clock", rect, CursorKind::Pointer, None, input);
    draw_frost_card(d2d, rect, ch.hovered, None);
    if ch.clicked {
        act.switch_tab = Some(ActiveTab::Settings);
    }
    let now = Local::now();
    let t = now.format("%H:%M").to_string();
    d2d.draw_text(
        &t,
        Rect::new(rect.x + 12.0, rect.y + 32.0, rect.w - 24.0, 38.0),
        &TextStyle::default()
            .size(28.0)
            .bold()
            .color(Color::white(0.95))
            .center()
            .vcenter(),
    );
    let d = format_tanggal_id(now.date_naive());
    d2d.draw_text(
        &d,
        Rect::new(rect.x + 12.0, rect.y + 76.0, rect.w - 24.0, 18.0),
        &TextStyle::default()
            .size(10.5)
            .color(Color::white(0.55))
            .center()
            .ellipsis(),
    );
}

fn format_time_12h(epoch_ms: i64) -> String {
    if let Some(dt) = DateTime::from_timestamp_millis(epoch_ms) {
        let local = dt.with_timezone(&Local);
        let raw = local.format("%I:%M %p").to_string();
        raw.strip_prefix('0').unwrap_or(&raw).to_string()
    } else {
        String::new()
    }
}

fn format_rem_countdown(at_ms: i64, now_ms: i64) -> String {
    let diff_mins = ((at_ms - now_ms) as f64 / 60_000.0).ceil() as i64;
    if diff_mins <= 0 {
        return "REMINDER · NOW".to_string();
    }
    let h = diff_mins / 60;
    let m = diff_mins % 60;
    if h > 0 {
        if m > 0 {
            format!("REMINDER · IN {} H {} MIN", h, m)
        } else {
            format!("REMINDER · IN {} H", h)
        }
    } else {
        format!("REMINDER · IN {} MIN", diff_mins)
    }
}

fn parse_hhmm_parts(raw: &str) -> Option<(u32, u32)> {
    let clean = raw.trim().replace('.', ":");
    if let Some((h_s, m_s)) = clean.split_once(':') {
        let hh: u32 = h_s.trim().parse().ok()?;
        let mm: u32 = m_s.trim().parse().ok()?;
        if hh < 24 && mm < 60 {
            return Some((hh, mm));
        }
        return None;
    }
    let digits: String = clean.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 4 {
        let hh: u32 = digits[..2].parse().ok()?;
        let mm: u32 = digits[2..].parse().ok()?;
        if hh < 24 && mm < 60 {
            return Some((hh, mm));
        }
    } else if digits.len() == 3 {
        let hh: u32 = digits[..1].parse().ok()?;
        let mm: u32 = digits[1..].parse().ok()?;
        if hh < 24 && mm < 60 {
            return Some((hh, mm));
        }
    }
    None
}

fn schedule_hhmm(raw: &str) -> Option<i64> {
    let (hh, mm) = parse_hhmm_parts(raw)?;
    let now = Local::now();
    let today = now.date_naive();
    let cand = today
        .and_hms_opt(hh, mm, 0)
        .and_then(|ndt| Local.from_local_datetime(&ndt).single())?;
    let target = if cand.timestamp_millis() <= now.timestamp_millis() {
        (today + ChronoDur::days(1))
            .and_hms_opt(hh, mm, 0)
            .and_then(|ndt| Local.from_local_datetime(&ndt).single())
            .unwrap_or(cand)
    } else {
        cand
    };
    Some(target.timestamp_millis())
}

fn push_reminder(store: &StoreManager, ui: &UiSender, reminders: &[ReminderItem], at_ms: i64, msg: String) {
    let mut next = reminders.to_vec();
    next.push(ReminderItem {
        id: format!("rem{}", now_unix_ms()),
        at: at_ms,
        message: msg,
    });
    next.sort_by_key(|r| r.at);
    if let Ok(v) = serde_json::to_value(next) {
        store.set(Some(ui), "reminders", v);
    }
}

fn draw_white_pill_btn(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    rect: Rect,
    label: &str,
    enabled: bool,
    text_size: f32,
    input: &UiInput,
) -> bool {
    let h = hits.register(
        id,
        rect,
        if enabled { CursorKind::Pointer } else { CursorKind::Arrow },
        None,
        input,
    );
    let bg = if !enabled {
        Color::white(0.08)
    } else if h.hovered {
        Color::white(1.0)
    } else {
        Color::rgb(244, 244, 245)
    };
    let fg = if !enabled {
        Color::white(0.38)
    } else {
        Color::rgb(11, 11, 13)
    };
    let rad = rect.h * 0.5;
    d2d.fill_rounded_rect(rect, rad, bg);
    d2d.draw_text(
        label,
        rect,
        &TextStyle::default()
            .size(text_size)
            .bold()
            .color(fg)
            .center()
            .vcenter(),
    );
    enabled && h.clicked
}

/// Focus & Tasks full tab (`720 x 188`) matching DeskNotch Photos 2–7.
#[allow(clippy::too_many_arguments)]
pub fn draw_desk(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    custom_photo: &str,
    accent: Color,
    accent_rgb: [u8; 3],
    todos: &[TodoItem],
    reminders: &[ReminderItem],
    _focus_log: &[FocusLogEntry],
    focus_running: bool,
    focus_remaining_sec: u32,
    focus_total_sec: u32,
    companion_sleeping: bool,
    state: &mut HomeTabState,
    comp_renderer: &mut CompanionRenderer,
    comp_state: &mut CompanionState,
    particles: &mut ParticleBuffers,
    store: &StoreManager,
    ui: &UiSender,
    input: &UiInput,
) -> HomeAction {
    let mut act = HomeAction::default();
    let y = notch_rect.y + 40.0;
    let h = 136.0;

    let drawer_open = match state.desk_mode {
        DeskCardMode::Focus => state.focus_drawer_open,
        DeskCardMode::Reminder => state.rem_drawer_open,
    };
    let target_w = if drawer_open { 464.0 } else { 244.0 };
    if (state.card_w_anim - target_w).abs() < 0.5 {
        state.card_w_anim = target_w;
    } else {
        let f = (1.0 - (-18.0 * input.dt).exp()).clamp(0.12, 1.0);
        state.card_w_anim += (target_w - state.card_w_anim) * f;
    }

    let timer_active = focus_running || focus_remaining_sec < focus_total_sec;

    // Left Companion Focus/Reminder frost card
    let left = Rect::new(notch_rect.x + 14.0, y, state.card_w_anim, h);
    draw_frost_card(d2d, left, false, None);
    let comp_r = Rect::new(left.x + 12.0, left.y + 18.0, 84.0, 100.0);
    d2d.push_rounded_clip(left, 24.0);
    d2d.fill_radial_glow(
        comp_r.center_x(),
        left.center_y(),
        68.0,
        66.0,
        Color::white(0.085),
        Color::white(0.0),
    );
    if state.desk_mode == DeskCardMode::Focus && timer_active {
        let prog = if focus_total_sec > 0 {
            let p = (1.0 - (focus_remaining_sec as f32 / focus_total_sec as f32)).clamp(0.0, 1.0);
            if focus_running && p < 0.015 { 0.015 } else { p }
        } else {
            0.0
        };
        let ring_r = Rect::new(comp_r.center_x() - 50.0, comp_r.center_y() - 50.0, 100.0, 100.0);
        let tick = if focus_running {
            input.now_ms / 16
        } else {
            (focus_remaining_sec as u64).saturating_mul(100) + 1
        };
        particles.draw_focus_ring(
            d2d,
            "desk_focus_ring",
            ring_r,
            prog,
            accent_rgb,
            input.now_ms as f32 / 1000.0,
            focus_running,
            tick,
        );
    }
    d2d.pop_layer();

    comp_state.mood = if state.desk_mode == DeskCardMode::Focus && focus_running {
        CompanionMood::Focus
    } else if companion_sleeping || settings.companion_mode == "sleep" {
        CompanionMood::Sleep
    } else {
        CompanionMood::Idle
    };
    let comp_clicked = comp_renderer.draw(
        d2d,
        hits,
        "desk_comp",
        comp_r,
        68.0,
        &settings.avatar,
        settings.accent_hex(),
        custom_photo,
        comp_state,
        input,
    );
    if comp_clicked {
        act.woke_companion = true;
    }
    CompanionRenderer::cycle_avatar_on_click(comp_clicked, settings, custom_photo, store, ui);

    let cx = left.x + 108.0;
    let pill_w = if state.desk_mode == DeskCardMode::Reminder && drawer_open {
        118.0
    } else {
        122.0
    };
    let pill_r = Rect::new(cx, left.y + 14.0, pill_w, 20.0);
    let pill_lbl = match state.desk_mode {
        DeskCardMode::Focus => "FOCUS",
        DeskCardMode::Reminder => "REMINDER",
    };
    if draw_white_pill_btn(
        d2d,
        hits,
        "desk_mode_toggle",
        pill_r,
        pill_lbl,
        true,
        9.5,
        input,
    ) {
        state.desk_mode = match state.desk_mode {
            DeskCardMode::Focus => DeskCardMode::Reminder,
            DeskCardMode::Reminder => DeskCardMode::Focus,
        };
        state.focus_drawer_open = false;
        state.rem_drawer_open = false;
        state.time_picker_open = false;
    }

    match state.desk_mode {
        DeskCardMode::Focus => {
            let mm = focus_remaining_sec / 60;
            let ss = focus_remaining_sec % 60;
            let t_str = format!("{}:{:02}", mm, ss);
            let t_style = TextStyle::default()
                .size(25.0)
                .bold()
                .color(if timer_active { accent } else { Color::white(0.96) })
                .vcenter()
                .nowrap();
            let (tw, _) = d2d.measure_text(&t_str, 120.0, &t_style);
            let time_r = Rect::new(cx, left.y + 42.0, (tw + 26.0).min(126.0), 34.0);
            let th = hits.register(
                "desk_focus_time_click",
                time_r,
                CursorKind::Pointer,
                Some("Atur durasi fokus".to_string()),
                input,
            );
            d2d.draw_text(&t_str, time_r, &t_style);
            let chev_r = Rect::new(cx + tw + 6.0, time_r.center_y() - 6.0, 12.0, 12.0);
            d2d.draw_icon(
                if state.focus_drawer_open {
                    IconId::ChevronLeft
                } else {
                    IconId::ChevronRight
                },
                chev_r,
                Color::white(if th.hovered { 0.90 } else { 0.45 }),
                2.0,
                1.0,
            );
            if th.clicked {
                state.focus_drawer_open = !state.focus_drawer_open;
            }

            let by = left.y + 88.0;
            if draw_white_pill_btn(
                d2d,
                hits,
                "desk_focus_start",
                Rect::new(cx, by, 66.0, 26.0),
                if focus_running { "Pause" } else { "Start" },
                true,
                11.5,
                input,
            ) {
                act.toggle_focus = true;
                if !focus_running {
                    state.focus_drawer_open = false;
                }
            }
            if (focus_running || focus_remaining_sec != focus_total_sec)
                && draw_icon_btn(
                    d2d,
                    hits,
                    "desk_focus_reset",
                    Rect::new(cx + 72.0, by, 26.0, 26.0),
                    IconId::Refresh,
                    12.5,
                    Color::white(0.70),
                    Color::white(0.10),
                    Some("Reset timer"),
                    input,
                )
            {
                act.reset_focus = true;
            }

            // Expanded LENGTH drawer (Photo 3)
            if state.focus_drawer_open && left.w > 360.0 {
                let div_x = left.x + 238.0;
                d2d.draw_line(
                    div_x,
                    left.y + 12.0,
                    div_x,
                    left.bottom() - 12.0,
                    1.0,
                    Color::white(0.08),
                );
                let dx = div_x + 12.0;
                d2d.draw_text(
                    "LENGTH",
                    Rect::new(dx, left.y + 13.0, 120.0, 14.0),
                    &TextStyle::default()
                        .size(9.0)
                        .bold()
                        .color(Color::white(0.42)),
                );

                let presets_rows = [[1u32, 2, 5, 10, 15], [20, 25, 30, 45, 60]];
                for (ri, row) in presets_rows.iter().enumerate() {
                    let py = left.y + 30.0 + ri as f32 * 26.0;
                    for (ci, &mins) in row.iter().enumerate() {
                        let px = dx + ci as f32 * 40.5;
                        let pr = Rect::new(px, py, 36.0, 22.0);
                        let active = focus_total_sec == mins * 60;
                        let ph = hits.register(
                            format!("desk_len_p_{}", mins),
                            pr,
                            CursorKind::Pointer,
                            None,
                            input,
                        );
                        let bg = if active {
                            Color::rgb(244, 244, 245)
                        } else if ph.hovered {
                            Color::white(0.14)
                        } else {
                            Color::white(0.08)
                        };
                        let fg = if active {
                            Color::rgb(11, 11, 13)
                        } else {
                            Color::white(0.78)
                        };
                        d2d.fill_rounded_rect(pr, 11.0, bg);
                        d2d.draw_text(
                            &mins.to_string(),
                            pr,
                            &TextStyle::default()
                                .size(11.0)
                                .bold()
                                .color(fg)
                                .center()
                                .vcenter(),
                        );
                        if ph.clicked {
                            act.set_focus_seconds = Some(mins * 60);
                        }
                    }
                }

                // Custom min : sec + Set row
                let iy = left.y + 92.0;
                let num_box_style = TextInputStyle {
                    bg: Color::white(0.075),
                    hover_bg: Color::white(0.10),
                    focus_bg: Color::white(0.12),
                    border: Color::white(0.06),
                    focus_border: Color::white(0.28),
                    radius: 8.0,
                    pad_left: 6.0,
                    pad_right: 6.0,
                };
                let num_text_style = TextStyle::default()
                    .size(11.5)
                    .color(Color::white(0.92))
                    .center()
                    .vcenter()
                    .nowrap();

                let ma = state.min_input.draw_styled(
                    d2d,
                    hits,
                    "desk_custom_min",
                    Rect::new(dx, iy, 48.0, 26.0),
                    "min",
                    false,
                    Some(num_text_style.clone()),
                    &num_box_style,
                    input,
                );
                if ma.changed {
                    state.min_input.text.retain(|c| c.is_ascii_digit());
                    if state.min_input.text.len() > 3 {
                        state.min_input.text.truncate(3);
                    }
                    state.min_input.cursor = state.min_input.cursor.min(state.min_input.text.len());
                }

                d2d.draw_text(
                    ":",
                    Rect::new(dx + 49.0, iy, 10.0, 26.0),
                    &TextStyle::default()
                        .size(12.0)
                        .bold()
                        .color(Color::white(0.45))
                        .center()
                        .vcenter(),
                );

                let sa = state.sec_input.draw_styled(
                    d2d,
                    hits,
                    "desk_custom_sec",
                    Rect::new(dx + 60.0, iy, 48.0, 26.0),
                    "sec",
                    false,
                    Some(num_text_style),
                    &num_box_style,
                    input,
                );
                if sa.changed {
                    state.sec_input.text.retain(|c| c.is_ascii_digit());
                    if state.sec_input.text.len() > 2 {
                        state.sec_input.text.truncate(2);
                    }
                    state.sec_input.cursor = state.sec_input.cursor.min(state.sec_input.text.len());
                }

                let m_val: u32 = state.min_input.text.parse().unwrap_or(0);
                let s_val: u32 = state.sec_input.text.parse().unwrap_or(0);
                let custom_total = m_val.saturating_mul(60).saturating_add(s_val.min(59));
                let can_set = custom_total > 0;
                let set_clicked = draw_white_pill_btn(
                    d2d,
                    hits,
                    "desk_len_set",
                    Rect::new(left.right() - 62.0, iy, 50.0, 26.0),
                    "Set",
                    can_set,
                    11.0,
                    input,
                );
                if (set_clicked || ((ma.submitted || sa.submitted) && can_set)) && can_set {
                    act.set_focus_seconds = Some(custom_total.clamp(1, 10_800));
                    state.min_input.clear();
                    state.sec_input.clear();
                    state.focus_drawer_open = false;
                }
            }
        }

        DeskCardMode::Reminder => {
            let now_ms = now_unix_ms();
            let mut sorted_rems: Vec<&ReminderItem> =
                reminders.iter().filter(|r| !r.id.starts_with("cal")).collect();
            sorted_rems.sort_by_key(|r| r.at);

            if !state.rem_drawer_open {
                // Collapsed Reminder View (Photos 4 & 6)
                let sum_r = Rect::new(cx, left.y + 40.0, 130.0, 84.0);
                let sh = hits.register(
                    "desk_rem_open_drawer",
                    sum_r,
                    CursorKind::Pointer,
                    Some("Atur pengingat".to_string()),
                    input,
                );
                if let Some(first) = sorted_rems.first() {
                    let hdr = format_rem_countdown(first.at, now_ms);
                    d2d.draw_text(
                        &hdr,
                        Rect::new(cx, left.y + 42.0, 128.0, 24.0),
                        &TextStyle::default()
                            .size(9.0)
                            .bold()
                            .color(if first.at <= now_ms {
                                Color::rgb(251, 191, 36)
                            } else {
                                Color::white(0.45)
                            }),
                    );
                    d2d.draw_text(
                        &format_time_12h(first.at),
                        Rect::new(cx, left.y + 66.0, 128.0, 28.0),
                        &TextStyle::default()
                            .size(21.0)
                            .bold()
                            .color(Color::white(0.96))
                            .vcenter()
                            .nowrap(),
                    );
                    let sub_msg = if !first.message.trim().is_empty() {
                        first.message.clone()
                    } else if sorted_rems.len() > 1 {
                        format!("+{} more", sorted_rems.len() - 1)
                    } else {
                        "No message".to_string()
                    };
                    d2d.draw_text(
                        &sub_msg,
                        Rect::new(cx, left.y + 98.0, 128.0, 20.0),
                        &TextStyle::default()
                            .size(11.5)
                            .color(Color::white(0.58))
                            .ellipsis(),
                    );
                } else {
                    d2d.draw_text(
                        "REMINDER",
                        Rect::new(cx, left.y + 44.0, 128.0, 14.0),
                        &TextStyle::default()
                            .size(9.0)
                            .bold()
                            .color(Color::white(0.42)),
                    );
                    d2d.draw_icon(
                        IconId::Bell,
                        Rect::new(cx, left.y + 68.0, 14.0, 14.0),
                        accent,
                        2.0,
                        1.0,
                    );
                    d2d.draw_text(
                        "Tap to set a\nreminder",
                        Rect::new(cx + 21.0, left.y + 62.0, 108.0, 36.0),
                        &TextStyle::default()
                            .size(11.5)
                            .color(Color::white(if sh.hovered { 0.85 } else { 0.50 }))
                            .line_height(15.0),
                    );
                }
                if sh.clicked {
                    state.rem_drawer_open = true;
                    if state.rem_time_input.text.trim().is_empty() {
                        let def_t = (Local::now() + ChronoDur::minutes(10))
                            .format("%H:%M")
                            .to_string();
                        state.rem_time_input.set_text(def_t);
                    }
                }
            } else if left.w > 430.0 {
                // Expanded Reminder List + Form (Photos 5 & 7)
                let hdr_lbl = if sorted_rems.is_empty() {
                    "REMINDERS".to_string()
                } else {
                    format!("REMINDERS · {}", sorted_rems.len())
                };
                d2d.draw_text(
                    &hdr_lbl,
                    Rect::new(cx, left.y + 41.0, 110.0, 14.0),
                    &TextStyle::default()
                        .size(9.0)
                        .bold()
                        .color(Color::white(0.42)),
                );
                if draw_icon_btn(
                    d2d,
                    hits,
                    "desk_rem_collapse",
                    Rect::new(left.x + 216.0, left.y + 39.0, 18.0, 18.0),
                    IconId::ChevronLeft,
                    11.0,
                    Color::white(0.55),
                    Color::white(0.08),
                    Some("Tutup panel reminder"),
                    input,
                ) {
                    state.rem_drawer_open = false;
                    state.time_picker_open = false;
                }

                let list_area = Rect::new(cx, left.y + 58.0, 128.0, 68.0);
                if sorted_rems.is_empty() {
                    d2d.draw_text(
                        "None yet. Pick a time\non the right.",
                        list_area,
                        &TextStyle::default()
                            .size(11.0)
                            .color(Color::white(0.42))
                            .line_height(14.5),
                    );
                } else {
                    let row_h = 30.0;
                    state
                        .rem_list_scroll
                        .update_bounds(list_area.h, sorted_rems.len() as f32 * row_h, input);
                    state.rem_list_scroll.handle_wheel(list_area, false, input);
                    d2d.push_clip_rect(list_area);
                    let mut ry = list_area.y - state.rem_list_scroll.offset;
                    for (idx, rem) in sorted_rems.iter().enumerate() {
                        let rr = Rect::new(list_area.x, ry, list_area.w - 4.0, row_h - 2.0);
                        if rr.bottom() >= list_area.y && rr.y <= list_area.bottom() {
                            let row_hovered = input.mouse_inside
                                && list_area.contains(input.mouse_x, input.mouse_y)
                                && rr.contains(input.mouse_x, input.mouse_y);
                            let t_lbl = if rem.at <= now_ms {
                                format!("Now · {}", format_time_12h(rem.at))
                            } else {
                                format_time_12h(rem.at)
                            };
                            d2d.draw_text(
                                &t_lbl,
                                Rect::new(rr.x, rr.y, rr.w - 18.0, 15.0),
                                &TextStyle::default()
                                    .size(11.0)
                                    .bold()
                                    .color(if rem.at <= now_ms {
                                        accent
                                    } else {
                                        Color::white(0.94)
                                    })
                                    .ellipsis(),
                            );
                            if !rem.message.is_empty() {
                                d2d.draw_text(
                                    &rem.message,
                                    Rect::new(rr.x, rr.y + 14.0, rr.w - 18.0, 14.0),
                                    &TextStyle::default()
                                        .size(10.0)
                                        .color(Color::white(0.55))
                                        .ellipsis(),
                                );
                            }
                            let del_r = Rect::new(rr.right() - 16.0, rr.y + 2.0, 16.0, 16.0);
                            if row_hovered
                                && draw_icon_btn(
                                    d2d,
                                    hits,
                                    &format!("desk_rem_del_{}", idx),
                                    del_r,
                                    IconId::Close,
                                    10.0,
                                    Color::white(0.55),
                                    Color::white(0.10),
                                    Some("Hapus reminder"),
                                    input,
                                )
                            {
                                let next: Vec<ReminderItem> = reminders
                                    .iter()
                                    .filter(|r| r.id != rem.id)
                                    .cloned()
                                    .collect();
                                if let Ok(v) = serde_json::to_value(next) {
                                    store.set(Some(ui), "reminders", v);
                                }
                            }
                        }
                        ry += row_h;
                    }
                    d2d.pop_clip_rect();
                }

                // Right NEW REMINDER form
                let div_x = left.x + 238.0;
                d2d.draw_line(
                    div_x,
                    left.y + 12.0,
                    div_x,
                    left.bottom() - 12.0,
                    1.0,
                    Color::white(0.08),
                );
                let dx = div_x + 12.0;
                let dw = left.right() - dx - 12.0;
                d2d.draw_text(
                    "NEW REMINDER",
                    Rect::new(dx, left.y + 13.0, dw, 14.0),
                    &TextStyle::default()
                        .size(9.0)
                        .bold()
                        .color(Color::white(0.42)),
                );

                let f_style = TextInputStyle {
                    bg: Color::white(0.075),
                    hover_bg: Color::white(0.10),
                    focus_bg: Color::white(0.12),
                    border: Color::white(0.05),
                    focus_border: Color::white(0.28),
                    radius: 8.0,
                    pad_left: 9.0,
                    pad_right: 24.0,
                };
                let time_r = Rect::new(dx, left.y + 30.0, dw - 54.0, 26.0);
                state.tp_anchor = time_r;
                let ta = state.rem_time_input.draw_styled(
                    d2d,
                    hits,
                    "desk_rem_time",
                    time_r,
                    "07:38",
                    false,
                    Some(
                        TextStyle::default()
                            .size(12.0)
                            .semibold()
                            .color(Color::white(0.95))
                            .vcenter()
                            .nowrap(),
                    ),
                    &f_style,
                    input,
                );
                if ta.changed {
                    let clean: String = state
                        .rem_time_input
                        .text
                        .chars()
                        .filter(|c| c.is_ascii_digit() || *c == ':' || *c == '.')
                        .take(5)
                        .collect();
                    if clean != state.rem_time_input.text {
                        state.rem_time_input.set_text(clean);
                    }
                    state.rem_time_input.scroll_x = 0.0;
                }
                let clk_r = Rect::new(time_r.right() - 22.0, time_r.y + 2.0, 20.0, 22.0);
                if draw_icon_btn(
                    d2d,
                    hits,
                    "desk_rem_clock_pick",
                    clk_r,
                    IconId::Clock,
                    12.5,
                    if state.time_picker_open {
                        Color::white(0.98)
                    } else {
                        Color::white(0.60)
                    },
                    Color::white(0.12),
                    Some("Pilih jam"),
                    input,
                ) {
                    state.time_picker_open = !state.time_picker_open;
                    if state.time_picker_open {
                        let (sh, sm) = parse_hhmm_parts(&state.rem_time_input.text).unwrap_or((7, 0));
                        state.tp_hr_scroll.target = (sh as f32 * 24.0 - 60.0).max(0.0);
                        state.tp_min_scroll.target = (sm as f32 * 24.0 - 60.0).max(0.0);
                    }
                }

                let parsed_at = schedule_hhmm(&state.rem_time_input.text);
                let set_rem_clicked = draw_white_pill_btn(
                    d2d,
                    hits,
                    "desk_rem_set",
                    Rect::new(left.right() - 58.0, left.y + 30.0, 46.0, 26.0),
                    "Set",
                    parsed_at.is_some(),
                    11.0,
                    input,
                );

                let msg_style = TextInputStyle {
                    pad_right: 9.0,
                    ..f_style
                };
                let ma = state.rem_msg_input.draw_styled(
                    d2d,
                    hits,
                    "desk_rem_msg",
                    Rect::new(dx, left.y + 62.0, dw, 26.0),
                    "Message (optional)",
                    false,
                    Some(
                        TextStyle::default()
                            .size(11.5)
                            .color(Color::white(0.90))
                            .vcenter()
                            .nowrap(),
                    ),
                    &msg_style,
                    input,
                );

                if (set_rem_clicked || ta.submitted || ma.submitted) && parsed_at.is_some() {
                    if let Some(at_ms) = parsed_at {
                        let msg = state.rem_msg_input.text.trim().to_string();
                        push_reminder(store, ui, reminders, at_ms, msg);
                        state.rem_msg_input.clear();
                        state.time_picker_open = false;
                        state.rem_drawer_open = false;
                    }
                }

                let py = left.y + 94.0;
                let pw = (dw - 10.0) / 3.0;
                for (pi, (lbl, add_ms)) in [
                    ("10 min", 600_000i64),
                    ("30 min", 1_800_000),
                    ("1 hour", 3_600_000),
                ]
                .iter()
                .enumerate()
                {
                    let pr = Rect::new(dx + pi as f32 * (pw + 5.0), py, pw, 24.0);
                    let ph = hits.register(
                        format!("desk_rem_preset_{}", pi),
                        pr,
                        CursorKind::Pointer,
                        None,
                        input,
                    );
                    d2d.fill_rounded_rect(
                        pr,
                        12.0,
                        Color::white(if ph.hovered { 0.14 } else { 0.08 }),
                    );
                    d2d.draw_text(
                        lbl,
                        pr,
                        &TextStyle::default()
                            .size(10.5)
                            .semibold()
                            .color(Color::white(0.78))
                            .center()
                            .vcenter(),
                    );
                    if ph.clicked {
                        let msg = state.rem_msg_input.text.trim().to_string();
                        push_reminder(store, ui, reminders, now_ms + *add_ms, msg);
                        state.rem_msg_input.clear();
                        state.time_picker_open = false;
                        state.rem_drawer_open = false;
                    }
                }
            }
        }
    }

    // Right minimalist Tasks column directly on #0b0b0d (Photos 2-7)
    let tx = left.right() + 18.0;
    let tw = (notch_rect.right() - tx - 16.0).max(160.0);

    let open_items: Vec<(usize, &TodoItem)> =
        todos.iter().enumerate().filter(|(_, t)| !t.done).collect();
    let done_count = todos.len().saturating_sub(open_items.len());

    // Add task line
    let inp_row = Rect::new(tx, y + 2.0, tw, 30.0);
    d2d.draw_icon(
        IconId::Plus,
        Rect::new(inp_row.x, inp_row.center_y() - 6.5, 13.0, 13.0),
        Color::white(0.38),
        2.0,
        1.0,
    );
    let ta = state.todo_input.draw_styled(
        d2d,
        hits,
        "desk_add_todo",
        Rect::new(inp_row.x + 20.0, inp_row.y, (tw - 46.0).max(60.0), inp_row.h),
        "What needs doing",
        false,
        Some(
            TextStyle::default()
                .size(12.5)
                .color(Color::white(0.92))
                .vcenter()
                .nowrap(),
        ),
        &TextInputStyle::borderless(),
        input,
    );
    if !open_items.is_empty() {
        d2d.draw_text(
            &open_items.len().to_string(),
            Rect::new(inp_row.right() - 24.0, inp_row.y, 24.0, inp_row.h),
            &TextStyle::default()
                .size(11.0)
                .color(Color::white(0.35))
                .right()
                .vcenter(),
        );
    }
    d2d.draw_line(
        inp_row.x,
        inp_row.bottom() + 1.0,
        inp_row.right(),
        inp_row.bottom() + 1.0,
        1.0,
        Color::white(if state.todo_input.focused { 0.38 } else { 0.09 }),
    );
    if ta.submitted && !state.todo_input.text.trim().is_empty() {
        let mut next = todos.to_vec();
        next.push(TodoItem {
            id: format!("t{}", now_unix_ms()),
            label: state.todo_input.text.trim().to_string(),
            done: false,
        });
        state.todo_input.clear();
        if let Ok(v) = serde_json::to_value(next) {
            store.set(Some(ui), "todos", v);
        }
    }

    let footer_h = if done_count > 0 { 28.0 } else { 0.0 };
    let list_r = Rect::new(
        tx,
        inp_row.bottom() + 6.0,
        tw,
        (notch_rect.bottom() - inp_row.bottom() - 16.0 - footer_h).max(28.0),
    );
    let content_h = open_items.len() as f32 * 28.0;
    state.todo_scroll.update_bounds(list_r.h, content_h, input);
    state.todo_scroll.handle_wheel(list_r, false, input);

    d2d.push_clip_rect(list_r);
    let mut ty = list_r.y - state.todo_scroll.offset;
    for (orig_idx, item) in open_items {
        let row = Rect::new(list_r.x, ty, list_r.w - 6.0, 26.0);
        if row.bottom() >= list_r.y && row.y <= list_r.bottom() {
            let rh = hits.register(
                format!("desk_todo_row_{}", orig_idx),
                row,
                CursorKind::Pointer,
                None,
                input,
            );
            d2d.stroke_circle(
                row.x + 7.5,
                row.center_y(),
                7.0,
                1.3,
                if rh.hovered { accent } else { Color::white(0.32) },
            );
            d2d.draw_text(
                &item.label,
                Rect::new(row.x + 24.0, row.y, row.w - 28.0, row.h),
                &TextStyle::default()
                    .size(12.5)
                    .color(Color::white(if rh.hovered { 0.98 } else { 0.88 }))
                    .vcenter()
                    .ellipsis(),
            );
            if rh.clicked {
                let mut next = todos.to_vec();
                if let Some(t) = next.get_mut(orig_idx) {
                    t.done = true;
                }
                if let Ok(v) = serde_json::to_value(next) {
                    store.set(Some(ui), "todos", v);
                }
            }
        }
        ty += 28.0;
    }
    d2d.pop_clip_rect();
    state.todo_scroll.draw_v_bar(d2d, list_r, input);

    if done_count > 0 {
        let fy = notch_rect.bottom() - 34.0;
        d2d.draw_line(tx, fy, tx + tw, fy, 1.0, Color::white(0.07));
        let done_lbl = format!("{} done", done_count);
        let done_st = TextStyle::default().size(11.0).color(Color::white(0.38)).vcenter();
        let (dw, _) = d2d.measure_text(&done_lbl, 80.0, &done_st);
        d2d.draw_text(&done_lbl, Rect::new(tx, fy + 4.0, dw + 4.0, 22.0), &done_st);

        let clr_r = Rect::new(tx + dw + 14.0, fy + 4.0, 38.0, 22.0);
        let ch = hits.register("desk_clear_done", clr_r, CursorKind::Pointer, None, input);
        d2d.draw_text(
            "clear",
            clr_r,
            &TextStyle::default()
                .size(11.0)
                .color(Color::white(if ch.hovered { 0.92 } else { 0.45 }))
                .vcenter(),
        );
        if ch.clicked {
            let next: Vec<TodoItem> = todos.iter().filter(|t| !t.done).cloned().collect();
            if let Ok(v) = serde_json::to_value(next) {
                store.set(Some(ui), "todos", v);
            }
        }
    }

    act
}

/// Floating HH | MM time-picker popover drawn outside notch clip (Photo 7).
pub fn draw_desk_time_picker(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    state: &mut HomeTabState,
    input: &UiInput,
) {
    if state.desk_mode != DeskCardMode::Reminder
        || !state.rem_drawer_open
        || !state.time_picker_open
        || state.tp_anchor.w <= 0.0
    {
        return;
    }

    let pop = Rect::new(state.tp_anchor.x, state.tp_anchor.bottom() + 4.0, 114.0, 168.0);
    hits.add_shell_rect(pop);
    let _bg_hit = hits.register("desk_tp_bg", pop, CursorKind::Arrow, None, input);
    d2d.fill_rounded_rect(pop, 8.0, Color::rgb(42, 42, 48));
    d2d.stroke_rounded_rect(pop, 8.0, 1.0, Color::white(0.18));

    let (mut sel_h, mut sel_m) = parse_hhmm_parts(&state.rem_time_input.text).unwrap_or((7, 0));

    let hr_col = Rect::new(pop.x + 6.0, pop.y + 6.0, 48.0, pop.h - 12.0);
    let min_col = Rect::new(hr_col.right() + 6.0, pop.y + 6.0, 48.0, pop.h - 12.0);
    let item_h = 24.0;

    state.tp_hr_scroll.update_bounds(hr_col.h, 24.0 * item_h, input);
    state.tp_hr_scroll.handle_wheel(hr_col, false, input);
    d2d.push_clip_rect(hr_col);
    for h in 0..24u32 {
        let rr = Rect::new(
            hr_col.x,
            hr_col.y + h as f32 * item_h - state.tp_hr_scroll.offset,
            hr_col.w,
            item_h - 2.0,
        );
        if rr.bottom() < hr_col.y || rr.y > hr_col.bottom() {
            continue;
        }
        let rh = hits.register(format!("desk_tp_h_{}", h), rr, CursorKind::Pointer, None, input);
        let selected = h == sel_h;
        if selected {
            d2d.fill_rounded_rect(rr, 4.0, Color::rgb(147, 197, 253));
        } else if rh.hovered {
            d2d.fill_rounded_rect(rr, 4.0, Color::white(0.10));
        }
        d2d.draw_text(
            &format!("{:02}", h),
            rr,
            &TextStyle::default()
                .size(12.0)
                .bold()
                .color(if selected {
                    Color::rgb(11, 11, 13)
                } else {
                    Color::white(0.88)
                })
                .center()
                .vcenter(),
        );
        if rh.clicked {
            sel_h = h;
            state.rem_time_input.set_text(format!("{:02}:{:02}", sel_h, sel_m));
        }
    }
    d2d.pop_clip_rect();

    state.tp_min_scroll.update_bounds(min_col.h, 60.0 * item_h, input);
    state.tp_min_scroll.handle_wheel(min_col, false, input);
    d2d.push_clip_rect(min_col);
    for m in 0..60u32 {
        let rr = Rect::new(
            min_col.x,
            min_col.y + m as f32 * item_h - state.tp_min_scroll.offset,
            min_col.w,
            item_h - 2.0,
        );
        if rr.bottom() < min_col.y || rr.y > min_col.bottom() {
            continue;
        }
        let rh = hits.register(format!("desk_tp_m_{}", m), rr, CursorKind::Pointer, None, input);
        let selected = m == sel_m;
        if selected {
            d2d.fill_rounded_rect(rr, 4.0, Color::rgb(228, 228, 231));
        } else if rh.hovered {
            d2d.fill_rounded_rect(rr, 4.0, Color::white(0.10));
        }
        d2d.draw_text(
            &format!("{:02}", m),
            rr,
            &TextStyle::default()
                .size(12.0)
                .semibold()
                .color(if selected {
                    Color::rgb(11, 11, 13)
                } else {
                    Color::white(0.88)
                })
                .center()
                .vcenter(),
        );
        if rh.clicked {
            sel_m = m;
            state.rem_time_input.set_text(format!("{:02}:{:02}", sel_h, sel_m));
        }
    }
    d2d.pop_clip_rect();

    // Close picker when user clicks outside popover and not directly on the clock icon
    if input.left_pressed
        && !pop.contains(input.mouse_x, input.mouse_y)
        && !state.tp_anchor.contains(input.mouse_x, input.mouse_y)
    {
        state.time_picker_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::indonesian_greeting;

    #[test]
    fn indonesian_greeting_covers_all_24_hours() {
        for h in 0..5 {
            assert_eq!(indonesian_greeting(h), "Selamat malam", "hour {}", h);
        }
        for h in 5..11 {
            assert_eq!(indonesian_greeting(h), "Selamat pagi", "hour {}", h);
        }
        for h in 11..15 {
            assert_eq!(indonesian_greeting(h), "Selamat siang", "hour {}", h);
        }
        for h in 15..18 {
            assert_eq!(indonesian_greeting(h), "Selamat sore", "hour {}", h);
        }
        for h in 18..24 {
            assert_eq!(indonesian_greeting(h), "Selamat malam", "hour {}", h);
        }
    }
}
