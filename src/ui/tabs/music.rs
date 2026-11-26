use crate::backend::{audio_out, smtc};
use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::events::{AudioDeviceKind, AudioOutputSnapshot, SmtcSessionInfo, UiSender};
use crate::logic::lyrics::LyricsState;
use crate::state::Settings;
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{
    draw_frost_card, draw_icon_btn, draw_pill_btn, CursorKind, HitRegistry, ScrollState, UiInput,
};
use serde_json::{Map, Value};

#[derive(Default)]
pub struct MusicTabState {
    pub lyr_scroll: ScrollState,
    pub manual_scroll_until_ms: u64,
    pub last_idx: i32,
    pub scrub_frac: Option<f32>,
    pub audio_picker_open: bool,
    pub audio_snapshot: AudioOutputSnapshot,
    pub audio_scroll: ScrollState,
    pub vol_scrub: Option<f32>,
    pub last_vol_send_ms: u64,
}

pub fn target_music_size(
    d2d: &mut D2dEngine,
    media: Option<&SmtcSessionInfo>,
    lyrics: &LyricsState,
    state: &MusicTabState,
) -> (f32, f32) {
    if state.audio_picker_open {
        let rows = state.audio_snapshot.devices.len().clamp(2, 4) as f32;
        let h = (96.0 + rows * 50.0).clamp(196.0, 296.0);
        return (540.0, h);
    }

    let has_song = media.map(|m| !m.title.is_empty()).unwrap_or(false);
    if !has_song || lyrics.status != "synced" || lyrics.lines.is_empty() {
        return (600.0, 196.0);
    }

    let len = lyrics.lines.len();
    let center = if lyrics.cur_idx >= 0 {
        (lyrics.cur_idx as usize).min(len - 1)
    } else {
        0
    };
    let start = center.saturating_sub(1);
    let end = (center + 2).min(len - 1);

    let active_st = TextStyle::default().size(15.5).bold();
    let other_st = TextStyle::default().size(13.5).medium();

    let mut max_line_w = 0.0f32;
    for (idx, line) in lyrics.lines.iter().enumerate().take(end + 1).skip(start) {
        let txt = line.text.trim();
        if txt.is_empty() {
            continue;
        }
        let st = if lyrics.cur_idx >= 0 && idx == lyrics.cur_idx as usize {
            &active_st
        } else {
            &other_st
        };
        let (tw, _) = d2d.measure_text(txt, 1400.0, st);
        if tw > max_line_w {
            max_line_w = tw;
        }
    }

    // 16px left + 222px player col + 24px gap + 24px right padding/scrollbar = 286px fixed
    let target_w = (286.0 + max_line_w).clamp(600.0, 860.0);
    (target_w, 196.0)
}

fn fmt_mmss(ms: u64) -> String {
    let sec = ms / 1000;
    format!("{}:{:02}", sec / 60, sec % 60)
}

fn fmt_remaining(cur_ms: u64, dur_ms: u64) -> String {
    if dur_ms == 0 {
        "-:--".to_string()
    } else {
        let rem_sec = dur_ms.saturating_sub(cur_ms) / 1000;
        format!("-{}:{:02}", rem_sec / 60, rem_sec % 60)
    }
}

fn draw_three_dots(d2d: &mut D2dEngine, x: f32, cy: f32, active: bool, now_ms: u64) {
    let t = now_ms as f32 / 1000.0;
    for i in 0..3 {
        let pulse = if active {
            0.35 + ((t * 3.0 - i as f32 * 0.75).sin() * 0.5 + 0.5) * 0.45
        } else {
            0.38
        };
        d2d.fill_circle(x + i as f32 * 9.0, cy, 2.5, Color::white(pulse));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_audio_output_panel(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    content: Rect,
    state: &mut MusicTabState,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    // Top volume slider bar matching Apple Music / macOS HUD look (Volume icon + white track + % + back button)
    let vol_row = Rect::new(content.x + 2.0, content.y + 4.0, content.w - 4.0, 28.0);
    let mute_r = Rect::new(vol_row.x, vol_row.y + 2.0, 24.0, 24.0);
    let mute_col = if state.audio_snapshot.muted {
        Color::rgb(248, 113, 113)
    } else {
        Color::white(0.90)
    };
    if draw_icon_btn(
        d2d,
        hits,
        "music_audio_mute",
        mute_r,
        IconId::Volume,
        15.0,
        mute_col,
        Color::white(0.10),
        Some(if state.audio_snapshot.muted {
            "Nyalakan suara (Unmute)"
        } else {
            "Bisukan suara (Mute)"
        }),
        input,
    ) {
        state.audio_snapshot.muted = !state.audio_snapshot.muted;
        audio_out::toggle_mute_and_refresh(rt, ui);
    }

    let back_r = Rect::new(vol_row.right() - 28.0, vol_row.y, 28.0, 28.0);
    let back_hit = hits.register(
        "music_audio_close",
        back_r,
        CursorKind::Pointer,
        Some("Kembali ke pemutar musik".to_string()),
        input,
    );
    d2d.fill_circle(
        back_r.center_x(),
        back_r.center_y(),
        13.5,
        Color::white(if back_hit.hovered { 0.24 } else { 0.14 }),
    );
    d2d.fill_circle(
        back_r.center_x(),
        back_r.center_y(),
        9.5,
        Color::white(if back_hit.hovered { 1.0 } else { 0.92 }),
    );
    if back_hit.hovered {
        d2d.draw_icon(
            IconId::Music,
            Rect::new(back_r.center_x() - 5.5, back_r.center_y() - 5.5, 11.0, 11.0),
            Color::black(0.88),
            2.0,
            1.0,
        );
    }
    if back_hit.clicked {
        state.audio_picker_open = false;
        state.vol_scrub = None;
        return;
    }

    let pct_r = Rect::new(back_r.x - 42.0, vol_row.y, 36.0, 28.0);
    let slider_x = mute_r.right() + 10.0;
    let slider_w = (pct_r.x - 10.0 - slider_x).max(60.0);
    let slider_hit_r = Rect::new(slider_x, vol_row.y + 4.0, slider_w, 20.0);
    let sl_h = hits.register("music_audio_vol", slider_hit_r, CursorKind::Pointer, None, input);

    if sl_h.down || (state.vol_scrub.is_some() && input.left_down) {
        let f = ((input.mouse_x - slider_x) / slider_w).clamp(0.0, 1.0);
        state.vol_scrub = Some(f);
        state.audio_snapshot.volume_pct = (f * 100.0).round() as u8;
        if f > 0.01 {
            state.audio_snapshot.muted = false;
        }
        if input.now_ms.saturating_sub(state.last_vol_send_ms) >= 65 {
            state.last_vol_send_ms = input.now_ms;
            audio_out::set_volume_and_refresh(rt, ui, f);
        }
    } else if let Some(f) = state.vol_scrub.take() {
        state.audio_snapshot.volume_pct = (f * 100.0).round() as u8;
        audio_out::set_volume_and_refresh(rt, ui, f);
    } else if sl_h.clicked {
        let f = ((input.mouse_x - slider_x) / slider_w).clamp(0.0, 1.0);
        state.audio_snapshot.volume_pct = (f * 100.0).round() as u8;
        if f > 0.01 {
            state.audio_snapshot.muted = false;
        }
        audio_out::set_volume_and_refresh(rt, ui, f);
    }

    let vol_frac = state
        .vol_scrub
        .unwrap_or_else(|| (state.audio_snapshot.volume_pct as f32 / 100.0).clamp(0.0, 1.0));
    d2d.fill_rounded_rect(
        Rect::new(slider_x, slider_hit_r.center_y() - 2.0, slider_w, 4.0),
        2.0,
        Color::white(0.20),
    );
    if vol_frac > 0.005 {
        d2d.fill_rounded_rect(
            Rect::new(slider_x, slider_hit_r.center_y() - 2.0, slider_w * vol_frac, 4.0),
            2.0,
            if state.audio_snapshot.muted {
                Color::white(0.42)
            } else {
                Color::white(0.95)
            },
        );
    }
    if sl_h.hovered || state.vol_scrub.is_some() {
        d2d.fill_circle(
            slider_x + slider_w * vol_frac,
            slider_hit_r.center_y(),
            5.2,
            Color::white(0.98),
        );
    }

    let pct_label = if state.audio_snapshot.muted {
        "Mute".to_string()
    } else {
        format!("{}%", state.audio_snapshot.volume_pct)
    };
    d2d.draw_text(
        &pct_label,
        pct_r,
        &TextStyle::default()
            .size(11.0)
            .medium()
            .color(Color::white(0.76))
            .right()
            .vcenter(),
    );

    // Output devices list
    let list_r = Rect::new(
        content.x,
        vol_row.bottom() + 8.0,
        content.w,
        (content.bottom() - (vol_row.bottom() + 8.0)).max(56.0),
    );
    if state.audio_snapshot.devices.is_empty() {
        d2d.draw_text(
            "Memindai perangkat output audio...",
            list_r,
            &TextStyle::default()
                .size(12.0)
                .color(Color::white(0.50))
                .center()
                .vcenter(),
        );
        return;
    }

    let row_h = 48.0;
    let total_h = state.audio_snapshot.devices.len() as f32 * row_h;
    state.audio_scroll.update_bounds(list_r.h, total_h, input);
    state.audio_scroll.handle_wheel(list_r, false, input);

    d2d.push_clip_rect(list_r);
    hits.push_clip(list_r);
    let mut y = list_r.y - state.audio_scroll.offset;
    let mut clicked_id: Option<String> = None;

    for (idx, dev) in state.audio_snapshot.devices.iter().enumerate() {
        let rr = Rect::new(list_r.x, y, list_r.w - 6.0, row_h - 2.0);
        if rr.bottom() >= list_r.y && rr.y <= list_r.bottom() {
            let rh = hits.register(
                format!("music_audio_dev_{}", idx),
                rr,
                CursorKind::Pointer,
                Some("Pilih sebagai output suara utama".to_string()),
                input,
            );
            let bg_a = if rh.hovered {
                0.10
            } else if dev.is_default {
                0.05
            } else {
                0.0
            };
            if bg_a > 0.0 {
                d2d.fill_rounded_rect(rr, 10.0, Color::white(bg_a));
            }
            let icon = match dev.kind {
                AudioDeviceKind::Bluetooth | AudioDeviceKind::Headphones => IconId::Headphones,
                AudioDeviceKind::Monitor => IconId::Monitor,
                AudioDeviceKind::Speaker => IconId::Volume,
            };
            d2d.draw_icon(
                icon,
                Rect::new(rr.x + 10.0, rr.center_y() - 9.0, 18.0, 18.0),
                Color::white(if dev.is_default { 0.96 } else { 0.72 }),
                1.9,
                1.0,
            );

            let tx = rr.x + 38.0;
            let tw = (rr.w - 72.0).max(80.0);
            d2d.draw_text(
                &dev.title,
                Rect::new(tx, rr.y + 6.0, tw, 18.0),
                &TextStyle::default()
                    .size(12.5)
                    .semibold()
                    .color(Color::white(0.96))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                &dev.subtitle,
                Rect::new(tx, rr.y + 24.0, tw, 15.0),
                &TextStyle::default()
                    .size(10.5)
                    .color(Color::white(0.48))
                    .vcenter()
                    .ellipsis(),
            );

            if dev.is_default {
                d2d.draw_icon(
                    IconId::Check,
                    Rect::new(rr.right() - 24.0, rr.center_y() - 7.0, 14.0, 14.0),
                    Color::white(0.94),
                    2.2,
                    1.0,
                );
            }
            if rh.clicked {
                clicked_id = Some(dev.id.clone());
            }
        }
        y += row_h;
    }

    if let Some(id) = clicked_id {
        for d in &mut state.audio_snapshot.devices {
            d.is_default = d.id == id;
        }
        audio_out::select_device_and_refresh(rt, ui, &id);
    }

    hits.pop_clip();
    d2d.pop_clip_rect();
    state.audio_scroll.draw_v_bar(d2d, list_r, input);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_music(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    media: Option<&SmtcSessionInfo>,
    lyrics: &mut LyricsState,
    accent: Color,
    state: &mut MusicTabState,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    let content = Rect::new(
        notch_rect.x + 16.0,
        notch_rect.y + 38.0,
        notch_rect.w - 32.0,
        (notch_rect.h - 48.0).max(96.0),
    );

    if state.audio_picker_open {
        draw_audio_output_panel(d2d, hits, content, state, ui, rt, input);
        return;
    }

    let Some(m) = media.filter(|s| !s.title.is_empty()) else {
        draw_frost_card(d2d, content, false, None);
        d2d.draw_icon(
            IconId::Music,
            Rect::new(content.center_x() - 16.0, content.center_y() - 34.0, 32.0, 32.0),
            Color::white(0.28),
            1.6,
            1.0,
        );
        d2d.draw_text(
            "Tidak ada media yang sedang diputar",
            Rect::new(content.x + 20.0, content.center_y() + 2.0, content.w - 40.0, 22.0),
            &TextStyle::default()
                .size(12.0)
                .color(Color::white(0.45))
                .center(),
        );
        if draw_pill_btn(
            d2d,
            hits,
            "music_empty_audio_out",
            Rect::new(content.center_x() - 56.0, content.center_y() + 28.0, 112.0, 22.0),
            "Output Audio",
            false,
            Some(accent),
            input,
        ) {
            state.audio_picker_open = true;
            audio_out::refresh_outputs_async(rt, ui);
        }
        return;
    };

    let tint = m
        .accent_tint
        .map(|(r, g, b)| Color::rgb(r, g, b))
        .unwrap_or(accent);

    let left_w = 222.0_f32.min((content.w * 0.44).max(176.0));
    let left = Rect::new(content.x, content.y, left_w, content.h);
    let right = Rect::new(
        left.right() + 24.0,
        content.y,
        (content.right() - (left.right() + 24.0)).max(120.0),
        content.h,
    );

    // Top-left row: 58x58 Album Art + Title / Artist / 4-bar Equalizer
    let art_size = 58.0;
    let art_r = Rect::new(left.x, left.y + 2.0, art_size, art_size);
    let art_hit = hits.register(
        "music_art_focus",
        art_r,
        CursorKind::Pointer,
        Some("Fokuskan aplikasi pemutar".to_string()),
        input,
    );
    if let Some(thumb) = m.thumbnail_bytes.as_deref().filter(|b| !b.is_empty()) {
        let key = format!("smtc:{}:{}:{}", m.artist, m.album, thumb.len());
        d2d.draw_image_bytes(&key, thumb, art_r, 11.0, 1.0, true);
    } else {
        d2d.fill_rounded_rect(art_r, 11.0, Color::white(0.09));
        d2d.draw_icon(IconId::Music, art_r.inset(16.0, 16.0), Color::white(0.65), 1.8, 1.0);
    }
    d2d.stroke_rounded_rect(
        art_r,
        11.0,
        1.0,
        Color::white(if art_hit.hovered { 0.28 } else { 0.10 }),
    );
    if art_hit.clicked && !m.source_app_id.is_empty() {
        let app_id = m.source_app_id.clone();
        rt.spawn(async move {
            system::focus_media_app(&app_id).await;
        });
    }

    let meta_x = art_r.right() + 12.0;
    let meta_w = (left.right() - meta_x).max(60.0);
    d2d.draw_text(
        &m.title,
        Rect::new(meta_x, art_r.y + 1.0, meta_w, 19.0),
        &TextStyle::default()
            .size(13.5)
            .bold()
            .color(Color::white(0.96))
            .vcenter()
            .ellipsis(),
    );
    let artist_txt = if m.artist.is_empty() {
        if m.album.is_empty() { "Media" } else { &m.album }
    } else {
        &m.artist
    };
    d2d.draw_text(
        artist_txt,
        Rect::new(meta_x, art_r.y + 20.0, meta_w, 16.0),
        &TextStyle::default()
            .size(11.0)
            .color(Color::white(0.56))
            .vcenter()
            .ellipsis(),
    );

    // Subtle 4-bar equalizer below Artist
    let eq_base_y = art_r.y + 49.0;
    let t = input.now_ms as f32 / 1000.0;
    let eq_col = if m.is_playing {
        Color::white(0.88)
    } else {
        Color::white(0.42)
    };
    for (i, period) in [(0, 0.82f32), (1, 1.12), (2, 0.72), (3, 0.98)] {
        let h = if m.is_playing {
            let osc = ((t / period) * std::f32::consts::TAU + i as f32 * 1.25).sin() * 0.5 + 0.5;
            3.5 + osc * 7.5
        } else {
            match i {
                0 => 4.0,
                1 => 6.0,
                2 => 4.5,
                _ => 3.5,
            }
        };
        let bx = meta_x + i as f32 * 4.5;
        d2d.fill_rounded_rect(Rect::new(bx, eq_base_y - h, 2.2, h), 1.1, eq_col);
    }

    // Inline Seek Row: elapsed  [========---]  -remaining
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
    let live_cur_ms = lyrics.cur_ms();
    let cur_ms = if eff_dur_ms > 0 {
        live_cur_ms.min(eff_dur_ms)
    } else {
        live_cur_ms
    };

    let prog_y = art_r.bottom() + 14.0;
    let sb = Rect::new(left.x + 36.0, prog_y, (left.w - 78.0).max(40.0), 16.0);
    let sb_h = hits.register("music_seek", sb, CursorKind::Pointer, None, input);
    if eff_dur_ms > 0 && (sb_h.down || (state.scrub_frac.is_some() && input.left_down)) {
        let f = ((input.mouse_x - sb.x) / sb.w.max(1.0)).clamp(0.0, 1.0);
        state.scrub_frac = Some(f);
    } else if let Some(f) = state.scrub_frac.take() {
        if eff_dur_ms > 0 {
            let target_ms = (f as f64 * eff_dur_ms as f64) as u64;
            let target_sec = target_ms as f64 / 1000.0;
            lyrics.seek_local(target_ms);
            lyrics.update_active_index(&m.artist, &m.title);
            smtc::seek_and_refresh(rt, ui, target_sec, &m.source_app_id, &m.title);
        }
    } else if sb_h.clicked && eff_dur_ms > 0 {
        let f = ((input.mouse_x - sb.x) / sb.w.max(1.0)).clamp(0.0, 1.0);
        let target_ms = (f as f64 * eff_dur_ms as f64) as u64;
        let target_sec = target_ms as f64 / 1000.0;
        lyrics.seek_local(target_ms);
        lyrics.update_active_index(&m.artist, &m.title);
        smtc::seek_and_refresh(rt, ui, target_sec, &m.source_app_id, &m.title);
    }

    let frac = if let Some(f) = state.scrub_frac {
        f
    } else if eff_dur_ms > 0 {
        (cur_ms as f32 / eff_dur_ms as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let disp_cur_ms = if let Some(f) = state.scrub_frac {
        (f as f64 * eff_dur_ms as f64) as u64
    } else {
        cur_ms
    };

    d2d.draw_text(
        &fmt_mmss(disp_cur_ms),
        Rect::new(left.x, prog_y, 33.0, 16.0),
        &TextStyle::default()
            .size(10.0)
            .color(Color::white(0.56))
            .vcenter(),
    );
    d2d.fill_rounded_rect(
        Rect::new(sb.x, sb.center_y() - 1.75, sb.w, 3.5),
        1.75,
        Color::white(0.20),
    );
    if frac > 0.001 {
        d2d.fill_rounded_rect(
            Rect::new(sb.x, sb.center_y() - 1.75, sb.w * frac, 3.5),
            1.75,
            Color::white(0.92),
        );
    }
    if sb_h.hovered || state.scrub_frac.is_some() {
        d2d.fill_circle(sb.x + sb.w * frac, sb.center_y(), 4.8, Color::white(0.98));
    }
    d2d.draw_text(
        &fmt_remaining(disp_cur_ms, eff_dur_ms),
        Rect::new(left.right() - 40.0, prog_y, 40.0, 16.0),
        &TextStyle::default()
            .size(10.0)
            .color(Color::white(0.56))
            .right()
            .vcenter(),
    );

    // Compact 5-button Transport Row
    let ctrl_y = (prog_y + 24.0).min(left.bottom() - 32.0);
    let slot_w = left.w / 5.0;

    // 1. Closed-notch lyrics pill toggle
    let lyr_btn = Rect::new(left.x + slot_w * 0.5 - 15.0, ctrl_y, 30.0, 30.0);
    let lyr_hit = hits.register(
        "music_lyr_closed_toggle",
        lyr_btn,
        CursorKind::Pointer,
        Some(if settings.lyrics_on_closed {
            "Lirik di notch tertutup: Aktif".to_string()
        } else {
            "Lirik di notch tertutup: Nonaktif".to_string()
        }),
        input,
    );
    if settings.lyrics_on_closed || lyr_hit.hovered {
        let bg_a = if settings.lyrics_on_closed {
            if lyr_hit.hovered { 0.28 } else { 0.20 }
        } else {
            0.10
        };
        d2d.fill_circle(lyr_btn.center_x(), lyr_btn.center_y(), 15.0, Color::white(bg_a));
    }
    d2d.draw_icon(
        IconId::Chat,
        Rect::new(lyr_btn.center_x() - 7.0, lyr_btn.center_y() - 7.0, 14.0, 14.0),
        if settings.lyrics_on_closed {
            Color::white(0.96)
        } else {
            Color::white(0.54)
        },
        1.8,
        1.0,
    );
    if lyr_hit.clicked {
        let next = !settings.lyrics_on_closed;
        let mut p = Map::new();
        p.insert("lyricsOnClosed".to_string(), Value::Bool(next));
        store.patch_settings(Some(ui), p);
        store.set(Some(ui), "lyricsOnClosed", Value::Bool(next));
    }

    // 2. Previous track
    let prev_r = Rect::new(left.x + slot_w * 1.5 - 14.0, ctrl_y + 1.0, 28.0, 28.0);
    if draw_icon_btn(
        d2d,
        hits,
        "music_prev",
        prev_r,
        IconId::SkipBack,
        16.0,
        Color::white(0.90),
        Color::white(0.10),
        Some("Sebelumnya"),
        input,
    ) {
        smtc::media_key_and_refresh(rt, ui, "prev", &m.source_app_id, &m.title);
    }

    // 3. Play / Pause
    let pp_r = Rect::new(left.x + slot_w * 2.5 - 16.0, ctrl_y - 1.0, 32.0, 32.0);
    if draw_icon_btn(
        d2d,
        hits,
        "music_pp",
        pp_r,
        if m.is_playing { IconId::Pause } else { IconId::Play },
        19.0,
        Color::white(0.98),
        Color::white(0.12),
        Some(if m.is_playing { "Jeda" } else { "Putar" }),
        input,
    ) {
        smtc::media_key_and_refresh(rt, ui, "playpause", &m.source_app_id, &m.title);
    }

    // 4. Next track
    let next_r = Rect::new(left.x + slot_w * 3.5 - 14.0, ctrl_y + 1.0, 28.0, 28.0);
    if draw_icon_btn(
        d2d,
        hits,
        "music_next",
        next_r,
        IconId::SkipForward,
        16.0,
        Color::white(0.90),
        Color::white(0.10),
        Some("Berikutnya"),
        input,
    ) {
        smtc::media_key_and_refresh(rt, ui, "next", &m.source_app_id, &m.title);
    }

    // 5. Sound Output & Volume switcher
    let foc_r = Rect::new(left.x + slot_w * 4.5 - 14.0, ctrl_y + 1.0, 28.0, 28.0);
    if draw_icon_btn(
        d2d,
        hits,
        "music_audio_out_btn",
        foc_r,
        IconId::Headphones,
        15.5,
        Color::white(0.84),
        Color::white(0.12),
        Some("Output Audio & Volume"),
        input,
    ) {
        state.audio_picker_open = true;
        audio_out::refresh_outputs_async(rt, ui);
    }

    // Right: Unboxed Synced Lyrics Window
    let lyr_area = Rect::new(right.x, right.y + 4.0, right.w, (right.h - 6.0).max(60.0));
    if lyrics.status == "loading" {
        draw_three_dots(d2d, lyr_area.x + 8.0, lyr_area.center_y() - 10.0, true, input.now_ms);
        d2d.draw_text(
            "Mencari lirik di LRCLIB...",
            Rect::new(lyr_area.x, lyr_area.center_y() + 4.0, lyr_area.w, 20.0),
            &TextStyle::default()
                .size(11.5)
                .color(Color::white(0.42))
                .vcenter(),
        );
        return;
    }
    if lyrics.status == "instrumental" {
        draw_three_dots(d2d, lyr_area.x + 8.0, lyr_area.center_y() - 12.0, m.is_playing, input.now_ms);
        d2d.draw_text(
            "♪ Instrumental",
            Rect::new(lyr_area.x, lyr_area.center_y() + 4.0, lyr_area.w, 22.0),
            &TextStyle::default()
                .size(13.5)
                .medium()
                .color(Color::white(0.65))
                .vcenter(),
        );
        return;
    }
    if lyrics.status == "plain" {
        if let Some(plain) = lyrics.plain.as_deref() {
            let st = TextStyle::default()
                .size(12.5)
                .color(Color::white(0.76))
                .line_height(20.0);
            let (_, ch) = d2d.measure_text(plain, lyr_area.w - 10.0, &st);
            state.lyr_scroll.update_bounds(lyr_area.h, ch + 16.0, input);
            state.lyr_scroll.handle_wheel(lyr_area, false, input);
            d2d.push_clip_rect(lyr_area);
            d2d.draw_text(
                plain,
                Rect::new(
                    lyr_area.x,
                    lyr_area.y - state.lyr_scroll.offset,
                    lyr_area.w - 10.0,
                    ch + 16.0,
                ),
                &st,
            );
            d2d.pop_clip_rect();
            state.lyr_scroll.draw_v_bar(d2d, lyr_area, input);
            return;
        }
    }
    if lyrics.lines.is_empty() {
        d2d.draw_text(
            "Lirik tidak tersedia untuk lagu ini",
            lyr_area,
            &TextStyle::default()
                .size(12.5)
                .color(Color::white(0.42))
                .vcenter(),
        );
        return;
    }

    let line_h = 34.0;
    let total_h = lyrics.lines.len() as f32 * line_h + lyr_area.h * 0.55;
    state.lyr_scroll.update_bounds(lyr_area.h, total_h, input);
    if state.lyr_scroll.handle_wheel(lyr_area, false, input) {
        state.manual_scroll_until_ms = input.now_ms + 4000;
    }

    // Auto-scroll so one dimmed previous line sits above the bold active line
    if lyrics.cur_idx >= 0
        && lyrics.cur_idx != state.last_idx
        && input.now_ms >= state.manual_scroll_until_ms
    {
        state.last_idx = lyrics.cur_idx;
        let desired = (lyrics.cur_idx as f32 * line_h - line_h).max(0.0);
        state.lyr_scroll.target = desired;
    }

    d2d.push_clip_rect(lyr_area);
    let mut jump_to_ms: Option<u64> = None;
    let mut y = lyr_area.y + 8.0 - state.lyr_scroll.offset;
    for (idx, line) in lyrics.lines.iter().enumerate() {
        let lr = Rect::new(lyr_area.x, y, lyr_area.w - 8.0, line_h);
        if lr.bottom() >= lyr_area.y && lr.y <= lyr_area.bottom() {
            let is_cur = idx as i32 == lyrics.cur_idx;
            let is_past = lyrics.cur_idx >= 0 && (idx as i32) < lyrics.cur_idx;
            let lh = hits.register(
                format!("lyr_line_{}", idx),
                lr,
                CursorKind::Pointer,
                Some("Klik untuk lompat ke baris ini".to_string()),
                input,
            );
            let txt = line.text.trim();
            if txt.is_empty() {
                draw_three_dots(
                    d2d,
                    lr.x + 4.0,
                    lr.center_y(),
                    is_cur && m.is_playing,
                    input.now_ms,
                );
            } else {
                let col = if is_cur {
                    Color::white(0.98)
                } else if lh.hovered {
                    Color::white(0.84)
                } else if is_past {
                    Color::white(0.32)
                } else {
                    Color::white(0.46)
                };
                let st = if is_cur {
                    TextStyle::default()
                        .size(15.5)
                        .bold()
                        .color(col)
                        .vcenter()
                        .ellipsis()
                } else {
                    TextStyle::default()
                        .size(13.5)
                        .medium()
                        .color(col)
                        .vcenter()
                        .ellipsis()
                };
                d2d.draw_text(txt, lr, &st);
            }
            if lh.clicked && eff_dur_ms > 0 {
                jump_to_ms = Some(line.t_ms);
                let sec = line.t_ms as f64 / 1000.0;
                smtc::seek_and_refresh(rt, ui, sec, &m.source_app_id, &m.title);
            }
        }
        y += line_h;
    }
    if let Some(ms) = jump_to_ms {
        lyrics.seek_local(ms);
        lyrics.update_active_index(&m.artist, &m.title);
    }
    d2d.pop_clip_rect();
    state.lyr_scroll.draw_v_bar(d2d, lyr_area, input);

    // Subtle sync offset controls shown on right-pane hover or when non-zero offset is active
    let off_ms = lyrics.cur_offset_ms(&m.artist, &m.title);
    let hover_lyr = input.mouse_inside && lyr_area.contains(input.mouse_x, input.mouse_y);
    if hover_lyr || off_ms != 0 {
        let pill_w = 96.0;
        let pill_r = Rect::new(lyr_area.right() - pill_w - 2.0, lyr_area.bottom() - 22.0, pill_w, 20.0);
        d2d.fill_rounded_rect(pill_r, 10.0, Color::rgba(14, 14, 18, 0.72));
        d2d.stroke_rounded_rect(pill_r, 10.0, 1.0, Color::white(0.10));
        if draw_pill_btn(
            d2d,
            hits,
            "music_off_m",
            Rect::new(pill_r.x + 2.0, pill_r.y + 1.0, 22.0, 18.0),
            "-",
            false,
            Some(tint),
            input,
        ) {
            lyrics.adjust_offset(&m.artist, &m.title, -500, store, Some(ui));
        }
        d2d.draw_text(
            &format!("{:+.1}s", off_ms as f32 / 1000.0),
            Rect::new(pill_r.x + 24.0, pill_r.y, 48.0, 20.0),
            &TextStyle::default()
                .size(9.5)
                .color(Color::white(0.70))
                .center()
                .vcenter(),
        );
        if draw_pill_btn(
            d2d,
            hits,
            "music_off_p",
            Rect::new(pill_r.right() - 24.0, pill_r.y + 1.0, 22.0, 18.0),
            "+",
            false,
            Some(tint),
            input,
        ) {
            lyrics.adjust_offset(&m.artist, &m.title, 500, store, Some(ui));
        }
    }
}
