use crate::backend::display;
use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::backend::updater::{self, UpdateReleaseInfo};
use crate::events::{DisplayInfo, HwndHandle, UiEvent, UiSender};
use crate::logic::router;
use crate::state::{avatar_hex, Settings};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{
    draw_pill_btn, draw_segmented, draw_switch_visual, CursorKind, HitRegistry, ScrollState,
    TextInputState, UiInput, VK_ESCAPE,
};
use crate::voice::{
    downloader::{self, WhisperBackendKind},
    hotkey::{self, HotkeyBindings},
    whisper, VoiceManager,
};
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Notch,
    Companion,
    Voice,
    Layout,
    General,
}

pub struct SettingsTabState {
    pub section: SettingsSection,
    pub scroll: ScrollState,
    pub displays: Vec<DisplayInfo>,
    pub router_url_inp: TextInputState,
    pub router_key_inp: TextInputState,
    pub router_model_inp: TextInputState,
    pub gemini_key_inp: TextInputState,
    pub gemini_model_inp: TextInputState,
    pub openai_url_inp: TextInputState,
    pub openai_key_inp: TextInputState,
    pub openai_model_inp: TextInputState,
    pub router_loaded: bool,
    pub router_models: Vec<String>,
    pub router_msg: String,
    pub mic_test_msg: String,
    pub mic_testing: bool,
    pub whisper_dl_active: bool,
    pub whisper_dl_stage: String,
    pub whisper_dl_pct: f32,
    pub whisper_dl_mb: f32,
    pub whisper_dl_total_mb: f32,
    pub whisper_dl_msg: String,
    pub recording_hotkey: Option<usize>,
    pub hotkey_err: String,
    pub voice_dropdown_open: bool,
    pub update_checking: bool,
    pub update_downloading: bool,
    pub update_status: String,
    pub update_info: Option<UpdateReleaseInfo>,
    pub measured_h: [f32; 5],
    pub nav_indicator_y: f32,
}

impl Default for SettingsTabState {
    fn default() -> Self {
        Self {
            section: SettingsSection::Notch,
            scroll: ScrollState::default(),
            displays: Vec::new(),
            router_url_inp: TextInputState::default(),
            router_key_inp: TextInputState::default(),
            router_model_inp: TextInputState::default(),
            gemini_key_inp: TextInputState::default(),
            gemini_model_inp: TextInputState::default(),
            openai_url_inp: TextInputState::default(),
            openai_key_inp: TextInputState::default(),
            openai_model_inp: TextInputState::default(),
            router_loaded: false,
            router_models: Vec::new(),
            router_msg: String::new(),
            mic_test_msg: String::new(),
            mic_testing: false,
            whisper_dl_active: false,
            whisper_dl_stage: String::new(),
            whisper_dl_pct: 0.0,
            whisper_dl_mb: 0.0,
            whisper_dl_total_mb: 0.0,
            whisper_dl_msg: String::new(),
            recording_hotkey: None,
            hotkey_err: String::new(),
            voice_dropdown_open: false,
            update_checking: false,
            update_downloading: false,
            update_status: String::new(),
            update_info: None,
            measured_h: [360.0, 260.0, 520.0, 260.0, 620.0],
            nav_indicator_y: 0.0,
        }
    }
}

pub fn sec_index(s: SettingsSection) -> usize {
    match s {
        SettingsSection::Notch => 0,
        SettingsSection::Companion => 1,
        SettingsSection::Voice => 2,
        SettingsSection::Layout => 3,
        SettingsSection::General => 4,
    }
}

impl SettingsTabState {
    pub fn ensure_synced(&mut self, store: &StoreManager, settings: &Settings) {
        if !self.router_loaded {
            self.router_loaded = true;
            self.displays = display::get_displays_info();
            self.router_url_inp.set_text(&settings.dn_router_base_url);
            let k = store.read_router_api_key(None);
            self.router_key_inp.set_text(&k);
            self.router_model_inp.set_text(&settings.dn_router_model);
            let gk = store.read_gemini_api_key(None);
            self.gemini_key_inp.set_text(&gk);
            self.gemini_model_inp.set_text(&settings.dn_gemini_model);
            self.openai_url_inp.set_text(&settings.dn_openai_base_url);
            let ok = store.read_openai_api_key(None);
            self.openai_key_inp.set_text(&ok);
            self.openai_model_inp.set_text(&settings.dn_openai_model);
        }
    }
}

fn patch_bool(store: &StoreManager, ui: &UiSender, key: &str, val: bool) {
    let mut m = Map::new();
    m.insert(key.to_string(), Value::Bool(val));
    store.patch_settings(Some(ui), m);
}

fn patch_str(store: &StoreManager, ui: &UiSender, key: &str, val: &str) {
    let mut m = Map::new();
    m.insert(key.to_string(), Value::String(val.to_string()));
    store.patch_settings(Some(ui), m);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_settings(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    custom_photo: &str,
    accent: Color,
    hotkey_failures: &[String],
    state: &mut SettingsTabState,
    comp_renderer: &mut crate::ui::companion::CompanionRenderer,
    store: &StoreManager,
    ui: &UiSender,
    voice_mgr: &Arc<VoiceManager>,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    state.ensure_synced(store, settings);

    let area = Rect::new(
        notch_rect.x + 14.0,
        notch_rect.y + 40.0,
        notch_rect.w - 28.0,
        notch_rect.h - 52.0,
    );
    let side = Rect::new(area.x, area.y, 128.0, area.h);
    let body = Rect::new(side.right() + 14.0, area.y, area.w - 142.0, area.h);

    // Subtle vertical separator between sidebar and body (Tauri Foto 5 style, no outer boxes)
    d2d.draw_line(
        side.right() + 4.0,
        area.y + 6.0,
        side.right() + 4.0,
        area.bottom() - 6.0,
        1.0,
        Color::white(0.06),
    );

    let sections = [
        (SettingsSection::Notch, "notch", IconId::Monitor, "Notch"),
        (SettingsSection::Companion, "comp", IconId::Sparkles, "Companion"),
        (SettingsSection::Voice, "voice", IconId::Mic, "Voice"),
        (SettingsSection::Layout, "layout", IconId::Layers, "Layout"),
        (SettingsSection::General, "gen", IconId::Sliders, "General"),
    ];

    let cur_sec_idx = sec_index(state.section);
    let target_nav_y = side.y + 8.0 + cur_sec_idx as f32 * 34.0;
    if state.nav_indicator_y <= 0.1 {
        state.nav_indicator_y = target_nav_y;
    } else {
        state.nav_indicator_y += (target_nav_y - state.nav_indicator_y) * 0.36;
    }
    d2d.fill_rounded_rect(
        Rect::new(side.x + 2.0, state.nav_indicator_y, side.w - 8.0, 30.0),
        9.0,
        Color::white(0.10),
    );

    let mut sy = side.y + 8.0;
    for (sec, k, icon, label) in sections {
        let r = Rect::new(side.x + 2.0, sy, side.w - 8.0, 30.0);
        let h = hits.register(format!("set_sec_{}", k), r, CursorKind::Pointer, None, input);
        let active = state.section == sec;
        if !active && h.hovered {
            d2d.fill_rounded_rect(r, 9.0, Color::white(0.05));
        }
        d2d.draw_icon(
            icon,
            Rect::new(r.x + 10.0, r.center_y() - 6.5, 13.0, 13.0),
            Color::white(if active { 0.96 } else { 0.56 }),
            1.8,
            1.0,
        );
        d2d.draw_text(
            label,
            Rect::new(r.x + 30.0, r.y, r.w - 34.0, r.h),
            &TextStyle::default()
                .size(11.5)
                .medium()
                .color(Color::white(if active { 0.96 } else { 0.58 }))
                .vcenter(),
        );
        if sec == SettingsSection::General && state.update_info.is_some() {
            d2d.fill_circle(r.right() - 10.0, r.center_y(), 3.0, Color::rgb(52, 211, 153));
        }
        if h.clicked && state.section != sec {
            state.section = sec;
            state.voice_dropdown_open = false;
            state.scroll.offset = 0.0;
            state.scroll.target = 0.0;
        }
        sy += 34.0;
    }

    let ver_r = Rect::new(side.x + 4.0, side.bottom() - 24.0, side.w - 12.0, 20.0);
    let ver_h = hits.register("set_github", ver_r, CursorKind::Pointer, None, input);
    d2d.draw_text(
        concat!("SelaBar v", env!("CARGO_PKG_VERSION")),
        ver_r,
        &TextStyle::default()
            .size(10.0)
            .color(Color::white(if ver_h.hovered { 0.72 } else { 0.34 }))
            .vcenter(),
    );
    if ver_h.clicked {
        system::open_url_async(updater::REPO_URL);
    }

    // Right scrollable pane directly on Obsidian dark background
    let inner = body.inset(4.0, 4.0);
    let s_idx = sec_index(state.section);
    state.scroll.update_bounds(inner.h, state.measured_h[s_idx], input);
    state.scroll.handle_wheel(inner, false, input);

    d2d.push_clip_rect(inner);
    hits.push_clip(inner);
    let start_y = inner.y - state.scroll.offset + 2.0;
    let mut y = start_y;
    let w = inner.w - 10.0;

    let page_title = match state.section {
        SettingsSection::Notch => "Notch",
        SettingsSection::Companion => "Companion",
        SettingsSection::Voice => "Voice",
        SettingsSection::Layout => "Layout",
        SettingsSection::General => "General",
    };
    d2d.draw_text(
        page_title,
        Rect::new(inner.x, y, w, 22.0),
        &TextStyle::default()
            .size(15.0)
            .semibold()
            .color(Color::white(0.96))
            .vcenter(),
    );
    y += 28.0;

    match state.section {
        SettingsSection::Notch => {
            y = group_label(d2d, inner.x, y, w, "Size");
            draw_card_box(d2d, Rect::new(inner.x, y, w, 40.0));
            d2d.draw_text(
                "Ukuran Notch",
                Rect::new(inner.x + 12.0, y, 130.0, 40.0),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            if let Some(sz) = draw_segmented(
                d2d,
                hits,
                "set_notch_size",
                Rect::new(inner.x + w - 208.0, y + 7.0, 196.0, 26.0),
                &[("small", "Kecil"), ("default", "Normal"), ("large", "Besar")],
                &settings.notch_size,
                None,
                input,
            ) {
                let next_scale = match sz.as_str() {
                    "small" => 0.86,
                    "large" => 1.14,
                    _ => 1.0,
                };
                display::set_notch_ui_scale(next_scale);
                patch_str(store, ui, "notchSize", &sz);
                display::sync_notch_window(ui.get_hwnd(), store);
            }
            y += 40.0 + 14.0;

            y = group_label(d2d, inner.x, y, w, "Home cards");

            let card_rows: [(&str, &str, &str, Option<&str>, bool); 5] = [
                ("av", "showAvatar", "Companion", None, settings.show_avatar),
                ("mus", "showMusic", "Now playing", None, settings.show_music),
                ("tsk", "showTasks", "Tasks", None, settings.show_tasks),
                (
                    "sta",
                    "showStatus",
                    "Status",
                    Some("Mic, camera, battery and PC usage"),
                    settings.show_status,
                ),
                (
                    "aiu",
                    "showAiUsage",
                    "AI usage",
                    Some("Claude and Codex limits"),
                    settings.show_ai_usage,
                ),
            ];
            let card_h: f32 = card_rows
                .iter()
                .map(|r| if r.3.is_some() { 48.0 } else { 38.0 })
                .sum();
            let card_r = Rect::new(inner.x, y, w, card_h);
            draw_card_box(d2d, card_r);

            for (i, (id, key, lbl, detail, cur)) in card_rows.iter().enumerate() {
                let rh = if detail.is_some() { 48.0 } else { 38.0 };
                let row_r = Rect::new(inner.x, y, w, rh);
                if i > 0 {
                    d2d.draw_line(row_r.x + 12.0, row_r.y, row_r.right() - 12.0, row_r.y, 1.0, Color::white(0.06));
                }
                if let Some(v) = grouped_toggle_row(
                    d2d,
                    hits,
                    &format!("set_card_{}", id),
                    row_r,
                    lbl,
                    *detail,
                    *cur,
                    false,
                    input,
                ) {
                    patch_bool(store, ui, key, v);
                }
                y += rh;
            }
            y += 14.0;

            y = group_label(d2d, inner.x, y, w, "Closed notch");
            let closed_rows: [(&str, &str, &str, &str, bool); 2] = [
                (
                    "set_closed_lyr",
                    "lyricsOnClosed",
                    "Lyrics on closed notch",
                    "Current lyric line replaces the clock while music plays",
                    settings.lyrics_on_closed,
                ),
                (
                    "set_cap_ss",
                    "catchScreenshots",
                    "Open on screenshot",
                    "Catch new screenshots into the notch",
                    settings.catch_screenshots,
                ),
            ];
            let closed_h = closed_rows.len() as f32 * 48.0;
            draw_card_box(d2d, Rect::new(inner.x, y, w, closed_h));
            for (i, (hit_id, key, lbl, detail, cur)) in closed_rows.iter().enumerate() {
                let row_r = Rect::new(inner.x, y, w, 48.0);
                if i > 0 {
                    d2d.draw_line(row_r.x + 12.0, row_r.y, row_r.right() - 12.0, row_r.y, 1.0, Color::white(0.06));
                }
                if let Some(v) = grouped_toggle_row(
                    d2d,
                    hits,
                    hit_id,
                    row_r,
                    lbl,
                    Some(*detail),
                    *cur,
                    false,
                    input,
                ) {
                    patch_bool(store, ui, key, v);
                    if *key == "lyricsOnClosed" {
                        store.set(Some(ui), "lyricsOnClosed", Value::Bool(v));
                    }
                }
                y += 48.0;
            }
        }

        SettingsSection::Companion => {
            // Single grouped card: Character strip, Mode, Auto Sleep, Focus length
            let char_h = 68.0;
            let mode_h = 40.0;
            let sleep_h = 40.0;
            let dur_h = 40.0;
            let photo_row_h = if settings.avatar == "photo" { 38.0 } else { 0.0 };
            let card_h = char_h + mode_h + sleep_h + dur_h + photo_row_h;
            draw_card_box(d2d, Rect::new(inner.x, y, w, card_h));

            // 1. Character circular row matching Foto 1
            d2d.draw_text(
                "Character",
                Rect::new(inner.x + 12.0, y + 6.0, w - 24.0, 18.0),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            let species = [
                ("ghost", "Ghost"),
                ("cat", "Cat"),
                ("blob", "Blob"),
                ("clover", "Clover"),
                ("droid", "Droid"),
                ("alien", "Alien"),
                ("cloud", "Cloud"),
            ];
            let mut ax = inner.x + 10.0;
            let ay = y + 29.0;
            for (id, lbl) in species {
                let cell = Rect::new(ax, ay, 32.0, 32.0);
                let act = settings.avatar == id;
                let h = hits.register(
                    format!("set_av_{}", id),
                    cell,
                    CursorKind::Pointer,
                    Some(format!("Ganti karakter ke {}", lbl)),
                    input,
                );
                if act {
                    d2d.fill_circle(cell.center_x(), cell.center_y(), 16.0, Color::white(0.14));
                } else if h.hovered {
                    d2d.fill_circle(cell.center_x(), cell.center_y(), 16.0, Color::white(0.07));
                }
                let vis_r = Rect::new(cell.center_x() - 12.0, cell.center_y() - 12.0, 24.0, 24.0);
                let alpha = if act || h.hovered { 1.0 } else { 0.58 };
                comp_renderer.draw_visual_alpha(
                    d2d,
                    vis_r,
                    22.0,
                    id,
                    avatar_hex(id),
                    custom_photo,
                    input.now_ms,
                    alpha,
                );
                if h.clicked {
                    patch_str(store, ui, "avatar", id);
                }
                ax += 35.0;
            }
            // 8th custom photo circle button (Foto 1)
            let p_cell = Rect::new(ax, ay, 32.0, 32.0);
            let p_act = settings.avatar == "photo";
            let ph = hits.register(
                "set_av_photo",
                p_cell,
                CursorKind::Pointer,
                Some("Gunakan foto sendiri".to_string()),
                input,
            );
            if p_act {
                d2d.fill_circle(p_cell.center_x(), p_cell.center_y(), 16.0, Color::white(0.14));
            } else if ph.hovered {
                d2d.fill_circle(p_cell.center_x(), p_cell.center_y(), 16.0, Color::white(0.07));
            }
            if !custom_photo.is_empty() {
                let img_r = Rect::new(p_cell.center_x() - 11.0, p_cell.center_y() - 11.0, 22.0, 22.0);
                d2d.draw_image(custom_photo, img_r, 11.0, if p_act || ph.hovered { 1.0 } else { 0.65 }, true);
            } else {
                d2d.draw_icon(
                    IconId::Image,
                    Rect::new(p_cell.center_x() - 7.5, p_cell.center_y() - 7.5, 15.0, 15.0),
                    Color::white(if ph.hovered { 0.94 } else { 0.68 }),
                    1.8,
                    1.0,
                );
            }
            if ph.clicked {
                if !custom_photo.is_empty() && !p_act {
                    patch_str(store, ui, "avatar", "photo");
                } else {
                    patch_str(store, ui, "avatar", "photo");
                    pick_photo_bg(ui.get_hwnd(), ui);
                }
            }
            y += char_h;

            if settings.avatar == "photo" {
                d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
                d2d.draw_text(
                    "Custom photo",
                    Rect::new(inner.x + 12.0, y, w - 140.0, photo_row_h),
                    &TextStyle::default().size(12.0).color(Color::white(0.88)).vcenter(),
                );
                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_pick_photo",
                    Rect::new(inner.x + w - 122.0, y + 7.0, 110.0, 24.0),
                    if custom_photo.is_empty() { "Pilih Foto..." } else { "Ganti Foto..." },
                    false,
                    Some(accent),
                    input,
                ) {
                    pick_photo_bg(ui.get_hwnd(), ui);
                }
                y += photo_row_h;
            }

            // 2. Mode segmented row
            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            d2d.draw_text(
                "Mode",
                Rect::new(inner.x + 12.0, y, 120.0, mode_h),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            if let Some(m) = draw_segmented(
                d2d,
                hits,
                "set_cmode",
                Rect::new(inner.x + w - 248.0, y + 7.0, 236.0, 26.0),
                &[
                    ("time", "Time"),
                    ("focus", "Focus"),
                    ("calendar", "Jadwal"),
                    ("sleep", "Sleep"),
                ],
                &settings.companion_mode,
                None,
                input,
            ) {
                patch_str(store, ui, "companionMode", &m);
            }
            y += mode_h;

            // 2b. Auto Sleep segmented row
            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            d2d.draw_text(
                "Auto Sleep",
                Rect::new(inner.x + 12.0, y, 120.0, sleep_h),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            let sleep_val = match settings.companion_sleeps.as_str() {
                "time" => "night",
                other => other,
            };
            if let Some(slp) = draw_segmented(
                d2d,
                hits,
                "set_csleep",
                Rect::new(inner.x + w - 248.0, y + 7.0, 236.0, 26.0),
                &[
                    ("auto", "Auto"),
                    ("night", "22:00+"),
                    ("idle", "Idle"),
                    ("never", "Off"),
                ],
                sleep_val,
                None,
                input,
            ) {
                patch_str(store, ui, "companionSleeps", &slp);
            }
            y += sleep_h;

            // 3. Focus duration segmented row
            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            d2d.draw_text(
                "Focus duration",
                Rect::new(inner.x + 12.0, y, 140.0, dur_h),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            let cur_min_s = settings.focus_minutes.to_string();
            if let Some(chosen) = draw_segmented(
                d2d,
                hits,
                "set_fmin",
                Rect::new(inner.x + w - 196.0, y + 7.0, 184.0, 26.0),
                &[("15", "15m"), ("25", "25m"), ("45", "45m"), ("60", "60m")],
                &cur_min_s,
                None,
                input,
            ) {
                if let Ok(mins) = chosen.parse::<i32>() {
                    let mut m = Map::new();
                    m.insert("focusMinutes".to_string(), Value::from(mins));
                    store.patch_settings(Some(ui), m);
                }
            }
            y += dur_h;
        }

        SettingsSection::Voice => {
            if state.voice_dropdown_open
                && state.recording_hotkey.is_none()
                && input.keys.iter().any(|k| k.vk == VK_ESCAPE)
            {
                state.voice_dropdown_open = false;
            }

            let stt_detail = if let Some(lbl) = whisper::active_backend_label() {
                format!("Runtime lokal siap ({})", lbl)
            } else {
                "Model suara belum terpasang — unduh CPU atau CUDA di bawah".to_string()
            };
            let tts_detail = format!("Suara aktif: {}", voice_mgr.active_voice_label());

            let all_voices = voice_mgr.voice_infos();
            let mut voice_opts: Vec<(String, String, String, &'static str, bool)> =
                Vec::with_capacity(all_voices.len() + 1);
            voice_opts.push((
                "auto".to_string(),
                "Otomatis (Gadis Neural)".to_string(),
                "Otomatis (Neural)".to_string(),
                "Auto-Fallback",
                true,
            ));
            for v in all_voices.iter().filter(|v| v.is_edge_neural()) {
                voice_opts.push((
                    v.id.clone(),
                    format!("{} ({})", v.name, v.language),
                    v.name.clone(),
                    "Neural • Fallback",
                    true,
                ));
            }
            for v in all_voices.iter().filter(|v| !v.is_edge_neural()) {
                let short = v
                    .name
                    .replace("Microsoft ", "")
                    .replace(" Desktop", "")
                    .trim()
                    .to_string();
                let full_lbl = if v.language.is_empty() {
                    short.clone()
                } else {
                    format!("{} ({})", short, v.language)
                };
                voice_opts.push((
                    v.id.clone(),
                    full_lbl,
                    format!("{} (Offline)", short),
                    "Offline",
                    false,
                ));
            }

            let selected_idx = voice_opts
                .iter()
                .position(|(id, lbl, _, _, _)| {
                    *id == settings.tts_voice || lbl.eq_ignore_ascii_case(&settings.tts_voice)
                })
                .unwrap_or(0);
            let selected_short = voice_opts[selected_idx].2.clone();

            let has_id_voice = voice_mgr.has_indonesian_voice();
            let row2_h = if !has_id_voice { 32.0 } else { 0.0 };
            let msg_h = if state.mic_test_msg.is_empty() { 0.0 } else { 22.0 };
            let act_h = 44.0 + row2_h + msg_h;
            let dd_item_h = 30.0_f32;
            let dd_panel_h = 10.0 + (voice_opts.len() as f32) * dd_item_h;
            let dd_block_h = if state.voice_dropdown_open {
                dd_panel_h + 8.0
            } else {
                0.0
            };
            let card_h = 4.0 * 48.0 + dd_block_h + act_h;
            draw_card_box(d2d, Rect::new(inner.x, y, w, card_h));

            // Row 1: Balasan Suara (TTS) toggle
            let row1_r = Rect::new(inner.x, y, w, 48.0);
            if let Some(v) = grouped_toggle_row(
                d2d,
                hits,
                "set_tts_en",
                row1_r,
                "Balasan Suara (TTS)",
                Some(&tts_detail),
                settings.dn_voice_enabled,
                false,
                input,
            ) {
                patch_bool(store, ui, "dnVoiceEnabled", v);
            }
            y += 48.0;

            // Row 2: Suara Utama inline dropdown selector
            let dd_row_r = Rect::new(inner.x, y, w, 48.0);
            d2d.draw_line(
                dd_row_r.x + 12.0,
                dd_row_r.y,
                dd_row_r.right() - 12.0,
                dd_row_r.y,
                1.0,
                Color::white(0.06),
            );
            let dd_h = hits.register(
                "set_voice_dropdown_toggle",
                dd_row_r,
                CursorKind::Pointer,
                None,
                input,
            );
            if dd_h.hovered {
                d2d.fill_rounded_rect(dd_row_r.inset(2.0, 2.0), 8.0, Color::white(0.03));
            }
            let dd_box_w = 164.0_f32;
            let dd_text_w = (dd_row_r.w - dd_box_w - 32.0).max(90.0);
            d2d.draw_text(
                "Suara Utama",
                Rect::new(dd_row_r.x + 12.0, dd_row_r.y + 7.0, dd_text_w, 17.0),
                &TextStyle::default()
                    .size(12.0)
                    .medium()
                    .color(Color::white(0.92))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                "Otomatis fallback ke offline saat tanpa internet",
                Rect::new(dd_row_r.x + 12.0, dd_row_r.y + 24.0, dd_text_w, 15.0),
                &TextStyle::default()
                    .size(10.2)
                    .color(Color::white(0.44))
                    .vcenter()
                    .ellipsis(),
            );
            let dd_btn = Rect::new(
                dd_row_r.right() - dd_box_w - 12.0,
                dd_row_r.center_y() - 14.0,
                dd_box_w,
                28.0,
            );
            d2d.fill_rounded_rect(
                dd_btn,
                8.0,
                Color::white(if dd_h.hovered || state.voice_dropdown_open {
                    0.12
                } else {
                    0.075
                }),
            );
            d2d.stroke_rounded_rect(
                dd_btn,
                8.0,
                1.0,
                if state.voice_dropdown_open {
                    accent.with_alpha(0.65)
                } else {
                    Color::white(0.14)
                },
            );
            d2d.draw_text(
                &selected_short,
                Rect::new(dd_btn.x + 10.0, dd_btn.y, dd_btn.w - 28.0, dd_btn.h),
                &TextStyle::default()
                    .size(11.0)
                    .medium()
                    .color(Color::white(0.92))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_icon(
                IconId::ChevronDown,
                Rect::new(dd_btn.right() - 19.0, dd_btn.center_y() - 6.0, 12.0, 12.0),
                Color::white(0.75),
                1.8,
                1.0,
            );
            if dd_h.clicked {
                state.voice_dropdown_open = !state.voice_dropdown_open;
            }
            y += 48.0;

            if state.voice_dropdown_open {
                let panel = Rect::new(inner.x + 12.0, y + 2.0, w - 24.0, dd_panel_h);
                d2d.fill_rounded_rect(panel, 10.0, Color::rgba(14, 16, 22, 0.96));
                d2d.stroke_rounded_rect(panel, 10.0, 1.0, Color::white(0.14));

                for (oi, (opt_id, opt_label, _short, badge, is_neural)) in
                    voice_opts.iter().enumerate()
                {
                    let opt_r = Rect::new(
                        panel.x + 5.0,
                        panel.y + 5.0 + oi as f32 * dd_item_h,
                        panel.w - 10.0,
                        dd_item_h - 2.0,
                    );
                    let selected = oi == selected_idx;
                    let oh = hits.register(
                        format!("set_voice_opt_{}", oi),
                        opt_r,
                        CursorKind::Pointer,
                        None,
                        input,
                    );
                    if selected {
                        d2d.fill_rounded_rect(opt_r, 7.0, accent.with_alpha(0.16));
                    } else if oh.hovered {
                        d2d.fill_rounded_rect(opt_r, 7.0, Color::white(0.06));
                    }

                    if selected {
                        d2d.draw_icon(
                            IconId::Check,
                            Rect::new(opt_r.x + 8.0, opt_r.center_y() - 5.5, 11.0, 11.0),
                            accent,
                            2.0,
                            1.0,
                        );
                    }

                    let badge_w = 96.0_f32;
                    let lbl_x = opt_r.x + 25.0;
                    let lbl_w = (opt_r.w - 25.0 - badge_w - 10.0).max(80.0);
                    d2d.draw_text(
                        opt_label,
                        Rect::new(lbl_x, opt_r.y, lbl_w, opt_r.h),
                        &TextStyle::default()
                            .size(11.2)
                            .medium()
                            .color(Color::white(if selected { 0.98 } else { 0.78 }))
                            .vcenter()
                            .ellipsis(),
                    );

                    let badge_r = Rect::new(
                        opt_r.right() - badge_w - 6.0,
                        opt_r.center_y() - 9.0,
                        badge_w,
                        18.0,
                    );
                    d2d.fill_rounded_rect(
                        badge_r,
                        9.0,
                        if *is_neural {
                            Color::rgba(56, 189, 248, 0.14)
                        } else {
                            Color::white(0.07)
                        },
                    );
                    d2d.draw_text(
                        badge,
                        badge_r,
                        &TextStyle::default()
                            .size(9.5)
                            .medium()
                            .color(if *is_neural {
                                Color::rgb(125, 211, 252)
                            } else {
                                Color::white(0.55)
                            })
                            .center()
                            .vcenter(),
                    );

                    if oh.clicked {
                        voice_mgr.set_voice_preference(opt_id);
                        patch_str(store, ui, "ttsVoice", opt_id);
                        state.voice_dropdown_open = false;
                    }
                }
                y += dd_block_h;
            }

            // Row 3 & Row 4: Whisper STT & Chime
            let extra_rows: [(&str, &str, &str, &str, bool); 2] = [
                (
                    "set_stt_en",
                    "dnSttEnabled",
                    "Speech-to-Text Lokal (Whisper)",
                    &stt_detail,
                    settings.dn_stt_enabled,
                ),
                (
                    "set_ex_ai",
                    "dnExtrasAi",
                    "Chime & Notifikasi Selesai AI",
                    "Mainkan nada halus saat tugas AI atau pengingat selesai",
                    settings.dn_extras_ai,
                ),
            ];
            for (hit_id, key, lbl, detail, cur) in extra_rows {
                let row_r = Rect::new(inner.x, y, w, 48.0);
                d2d.draw_line(
                    row_r.x + 12.0,
                    row_r.y,
                    row_r.right() - 12.0,
                    row_r.y,
                    1.0,
                    Color::white(0.06),
                );
                if let Some(v) = grouped_toggle_row(
                    d2d,
                    hits,
                    hit_id,
                    row_r,
                    lbl,
                    Some(detail),
                    cur,
                    false,
                    input,
                ) {
                    patch_bool(store, ui, key, v);
                }
                y += 48.0;
            }

            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            if draw_pill_btn(
                d2d,
                hits,
                "set_test_tts",
                Rect::new(inner.x + 12.0, y + 10.0, 90.0, 24.0),
                "Tes Suara",
                false,
                Some(accent),
                input,
            ) {
                let vm = Arc::clone(voice_mgr);
                rt.spawn_blocking(move || {
                    let _ = vm.test_speak("Halo! Suara SelaBar siap membantu hari ini!");
                });
            }
            if draw_pill_btn(
                d2d,
                hits,
                "set_test_mic",
                Rect::new(inner.x + 108.0, y + 10.0, 118.0, 24.0),
                if state.mic_testing { "Merekam 5d..." } else { "Tes Mikrofon (5d)" },
                state.mic_testing,
                Some(Color::rgb(255, 159, 10)),
                input,
            ) && !state.mic_testing
            {
                if !whisper::is_available() {
                    state.mic_test_msg =
                        "Model Whisper belum terpasang — pilih Unduh CPU atau CUDA di bawah."
                            .to_string();
                } else {
                    state.mic_testing = true;
                    state.mic_test_msg = "Berbicara selama 4.5 detik...".to_string();
                    voice_mgr.run_mic_test(rt, ui.clone());
                }
            }

            if !has_id_voice {
                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_install_id_voice",
                    Rect::new(inner.x + 12.0, y + 40.0, 178.0, 22.0),
                    "Pasang Suara Offline Indo",
                    false,
                    Some(Color::rgb(251, 191, 36)),
                    input,
                ) {
                    system::open_url_async("ms-settings:regionlanguage");
                }
            }

            if !state.mic_test_msg.is_empty() {
                d2d.draw_text(
                    &state.mic_test_msg,
                    Rect::new(inner.x + 12.0, y + 40.0 + row2_h, w - 24.0, 18.0),
                    &TextStyle::default().size(10.5).color(Color::white(0.75)).ellipsis(),
                );
            }
            y += act_h + 14.0;

            y = group_label(d2d, inner.x, y, w, "Download Model Suara (Whisper STT)");
            let dl_active = state.whisper_dl_active || voice_mgr.downloader.is_downloading();
            let has_gpu = downloader::has_nvidia_gpu();
            let has_model = whisper::has_model();
            let cpu_installed = whisper::has_cpu_cli() && has_model;
            let cuda_installed = whisper::has_cuda_cli() && has_model;
            let dl_msg_h = if !dl_active && !state.whisper_dl_msg.is_empty() {
                20.0
            } else {
                0.0
            };
            let dl_card_h = if dl_active { 66.0 } else { 66.0 + dl_msg_h };
            draw_card_box(d2d, Rect::new(inner.x, y, w, dl_card_h));

            let hint_txt = if let Some(active_lbl) = whisper::active_backend_label() {
                if has_gpu {
                    format!("Terpasang: {} • GPU NVIDIA terdeteksi", active_lbl)
                } else {
                    format!("Terpasang: {}", active_lbl)
                }
            } else if has_gpu {
                "GPU NVIDIA terdeteksi — CUDA lebih cepat, CPU lebih hemat kuota".to_string()
            } else {
                "Pilih mode CPU (~190 MB, semua PC) atau CUDA (~882 MB, GPU NVIDIA)".to_string()
            };
            d2d.draw_text(
                &hint_txt,
                Rect::new(inner.x + 12.0, y + 7.0, w - 24.0, 16.0),
                &TextStyle::default()
                    .size(10.8)
                    .color(Color::white(0.62))
                    .vcenter()
                    .ellipsis(),
            );

            if dl_active {
                let stage_txt = if state.whisper_dl_stage.is_empty() {
                    "Mengunduh runtime & model suara...".to_string()
                } else if state.whisper_dl_total_mb > 0.1 {
                    format!(
                        "{}  ({:.1} / {:.1} MB • {}%)",
                        state.whisper_dl_stage,
                        state.whisper_dl_mb,
                        state.whisper_dl_total_mb,
                        (state.whisper_dl_pct * 100.0).round() as u32
                    )
                } else {
                    state.whisper_dl_stage.clone()
                };
                d2d.draw_text(
                    &stage_txt,
                    Rect::new(inner.x + 12.0, y + 27.0, w - 96.0, 16.0),
                    &TextStyle::default()
                        .size(11.0)
                        .medium()
                        .color(Color::white(0.90))
                        .vcenter()
                        .ellipsis(),
                );
                let bar_r = Rect::new(inner.x + 12.0, y + 48.0, w - 98.0, 6.0);
                d2d.fill_rounded_rect(bar_r, 3.0, Color::white(0.10));
                let fill_w = (bar_r.w * state.whisper_dl_pct.clamp(0.03, 1.0)).max(6.0);
                d2d.fill_rounded_rect(
                    Rect::new(bar_r.x, bar_r.y, fill_w, bar_r.h),
                    3.0,
                    Color::rgb(56, 189, 248),
                );
                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_dl_whisper_cancel",
                    Rect::new(inner.x + w - 76.0, y + 32.0, 64.0, 24.0),
                    "Batal",
                    false,
                    Some(Color::rgb(248, 113, 113)),
                    input,
                ) {
                    voice_mgr.downloader.cancel();
                    state.whisper_dl_stage = "Membatalkan unduhan...".to_string();
                }
            } else {
                let btn_w = (w - 34.0) * 0.5;
                let cpu_lbl = if cpu_installed {
                    "CPU Terpasang ✓"
                } else if has_model {
                    "Pasang CPU (~9 MB)"
                } else {
                    "Unduh CPU (~190 MB)"
                };
                let cuda_lbl = if cuda_installed {
                    "CUDA Terpasang ✓"
                } else if has_model {
                    "Pasang CUDA (~701 MB)"
                } else {
                    "Unduh CUDA (~882 MB)"
                };

                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_dl_whisper_cpu",
                    Rect::new(inner.x + 12.0, y + 29.0, btn_w, 26.0),
                    cpu_lbl,
                    cpu_installed,
                    Some(if cpu_installed {
                        Color::rgb(52, 211, 153)
                    } else {
                        Color::rgb(56, 189, 248)
                    }),
                    input,
                ) && !cpu_installed
                {
                    state.whisper_dl_active = true;
                    state.whisper_dl_stage = "Menyiapkan unduhan CPU...".to_string();
                    state.whisper_dl_pct = 0.0;
                    state.whisper_dl_mb = 0.0;
                    state.whisper_dl_total_mb = if has_model { 8.8 } else { 190.1 };
                    state.whisper_dl_msg.clear();
                    voice_mgr.downloader.start(WhisperBackendKind::Cpu, rt, ui.clone());
                }

                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_dl_whisper_cuda",
                    Rect::new(inner.x + 12.0 + btn_w + 10.0, y + 29.0, btn_w, 26.0),
                    cuda_lbl,
                    cuda_installed,
                    Some(if cuda_installed {
                        Color::rgb(52, 211, 153)
                    } else {
                        Color::rgb(168, 85, 247)
                    }),
                    input,
                ) && !cuda_installed
                {
                    state.whisper_dl_active = true;
                    state.whisper_dl_stage = "Menyiapkan unduhan CUDA...".to_string();
                    state.whisper_dl_pct = 0.0;
                    state.whisper_dl_mb = 0.0;
                    state.whisper_dl_total_mb = if has_model { 701.5 } else { 882.8 };
                    state.whisper_dl_msg.clear();
                    voice_mgr.downloader.start(WhisperBackendKind::Cuda, rt, ui.clone());
                }

                if !state.whisper_dl_msg.is_empty() {
                    let is_ok = state.whisper_dl_msg.contains("siap dipakai")
                        || state.whisper_dl_msg.contains("terpasang");
                    d2d.draw_text(
                        &state.whisper_dl_msg,
                        Rect::new(inner.x + 12.0, y + 61.0, w - 24.0, 16.0),
                        &TextStyle::default()
                            .size(10.5)
                            .color(if is_ok {
                                Color::rgb(52, 211, 153)
                            } else {
                                Color::rgb(251, 191, 36)
                            })
                            .vcenter()
                            .ellipsis(),
                    );
                }
            }
            y += dl_card_h + 14.0;

            y = group_label(d2d, inner.x, y, w, "Global Hotkeys");

            if let Some(rec_idx) = state.recording_hotkey {
                hits.focused_input = Some("hotkey_capture".to_string());
                for ev in &input.keys {
                    if ev.vk == VK_ESCAPE {
                        state.recording_hotkey = None;
                        state.hotkey_err.clear();
                        hits.focused_input = None;
                        voice_mgr.hotkeys.set_paused(false);
                        break;
                    }
                    if let Some(combo) = hotkey::format_hotkey_key_event(ev.vk, ev.ctrl, ev.alt, ev.shift) {
                        let cur = [
                            settings.hotkey_voice.as_str(),
                            settings.hotkey_dictate.as_str(),
                            settings.hotkey_ask.as_str(),
                            settings.hotkey_chat.as_str(),
                        ];
                        let dup = cur.iter().enumerate().any(|(j, existing)| {
                            j != rec_idx && existing.eq_ignore_ascii_case(&combo)
                        });
                        if dup {
                            state.hotkey_err = format!("Shortcut {} sudah dipakai aksi lain.", combo);
                        } else {
                            let setting_keys = [
                                "hotkeyVoice",
                                "hotkeyDictate",
                                "hotkeyAsk",
                                "hotkeyChat",
                            ];
                            let mut next = HotkeyBindings::from_settings(settings);
                            match rec_idx {
                                0 => next.voice = combo.clone(),
                                1 => next.dictate = combo.clone(),
                                2 => next.ask = combo.clone(),
                                3 => next.chat = combo.clone(),
                                _ => {}
                            }
                            patch_str(store, ui, setting_keys[rec_idx], &combo);
                            state.recording_hotkey = None;
                            state.hotkey_err.clear();
                            hits.focused_input = None;
                            voice_mgr.hotkeys.reload_bindings(next);
                            voice_mgr.hotkeys.set_paused(false);
                        }
                        break;
                    }
                }
            }

            let err_text = if !state.hotkey_err.is_empty() {
                state.hotkey_err.clone()
            } else if !hotkey_failures.is_empty() {
                format!("Bentrok hotkey: {}", hotkey_failures.join(", "))
            } else {
                String::new()
            };
            let hk_rows: [(&str, &str); 4] = [
                ("Live Voice AI", settings.hotkey_voice.as_str()),
                ("Dikte ke Kursor", settings.hotkey_dictate.as_str()),
                ("Tanya AI (Ask)", settings.hotkey_ask.as_str()),
                ("Chat AI", settings.hotkey_chat.as_str()),
            ];
            let err_h = if err_text.is_empty() { 0.0 } else { 22.0 };
            let hk_h = hk_rows.len() as f32 * 36.0 + 38.0 + err_h;
            draw_card_box(d2d, Rect::new(inner.x, y, w, hk_h));

            for (idx, (lbl, val)) in hk_rows.iter().enumerate() {
                let row_r = Rect::new(inner.x, y, w, 36.0);
                if idx > 0 {
                    d2d.draw_line(
                        row_r.x + 12.0,
                        row_r.y,
                        row_r.right() - 12.0,
                        row_r.y,
                        1.0,
                        Color::white(0.06),
                    );
                }
                d2d.draw_text(
                    lbl,
                    Rect::new(row_r.x + 12.0, row_r.y, (row_r.w - 160.0).max(80.0), row_r.h),
                    &TextStyle::default()
                        .size(12.0)
                        .medium()
                        .color(Color::white(0.90))
                        .vcenter(),
                );
                let is_rec = state.recording_hotkey == Some(idx);
                let pill_lbl = if is_rec { "Tekan tombol..." } else { *val };
                let pill_r = Rect::new(row_r.right() - 134.0, row_r.y + 6.0, 122.0, 24.0);
                if draw_pill_btn(
                    d2d,
                    hits,
                    &format!("set_hk_{}", idx),
                    pill_r,
                    pill_lbl,
                    is_rec,
                    if is_rec { Some(accent) } else { None },
                    input,
                ) {
                    if is_rec {
                        state.recording_hotkey = None;
                        state.hotkey_err.clear();
                        hits.focused_input = None;
                        voice_mgr.hotkeys.set_paused(false);
                    } else {
                        state.recording_hotkey = Some(idx);
                        state.hotkey_err.clear();
                        hits.focused_input = Some("hotkey_capture".to_string());
                        voice_mgr.hotkeys.set_paused(true);
                    }
                }
                y += 36.0;
            }

            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            d2d.draw_text(
                "Klik shortcut lalu tekan kombinasi baru • Esc: Batal",
                Rect::new(inner.x + 12.0, y, (w - 132.0).max(80.0), 38.0),
                &TextStyle::default()
                    .size(10.5)
                    .color(Color::white(0.45))
                    .vcenter()
                    .ellipsis(),
            );
            if draw_pill_btn(
                d2d,
                hits,
                "set_hk_reset",
                Rect::new(inner.x + w - 114.0, y + 7.0, 102.0, 24.0),
                "Reset Default",
                false,
                None,
                input,
            ) {
                let def = HotkeyBindings::default();
                let mut m = Map::new();
                m.insert("hotkeyVoice".to_string(), Value::String(def.voice.clone()));
                m.insert("hotkeyDictate".to_string(), Value::String(def.dictate.clone()));
                m.insert("hotkeyAsk".to_string(), Value::String(def.ask.clone()));
                m.insert("hotkeyChat".to_string(), Value::String(def.chat.clone()));
                store.patch_settings(Some(ui), m);
                state.recording_hotkey = None;
                state.hotkey_err.clear();
                hits.focused_input = None;
                voice_mgr.hotkeys.reload_bindings(def);
                voice_mgr.hotkeys.set_paused(false);
            }
            y += 38.0;

            if !err_text.is_empty() {
                d2d.draw_text(
                    &err_text,
                    Rect::new(inner.x + 12.0, y, w - 24.0, 18.0),
                    &TextStyle::default()
                        .size(10.5)
                        .color(Color::rgb(255, 69, 58))
                        .ellipsis(),
                );
                y += 22.0;
            }
        }

        SettingsSection::Layout => {
            y = group_label(d2d, inner.x, y, w, "Tabs");
            let tabs = [
                ("desk", "Focus"),
                ("files", "Files"),
                ("music", "Music"),
                ("cal", "Calendar"),
                ("conversation", "Chat"),
            ];
            let tabs_h = tabs.len() as f32 * 38.0;
            draw_card_box(d2d, Rect::new(inner.x, y, w, tabs_h));
            for (i, (k, lbl)) in tabs.iter().enumerate() {
                let row_r = Rect::new(inner.x, y, w, 38.0);
                if i > 0 {
                    d2d.draw_line(row_r.x + 12.0, row_r.y, row_r.right() - 12.0, row_r.y, 1.0, Color::white(0.06));
                }
                let is_vis = !settings.hidden_views.iter().any(|h| h == k);
                if let Some(next_vis) = grouped_toggle_row(
                    d2d,
                    hits,
                    &format!("set_tabvis_{}", k),
                    row_r,
                    lbl,
                    None,
                    is_vis,
                    false,
                    input,
                ) {
                    let mut next = settings.hidden_views.clone();
                    if next_vis {
                        next.retain(|h| h != k);
                    } else if !next.iter().any(|h| h == k) {
                        next.push((*k).to_string());
                    }
                    let mut m = Map::new();
                    m.insert("hiddenViews".to_string(), Value::from(next));
                    store.patch_settings(Some(ui), m);
                }
                y += 38.0;
            }
            y += 14.0;
        }

        SettingsSection::General => {
            y = group_label(d2d, inner.x, y, w, "Appearance");
            draw_card_box(d2d, Rect::new(inner.x, y, w, 136.0));
            d2d.draw_text(
                "Style",
                Rect::new(inner.x + 12.0, y, 120.0, 40.0),
                &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
            );
            if let Some(st_id) = draw_segmented(
                d2d,
                hits,
                "set_style",
                Rect::new(inner.x + w - 154.0, y + 7.0, 142.0, 26.0),
                &[("black", "Default"), ("glass", "Glass")],
                &settings.notch_style,
                None,
                input,
            ) {
                patch_str(store, ui, "notchStyle", &st_id);
                if st_id == "glass" {
                    patch_bool(store, ui, "hideInScreenshots", true);
                }
            }
            y += 40.0;
            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            if let Some(v) = grouped_toggle_row(
                d2d,
                hits,
                "set_music_glow",
                Rect::new(inner.x, y, w, 48.0),
                "Music glow",
                Some("Ambient glow behind expanded notch while music plays"),
                settings.ambient_video,
                false,
                input,
            ) {
                patch_bool(store, ui, "ambientVideo", v);
            }
            y += 48.0;
            d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
            if let Some(v) = grouped_toggle_row(
                d2d,
                hits,
                "set_alb_tint",
                Rect::new(inner.x, y, w, 48.0),
                "Album colours",
                Some("Tint notch accents from active album cover art"),
                settings.album_tint,
                false,
                input,
            ) {
                patch_bool(store, ui, "albumTint", v);
            }
            y += 48.0 + 14.0;

            y = group_label(d2d, inner.x, y, w, "Update Aplikasi");
            let upd_info = state.update_info.clone();
            let upd_h = if upd_info.is_some() { 86.0 } else { 54.0 };
            draw_card_box(d2d, Rect::new(inner.x, y, w, upd_h));
            let ux = inner.x + 12.0;
            let uw = w - 24.0;
            let headline = match &upd_info {
                Some(info) => format!(
                    "Update tersedia: v{} (terpasang v{})",
                    info.version,
                    updater::CURRENT_VERSION
                ),
                None => format!("Versi terpasang: SelaBar v{}", updater::CURRENT_VERSION),
            };
            let subline = if !state.update_status.is_empty() {
                state.update_status.as_str()
            } else if state.update_checking {
                "Memeriksa rilis terbaru di GitHub..."
            } else {
                "Auto-check rilis GitHub aktif setiap startup"
            };
            let text_w = (uw - 102.0).max(120.0);
            d2d.draw_text(
                &headline,
                Rect::new(ux, y + 8.0, text_w, 18.0),
                &TextStyle::default()
                    .size(11.5)
                    .semibold()
                    .color(if upd_info.is_some() {
                        Color::rgb(110, 231, 183)
                    } else {
                        Color::white(0.92)
                    })
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                subline,
                Rect::new(ux, y + 27.0, text_w, 16.0),
                &TextStyle::default()
                    .size(10.0)
                    .color(Color::white(0.52))
                    .vcenter()
                    .ellipsis(),
            );
            let chk_label = if state.update_checking {
                "Mengecek..."
            } else {
                "Cek Update"
            };
            if draw_pill_btn(
                d2d,
                hits,
                "set_upd_check",
                Rect::new(ux + uw - 92.0, y + 14.0, 92.0, 24.0),
                chk_label,
                false,
                None,
                input,
            ) && !state.update_checking
                && !state.update_downloading
            {
                state.update_checking = true;
                state.update_status = "Mengecek rilis terbaru di GitHub...".to_string();
                let u = ui.clone();
                rt.spawn(async move {
                    let outcome = updater::check_latest_release().await;
                    u.send(UiEvent::UpdateCheckDone {
                        manual: true,
                        outcome,
                    });
                });
            }
            if let Some(info) = upd_info {
                let dl_lbl = if state.update_downloading {
                    "Mengunduh..."
                } else {
                    "Unduh & Pasang"
                };
                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_upd_install",
                    Rect::new(ux, y + 52.0, 122.0, 24.0),
                    dl_lbl,
                    true,
                    Some(accent),
                    input,
                ) && !state.update_downloading
                {
                    state.update_downloading = true;
                    state.update_status =
                        format!("Mengunduh SelaBar v{} dari GitHub...", info.version);
                    let u = ui.clone();
                    rt.spawn(async move {
                        let res = updater::download_and_launch_installer(&info).await;
                        let (ok, msg) = match res {
                            Ok(m) => (true, m),
                            Err(e) => (false, e),
                        };
                        u.send(UiEvent::UpdateDownloadDone { ok, msg });
                    });
                }
                if draw_pill_btn(
                    d2d,
                    hits,
                    "set_upd_notes",
                    Rect::new(ux + 130.0, y + 52.0, 96.0, 24.0),
                    "Catatan Rilis",
                    false,
                    None,
                    input,
                ) {
                    let target = if updater::is_allowed_github_url(
                        state
                            .update_info
                            .as_ref()
                            .map(|i| i.html_url.as_str())
                            .unwrap_or(""),
                    ) {
                        state
                            .update_info
                            .as_ref()
                            .map(|i| i.html_url.clone())
                            .unwrap_or_else(|| format!("{}/releases/latest", updater::REPO_URL))
                    } else {
                        format!("{}/releases/latest", updater::REPO_URL)
                    };
                    system::open_url_async(&target);
                }
            }
            y += upd_h + 14.0;

            y = group_label(d2d, inner.x, y, w, "System");
            if state.displays.is_empty() {
                state.displays = display::get_displays_info();
            }
            let has_multi_disp = state.displays.len() > 1;
            let sys_h = 38.0 + 48.0 * 3.0 + if has_multi_disp { 40.0 } else { 0.0 };
            draw_card_box(d2d, Rect::new(inner.x, y, w, sys_h));

            let row0 = Rect::new(inner.x, y, w, 38.0);
            if let Some(v) = grouped_toggle_row(
                d2d,
                hits,
                "set_boot",
                row0,
                "Start with Windows",
                None,
                settings.start_on_boot,
                false,
                input,
            ) {
                patch_bool(store, ui, "startOnBoot", v);
                std::thread::spawn(move || {
                    let _ = system::set_start_on_boot(v);
                });
            }
            y += 38.0;

            let sys_detail_rows = [
                (
                    "set_hide_fs",
                    "hideOnFullscreen",
                    "Hide in fullscreen",
                    "Videos, games and F11",
                    settings.hide_on_fullscreen,
                    false,
                ),
                (
                    "set_hide_cap",
                    "hideInScreenshots",
                    "Hide in screenshots",
                    if settings.notch_style == "glass" {
                        "Always on with Glass"
                    } else {
                        "Also screen recordings and sharing"
                    },
                    settings.notch_style == "glass" || settings.hide_in_screenshots,
                    settings.notch_style == "glass",
                ),
                (
                    "set_tuck_br",
                    "tuckForBrowsers",
                    "Shrink over browsers",
                    "Keeps browser tabs visible",
                    settings.tuck_for_browsers,
                    false,
                ),
            ];
            for (hit_id, key, lbl, detail, cur, dis) in sys_detail_rows {
                let rr = Rect::new(inner.x, y, w, 48.0);
                d2d.draw_line(rr.x + 12.0, rr.y, rr.right() - 12.0, rr.y, 1.0, Color::white(0.06));
                if let Some(v) = grouped_toggle_row(
                    d2d,
                    hits,
                    hit_id,
                    rr,
                    lbl,
                    Some(detail),
                    cur,
                    dis,
                    input,
                ) {
                    patch_bool(store, ui, key, v);
                }
                y += 48.0;
            }
            if has_multi_disp {
                d2d.draw_line(inner.x + 12.0, y, inner.x + w - 12.0, y, 1.0, Color::white(0.06));
                d2d.draw_text(
                    "Display",
                    Rect::new(inner.x + 12.0, y, 90.0, 40.0),
                    &TextStyle::default().size(12.0).medium().color(Color::white(0.92)).vcenter(),
                );
                let mut dx = inner.x + 110.0;
                for (i, mon) in state.displays.iter().take(4).enumerate() {
                    let lbl = if mon.is_primary {
                        format!("Utama {}", i + 1)
                    } else {
                        format!("Layar {}", i + 1)
                    };
                    let act = settings.selected_display_id == mon.id
                        || (settings.selected_display_id == "primary" && mon.is_primary);
                    if draw_pill_btn(
                        d2d,
                        hits,
                        &format!("set_disp_{}", i),
                        Rect::new(dx, y + 8.0, 74.0, 24.0),
                        &lbl,
                        act,
                        Some(accent),
                        input,
                    ) {
                        patch_str(store, ui, "selectedDisplayId", &mon.id);
                        display::sync_notch_window(ui.get_hwnd(), store);
                    }
                    dx += 78.0;
                }
                y += 40.0;
            }
            y += 14.0;

            y = group_label(d2d, inner.x, y, w, "AI Providers (Auto-Fallback)");
            let msg_extra = if !state.router_msg.is_empty() { 22.0 } else { 0.0 };
            let mod_extra = if !state.router_models.is_empty() { 30.0 } else { 0.0 };
            let r_card_h = 460.0 + msg_extra + mod_extra;
            draw_card_box(d2d, Rect::new(inner.x, y, w, r_card_h));

            let rx = inner.x + 12.0;
            let rw = w - 24.0;
            y += 10.0;

            // Priority selector
            let prio_seg_w = 220.0_f32.min((rw * 0.62).max(160.0));
            let prio_lbl_w = (rw - prio_seg_w - 8.0).max(60.0);
            d2d.draw_text(
                "Prioritas AI",
                Rect::new(rx, y, prio_lbl_w, 16.0),
                &TextStyle::default().size(11.5).medium().color(Color::white(0.90)).vcenter(),
            );
            d2d.draw_text(
                "Auto fallback saat limit (429) atau gagal",
                Rect::new(rx, y + 16.0, prio_lbl_w, 14.0),
                &TextStyle::default().size(9.5).color(Color::white(0.45)).ellipsis(),
            );
            if let Some(prio) = draw_segmented(
                d2d,
                hits,
                "set_ai_prio",
                Rect::new(rx + rw - prio_seg_w, y + 2.0, prio_seg_w, 26.0),
                &[
                    ("auto", "Auto"),
                    ("gemini", "Gemini"),
                    ("openai", "OpenAI"),
                    ("router", "Router"),
                ],
                &settings.dn_ai_primary_provider,
                None,
                input,
            ) {
                patch_str(store, ui, "dnAiPrimaryProvider", &prio);
            }
            y += 38.0;
            d2d.draw_line(rx, y, rx + rw, y, 1.0, Color::white(0.06));
            y += 8.0;

            // 1. Google Gemini (AI Studio)
            d2d.draw_text(
                "Google Gemini (AI Studio)",
                Rect::new(rx, y, rw, 15.0),
                &TextStyle::default().size(10.5).semibold().color(Color::rgb(125, 211, 252)).vcenter(),
            );
            y += 18.0;
            d2d.draw_text("Gemini Key", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.gemini_key_inp.draw(d2d, hits, "set_g_key", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Tempel AI Studio API Key (AIza...)", true, None, input);
            y += 30.0;
            d2d.draw_text("Model Gemini", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.gemini_model_inp.draw(d2d, hits, "set_g_mod", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Default: gemini-2.5-flash", false, None, input);
            y += 30.0;
            d2d.draw_line(rx, y + 2.0, rx + rw, y + 2.0, 1.0, Color::white(0.06));
            y += 10.0;

            // 2. OpenAI-Compatible
            d2d.draw_text(
                "OpenAI-Compatible (OpenAI / Groq / OpenRouter)",
                Rect::new(rx, y, rw, 15.0),
                &TextStyle::default().size(10.5).semibold().color(Color::white(0.84)).vcenter(),
            );
            y += 18.0;
            d2d.draw_text("Endpoint URL", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.openai_url_inp.draw(d2d, hits, "set_oa_url", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Contoh: https://api.openai.com/v1", false, None, input);
            y += 30.0;
            d2d.draw_text("API Key", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.openai_key_inp.draw(d2d, hits, "set_oa_key", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "sk-...", true, None, input);
            y += 30.0;
            d2d.draw_text("Model", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.openai_model_inp.draw(d2d, hits, "set_oa_mod", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Contoh: gpt-4o-mini", false, None, input);
            y += 30.0;
            d2d.draw_line(rx, y + 2.0, rx + rw, y + 2.0, 1.0, Color::white(0.06));
            y += 10.0;

            // 3. Local / Custom Router
            d2d.draw_text(
                "Local / Custom Router (Ollama / 9router)",
                Rect::new(rx, y, rw, 15.0),
                &TextStyle::default().size(10.5).semibold().color(Color::white(0.84)).vcenter(),
            );
            y += 18.0;
            d2d.draw_text("URL Router", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.router_url_inp.draw(d2d, hits, "set_r_url", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Contoh: http://localhost:11434", false, None, input);
            y += 30.0;
            d2d.draw_text("API Key", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.router_key_inp.draw(d2d, hits, "set_r_key", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Opsional untuk localhost / sk-...", true, None, input);
            y += 30.0;
            d2d.draw_text("Model Default", Rect::new(rx, y, 96.0, 24.0), &TextStyle::default().size(11.0).color(Color::white(0.72)).vcenter());
            state.router_model_inp.draw(d2d, hits, "set_r_mod", Rect::new(rx + 100.0, y, rw - 100.0, 25.0), "Contoh: qwen2.5:7b", false, None, input);
            y += 30.0;
            d2d.draw_line(rx, y + 2.0, rx + rw, y + 2.0, 1.0, Color::white(0.06));
            y += 10.0;

            // Akses AI
            d2d.draw_text(
                "Akses AI",
                Rect::new(rx, y, (rw - 150.0).max(80.0), 17.0),
                &TextStyle::default().size(11.5).medium().color(Color::white(0.88)).vcenter(),
            );
            d2d.draw_text(
                "Auto: izin aksi penting • Manual: minta izin tiap aksi",
                Rect::new(rx, y + 17.0, (rw - 150.0).max(80.0), 15.0),
                &TextStyle::default().size(9.5).color(Color::white(0.45)).ellipsis(),
            );
            if let Some(mode) = draw_segmented(
                d2d,
                hits,
                "set_ai_access",
                Rect::new(rx + rw - 140.0, y + 4.0, 140.0, 26.0),
                &[("auto", "Auto"), ("manual", "Manual")],
                &settings.dn_ai_access_mode,
                None,
                input,
            ) {
                patch_str(store, ui, "dnAiAccessMode", &mode);
            }
            y += 42.0;

            if draw_pill_btn(
                d2d,
                hits,
                "set_r_save",
                Rect::new(rx, y, 88.0, 24.0),
                "Simpan AI",
                true,
                Some(accent),
                input,
            ) {
                let mut m = Map::new();
                m.insert(
                    "dnRouterBaseUrl".to_string(),
                    Value::String(state.router_url_inp.text.trim().to_string()),
                );
                m.insert(
                    "dnRouterModel".to_string(),
                    Value::String(state.router_model_inp.text.trim().to_string()),
                );
                m.insert(
                    "dnGeminiModel".to_string(),
                    Value::String(state.gemini_model_inp.text.trim().to_string()),
                );
                m.insert(
                    "dnOpenaiBaseUrl".to_string(),
                    Value::String(state.openai_url_inp.text.trim().to_string()),
                );
                m.insert(
                    "dnOpenaiModel".to_string(),
                    Value::String(state.openai_model_inp.text.trim().to_string()),
                );
                store.patch_settings(Some(ui), m);
                let _ = store.write_router_api_key(Some(ui), state.router_key_inp.text.trim());
                let _ = store.write_gemini_api_key(Some(ui), state.gemini_key_inp.text.trim());
                let _ = store.write_openai_api_key(Some(ui), state.openai_key_inp.text.trim());
                state.router_msg = "Konfigurasi AI tersimpan (DPAPI terenkripsi).".to_string();
            }

            if draw_pill_btn(
                d2d,
                hits,
                "set_r_test",
                Rect::new(rx + 96.0, y, 108.0, 24.0),
                "Tes Koneksi",
                false,
                None,
                input,
            ) {
                state.router_msg = "Mengecek semua provider AI...".to_string();
                let st = store.clone();
                let u = ui.clone();
                rt.spawn(async move {
                    let (ok, models, msg) = router::test_connection(&st, Some(&u)).await;
                    u.send(UiEvent::RouterModelsResult { ok, models, msg });
                });
            }

            if draw_pill_btn(
                d2d,
                hits,
                "set_quit_app",
                Rect::new(rx + rw - 104.0, y, 104.0, 24.0),
                "Keluar App",
                false,
                Some(Color::rgb(239, 68, 68)),
                input,
            ) {
                let h = ui.get_hwnd();
                if h.is_valid() {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                            h.0 as windows_sys::Win32::Foundation::HWND,
                            0x0010, // WM_CLOSE
                            0,
                            0,
                        );
                    }
                }
            }
            y += 32.0;

            if !state.router_msg.is_empty() {
                d2d.draw_text(
                    &state.router_msg,
                    Rect::new(rx, y, rw, 18.0),
                    &TextStyle::default().size(10.5).color(Color::white(0.76)).ellipsis(),
                );
                y += 22.0;
            }
            if !state.router_models.is_empty() {
                let mut mx = rx;
                for (i, md) in state.router_models.iter().take(4).enumerate() {
                    if mx + 112.0 > rx + rw {
                        break;
                    }
                    if draw_pill_btn(
                        d2d,
                        hits,
                        &format!("set_rmod_{}", i),
                        Rect::new(mx, y, 110.0, 22.0),
                        md,
                        state.router_model_inp.text == *md,
                        Some(accent),
                        input,
                    ) {
                        state.router_model_inp.set_text(md);
                        patch_str(store, ui, "dnRouterModel", md);
                    }
                    mx += 116.0;
                }
                y += 30.0;
            }
            y += 6.0 + 12.0;
        }
    }

    let actual_h = (y - start_y + 16.0).max(inner.h);
    state.measured_h[s_idx] = actual_h;
    state.scroll.update_bounds(inner.h, actual_h, input);

    hits.pop_clip();
    d2d.pop_clip_rect();
    state.scroll.draw_v_bar(d2d, inner, input);
}

fn group_label(d2d: &mut D2dEngine, x: f32, y: f32, w: f32, title: &str) -> f32 {
    d2d.draw_text(
        title,
        Rect::new(x + 2.0, y, w - 4.0, 16.0),
        &TextStyle::default().size(11.0).medium().color(Color::white(0.45)),
    );
    y + 21.0
}

fn draw_card_box(d2d: &mut D2dEngine, rect: Rect) {
    d2d.fill_rounded_rect(rect, 12.0, Color::white(0.045));
}

#[allow(clippy::too_many_arguments)]
fn grouped_toggle_row(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    row_r: Rect,
    label: &str,
    detail: Option<&str>,
    checked: bool,
    disabled: bool,
    input: &UiInput,
) -> Option<bool> {
    let h = hits.register(
        id,
        row_r,
        if disabled { CursorKind::Arrow } else { CursorKind::Pointer },
        None,
        input,
    );
    if h.hovered && !disabled {
        d2d.fill_rounded_rect(row_r.inset(2.0, 2.0), 8.0, Color::white(0.03));
    }

    let tx = row_r.x + 12.0;
    let tw = (row_r.w - 60.0).max(80.0);
    if let Some(sub) = detail {
        d2d.draw_text(
            label,
            Rect::new(tx, row_r.y + 7.0, tw, 17.0),
            &TextStyle::default()
                .size(12.0)
                .medium()
                .color(Color::white(if disabled { 0.45 } else { 0.92 }))
                .vcenter()
                .ellipsis(),
        );
        d2d.draw_text(
            sub,
            Rect::new(tx, row_r.y + 24.0, tw, 15.0),
            &TextStyle::default()
                .size(10.5)
                .color(Color::white(0.42))
                .vcenter()
                .ellipsis(),
        );
    } else {
        d2d.draw_text(
            label,
            Rect::new(tx, row_r.y, tw, row_r.h),
            &TextStyle::default()
                .size(12.0)
                .medium()
                .color(Color::white(if disabled { 0.45 } else { 0.92 }))
                .vcenter()
                .ellipsis(),
        );
    }

    let sw_r = Rect::new(row_r.right() - 42.0, row_r.center_y() - 9.0, 30.0, 18.0);
    draw_switch_visual(d2d, hits, id, sw_r, checked, h.hovered, disabled);

    if h.clicked && !disabled {
        Some(!checked)
    } else {
        None
    }
}

fn pick_photo_bg(hwnd: HwndHandle, ui: &UiSender) {
    let u = ui.clone();
    std::thread::spawn(move || {
        if let Some(data_or_path) = system::photo_pick(hwnd) {
            u.send(UiEvent::PhotoPicked(data_or_path));
        }
    });
}
