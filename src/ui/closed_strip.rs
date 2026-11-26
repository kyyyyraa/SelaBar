use crate::events::{ActiveCallInfo, PrivacySnapshot, SmtcSessionInfo};
use crate::logic::lyrics::LyricsState;
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{CursorKind, HitRegistry, UiInput};
use chrono::Local;
use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct HardwareMoment {
    pub kind: String,
    pub name: String,
    pub detail: String,
    pub accent: Color,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ClosedStripState<'a> {
    pub tucked: bool,
    pub hovered: bool,
    pub focus_running: bool,
    pub focus_remaining_sec: u32,
    pub focus_total_sec: u32,
    pub focus_progress: f32,
    pub avatar_color: Color,
    pub media: Option<&'a SmtcSessionInfo>,
    pub lyrics: &'a LyricsState,
    pub closed_lyrics_enabled: bool,
    pub privacy: &'a PrivacySnapshot,
    pub active_call: Option<&'a ActiveCallInfo>,
    pub battery_pct: Option<u32>,
    pub battery_charging: bool,
    pub hardware_moment: Option<&'a HardwareMoment>,
    pub moment_now_ms: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ClosedStripAction {
    pub expand_clicked: bool,
}

/// Computes target closed shell width (`240px` baseline, dynamically widened to `clamp(mid_px + 190, 240, 1000)` when `#dn-lyr-mid` is active).
pub fn target_closed_width(d2d: &mut D2dEngine, state: &ClosedStripState<'_>) -> f32 {
    if state.tucked {
        return 240.0;
    }
    if state.hardware_moment.is_some() {
        return 264.0;
    }
    if state.active_call.is_some() {
        return 260.0;
    }
    if state.lyrics.mid_active(state.closed_lyrics_enabled) {
        if let Some(m) = state.media {
            if m.is_playing {
                let mid = state.lyrics.current_line_text();
                let st = TextStyle::default().size(11.0).medium().nowrap();
                let (tw, _) = d2d.measure_text(mid, 780.0, &st);
                return (tw + 190.0).clamp(240.0, 1000.0);
            }
        }
    }
    240.0
}

pub fn draw_closed_strip(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    bottom_radius: f32,
    ear_radius: f32,
    state: &ClosedStripState<'_>,
    input: &UiInput,
) -> ClosedStripAction {
    let hit = hits.register(
        "closed_notch",
        notch_rect,
        CursorKind::Pointer,
        None,
        input,
    );
    let act = ClosedStripAction {
        expand_clicked: hit.clicked,
    };

    if state.tucked || notch_rect.h < 14.0 {
        return act;
    }

    d2d.push_notch_clip(notch_rect, bottom_radius, ear_radius);
    let inner = Rect::new(notch_rect.x + 16.0, notch_rect.y, (notch_rect.w - 32.0).max(40.0), notch_rect.h);
    let t = input.now_ms as f32 / 1000.0;

    // 1. Transient Hardware / Privacy Moment (1600ms)
    if let Some(mom) = state.hardware_moment {
        if state.moment_now_ms < mom.expires_at_ms {
            let icon = match mom.kind.as_str() {
                "cam" | "camera" => IconId::Camera,
                "mic" => IconId::Mic,
                "vol" | "volume" => IconId::Volume,
                "bt" | "bluetooth" => IconId::Bluetooth,
                "wifi" => IconId::Wifi,
                _ => IconId::Zap,
            };
            d2d.draw_icon(
                icon,
                Rect::new(inner.x, inner.center_y() - 7.0, 14.0, 14.0),
                mom.accent,
                1.9,
                1.0,
            );
            d2d.draw_text(
                &mom.name,
                Rect::new(inner.x + 20.0, inner.y, inner.w - 88.0, inner.h),
                &TextStyle::default()
                    .size(10.5)
                    .medium()
                    .color(Color::white(0.92))
                    .vcenter()
                    .nowrap()
                    .ellipsis(),
            );
            d2d.draw_text(
                &mom.detail,
                Rect::new(inner.right() - 68.0, inner.y, 68.0, inner.h),
                &TextStyle::default()
                    .size(10.0)
                    .medium()
                    .color(Color::white(0.55))
                    .right()
                    .vcenter()
                    .nowrap(),
            );
            d2d.pop_layer();
            return act;
        }
    }

    // 2. Active Voice / Video Call Strip (green phone left + caller middle + live MM:SS timer right)
    if let Some(call) = state.active_call {
        let pulse = (t * 3.2).sin() * 0.5 + 0.5;
        let phone_cx = inner.x + 9.0;
        let phone_cy = inner.center_y();
        d2d.fill_circle(
            phone_cx,
            phone_cy,
            10.5,
            Color::rgb(34, 197, 94).with_alpha(0.22 + pulse * 0.18),
        );
        d2d.fill_circle(phone_cx, phone_cy, 8.2, Color::rgb(34, 197, 94));
        d2d.draw_icon(
            IconId::Phone,
            Rect::new(phone_cx - 4.8, phone_cy - 4.8, 9.6, 9.6),
            Color::white(0.98),
            2.1,
            1.0,
        );

        let secs = input.now_ms.saturating_sub(call.started_at_ms) / 1000;
        let mm = secs / 60;
        let ss = secs % 60;
        let dur_txt = format!("{:02}:{:02}", mm, ss);
        let right_end = draw_right_dots_only(d2d, hits, inner, state, input);

        let st_dur = TextStyle::default()
            .size(10.5)
            .semibold()
            .color(Color::rgb(74, 222, 128))
            .right()
            .vcenter()
            .nowrap();
        let (dw, _) = d2d.measure_text(&dur_txt, 60.0, &st_dur);
        let dur_x = (right_end - dw - 2.0).max(inner.x + 110.0);
        d2d.draw_text(&dur_txt, Rect::new(dur_x, inner.y, dw + 2.0, inner.h), &st_dur);

        let mid_x = phone_cx + 14.0;
        let mid_w = (dur_x - mid_x - 8.0).max(0.0);
        if mid_w > 28.0 {
            d2d.draw_text(
                &call.caller,
                Rect::new(mid_x, inner.y, mid_w, inner.h),
                &TextStyle::default()
                    .size(10.5)
                    .medium()
                    .color(Color::white(0.84))
                    .vcenter()
                    .ellipsis(),
            );
        }

        d2d.pop_layer();
        return act;
    }

    // 3. Focus Running Strip (11x11 ring on left + M:SS, optional closed lyric, clock + dots on right)
    if state.focus_running {
        let total = state.focus_total_sec.max(1) as f32;
        let rem = state.focus_remaining_sec.min(state.focus_total_sec) as f32;
        let prog = if state.focus_progress >= 0.0 {
            state.focus_progress.clamp(0.0, 1.0)
        } else {
            1.0 - rem / total
        };
        let ring_cx = inner.x + 5.5;
        d2d.draw_ring_arc(
            ring_cx,
            inner.center_y(),
            4.5,
            prog,
            2.0,
            Color::white(0.18),
            state.avatar_color,
        );
        let mm = state.focus_remaining_sec / 60;
        let ss = state.focus_remaining_sec % 60;
        let timer_str = format!("{}:{:02}", mm, ss);
        d2d.draw_text(
            &timer_str,
            Rect::new(ring_cx + 10.0, inner.y, 44.0, inner.h),
            &TextStyle::default()
                .size(10.0)
                .semibold()
                .color(Color::white(0.84))
                .vcenter()
                .nowrap(),
        );
        let dots_on = state.privacy.mic || state.privacy.camera;
        // Privacy dots: the clock yields its space (lyric or title uses it).
        let right_end = if dots_on {
            draw_right_dots_only(d2d, hits, inner, state, input)
        } else {
            draw_right_status(d2d, hits, inner, state, input)
        };
        if let Some(m) = state.media {
            if state.lyrics.mid_active(state.closed_lyrics_enabled) && m.is_playing {
                let mid_left = ring_cx + 52.0;
                let mid_w = (right_end - mid_left - 6.0).max(0.0);
                if mid_w > 28.0 {
                    d2d.draw_text(
                        state.lyrics.current_line_text(),
                        Rect::new(mid_left, inner.y, mid_w, inner.h),
                        &TextStyle::default()
                            .size(11.0)
                            .semibold()
                            .color(Color::white(0.94))
                            .center()
                            .vcenter()
                            .ellipsis(),
                    );
                }
            }
        }
        d2d.pop_layer();
        return act;
    }

    // 4. Active Media Strip (playing or paused <90s)
    if let Some(m) = state.media {
        if !m.title.is_empty() {
            let art_r = Rect::new(inner.x, inner.center_y() - 8.0, 16.0, 16.0);
            if let Some(thumb) = m.thumbnail_bytes.as_deref().filter(|b| !b.is_empty()) {
                let key = format!("smtc:{}:{}:{}", m.artist, m.album, thumb.len());
                d2d.draw_image_bytes(&key, thumb, art_r, 4.0, 1.0, true);
            } else {
                d2d.fill_rounded_rect(art_r, 4.0, Color::white(0.10));
                d2d.draw_icon(
                    IconId::Music,
                    art_r.inset(3.0, 3.0),
                    Color::white(0.78),
                    1.8,
                    1.0,
                );
            }

            let eq_x = art_r.right() + 7.0;
            let bar_rgb = m.accent_tint.unwrap_or((255, 255, 255));
            let eq_col = Color::rgb(bar_rgb.0, bar_rgb.1, bar_rgb.2);
            if m.is_playing {
                // 3-bar equalizer (0.85s, 1.15s, 0.95s)
                for (i, period) in [(0, 0.85f32), (1, 1.15), (2, 0.95)] {
                    let h = 3.5 + ((t / period * 2.0 * PI + i as f32 * 1.1).sin() * 0.5 + 0.5) * 7.5;
                    let bx = eq_x + i as f32 * 3.5;
                    let by = inner.center_y() - h * 0.5;
                    d2d.fill_rounded_rect(Rect::new(bx, by, 2.0, h), 1.0, eq_col);
                }
            } else {
                // Paused 11x11 bars
                let py = inner.center_y() - 4.0;
                d2d.fill_rounded_rect(Rect::new(eq_x + 1.0, py, 2.2, 8.0), 1.0, Color::white(0.55));
                d2d.fill_rounded_rect(Rect::new(eq_x + 5.2, py, 2.2, 8.0), 1.0, Color::white(0.55));
            }

            // Right utility cluster: with privacy dots the clock yields its
            // space; otherwise the lyric stays centered in the middle strip.
            let lyric_on = state.lyrics.mid_active(state.closed_lyrics_enabled) && m.is_playing;
            let dots_on = state.privacy.mic || state.privacy.camera;
            let show_clock = !dots_on;
            let dots_end = draw_right_dots_only(d2d, hits, inner, state, input);
            let right_end = if show_clock {
                let clock_w =
                    draw_clock_12h(d2d, Rect::new(dots_end - 64.0, inner.y, 64.0, inner.h), false);
                dots_end - clock_w
            } else {
                dots_end
            };

            let mid_left = eq_x + 15.0;
            let mid_w = (right_end - mid_left - 6.0).max(0.0);
            if mid_w > 28.0 {
                if lyric_on {
                    let lyric = state.lyrics.current_line_text();
                    let st = TextStyle::default()
                        .size(11.0)
                        .semibold()
                        .color(Color::white(0.94))
                        .center()
                        .vcenter()
                        .ellipsis();
                    d2d.draw_text(
                        lyric,
                        Rect::new(mid_left, inner.y, mid_w, inner.h),
                        &st,
                    );
                } else if notch_rect.w > 260.0 {
                    d2d.draw_text(
                        &m.title,
                        Rect::new(mid_left, inner.y, mid_w, inner.h),
                        &TextStyle::default()
                            .size(10.5)
                            .medium()
                            .color(Color::white(0.68))
                            .center()
                            .vcenter()
                            .ellipsis(),
                    );
                }
            }

            d2d.pop_layer();
            return act;
        }
    }

    // 5. Standard Closed Strip (`10:15 PM` on left, battery <=20% + privacy dots on right)
    draw_clock_12h(d2d, Rect::new(inner.x, inner.y, 76.0, inner.h), true);
    draw_right_dots_only(d2d, hits, inner, state, input);

    d2d.pop_layer();
    act
}

fn draw_clock_12h(d2d: &mut D2dEngine, rect: Rect, align_left: bool) -> f32 {
    let now = Local::now();
    let time_s = now.format("%I:%M").to_string();
    let time_trim = time_s.strip_prefix('0').unwrap_or(&time_s);
    let ampm = now.format("%p").to_string();

    let st_t = TextStyle::default()
        .size(10.5)
        .semibold()
        .color(Color::white(0.75))
        .vcenter()
        .nowrap();
    let st_ap = TextStyle::default()
        .size(8.0)
        .semibold()
        .color(Color::white(0.45))
        .vcenter()
        .nowrap();

    let (tw, _) = d2d.measure_text(time_trim, 80.0, &st_t);
    let (aw, _) = d2d.measure_text(&ampm, 40.0, &st_ap);
    let total_w = tw + 3.0 + aw;

    let sx = if align_left {
        rect.x
    } else {
        rect.right() - total_w
    };
    d2d.draw_text(time_trim, Rect::new(sx, rect.y, tw + 2.0, rect.h), &st_t);
    d2d.draw_text(
        &ampm,
        Rect::new(sx + tw + 3.0, rect.y + 1.0, aw + 2.0, rect.h),
        &st_ap,
    );
    total_w
}

fn draw_right_dots_only(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    inner: Rect,
    state: &ClosedStripState<'_>,
    input: &UiInput,
) -> f32 {
    let mut x = crate::ui::popups::draw_privacy_dots(
        d2d,
        hits,
        "closed_priv",
        inner.right(),
        inner.center_y(),
        state.privacy,
        input,
    );
    if let Some(pct) = state.battery_pct {
        if pct <= 20 && !state.battery_charging {
            let txt = format!("{}%", pct);
            let st = TextStyle::default()
                .size(9.5)
                .semibold()
                .color(Color::rgb(255, 69, 58))
                .right()
                .vcenter()
                .nowrap();
            let (bw, _) = d2d.measure_text(&txt, 40.0, &st);
            x -= bw + 4.0;
            d2d.draw_text(&txt, Rect::new(x, inner.y, bw + 2.0, inner.h), &st);
            x -= 5.0;
        }
    }
    x
}

fn draw_right_status(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    inner: Rect,
    state: &ClosedStripState<'_>,
    input: &UiInput,
) -> f32 {
    let mut x = draw_right_dots_only(d2d, hits, inner, state, input);
    let clock_w = draw_clock_12h(d2d, Rect::new(x - 64.0, inner.y, 64.0, inner.h), false);
    x -= clock_w;
    x
}
