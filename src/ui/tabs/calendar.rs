use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::events::UiSender;
use crate::logic::ask::{format_tanggal_id, month_name_id};
use crate::logic::holidays::holiday_for;
use crate::state::{now_unix_ms, CalendarEvent, ReminderItem};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{
    draw_frost_card, draw_icon_btn, CursorKind, HitRegistry, ScrollState, TextInputState,
    UiInput,
};
use chrono::{Datelike, Local, NaiveDate, TimeZone};

pub struct CalendarTabState {
    pub view_year: i32,
    pub view_month: u32,
    pub selected: NaiveDate,
    pub add_input: TextInputState,
    pub agenda_scroll: ScrollState,
}

impl Default for CalendarTabState {
    fn default() -> Self {
        let today = Local::now().date_naive();
        Self {
            view_year: today.year(),
            view_month: today.month(),
            selected: today,
            add_input: TextInputState::default(),
            agenda_scroll: ScrollState::default(),
        }
    }
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(30)
}

fn parse_event_line(raw: &str, date: NaiveDate) -> Option<(String, String, String, i64, i64, String)> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }

    let meeting = system::detect_meeting_link(text);
    let cleaned_input = if let Some(ref ml) = meeting {
        text.replace(&ml.url, " ")
    } else {
        text.to_string()
    };

    // Match HH:MM or HH.MM, optional "-HH:MM" at end or anywhere in string
    let mut start_str = "09:00".to_string();
    let mut end_str = String::new();
    let mut title_parts: Vec<&str> = Vec::new();

    for tok in cleaned_input.split_whitespace() {
        if let Some((a, b)) = tok.split_once('-') {
            if let (Some(sa), Some(sb)) = (parse_hhmm(a), parse_hhmm(b)) {
                start_str = sa;
                end_str = sb;
                continue;
            }
        }
        if let Some(sa) = parse_hhmm(tok) {
            start_str = sa;
            continue;
        }
        title_parts.push(tok);
    }

    let title = if title_parts.is_empty() {
        meeting
            .as_ref()
            .map(|m| m.provider.label().to_string())
            .unwrap_or_else(|| text.to_string())
    } else {
        title_parts.join(" ")
    };

    let note = meeting.map(|m| m.url).unwrap_or_default();
    let at_ms = to_epoch_ms(date, &start_str);
    let end_at_ms = if !end_str.is_empty() {
        to_epoch_ms(date, &end_str)
    } else {
        0
    };

    Some((title, start_str, end_str, at_ms, end_at_ms, note))
}

fn parse_hhmm(tok: &str) -> Option<String> {
    let clean = tok.trim().replace('.', ":");
    let (h, m) = clean.split_once(':')?;
    let hh: u32 = h.parse().ok()?;
    let mm: u32 = m.parse().ok()?;
    if hh < 24 && mm < 60 {
        Some(format!("{:02}:{:02}", hh, mm))
    } else {
        None
    }
}

fn to_epoch_ms(d: NaiveDate, hhmm: &str) -> i64 {
    let (h, m) = hhmm.split_once(':').unwrap_or(("09", "00"));
    let hh = h.parse::<u32>().unwrap_or(9);
    let mm = m.parse::<u32>().unwrap_or(0);
    d.and_hms_opt(hh, mm, 0)
        .and_then(|ndt| Local.from_local_datetime(&ndt).single())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(now_unix_ms)
}

#[allow(clippy::too_many_arguments)]
pub fn draw_calendar(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    events: &[CalendarEvent],
    reminders: &[ReminderItem],
    state: &mut CalendarTabState,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    let area = Rect::new(
        notch_rect.x + 14.0,
        notch_rect.y + 40.0,
        notch_rect.w - 28.0,
        notch_rect.h - 52.0,
    );
    let left = Rect::new(area.x, area.y, 208.0, area.h);
    let right = Rect::new(left.right() + 12.0, area.y, area.w - 220.0, area.h);

    draw_frost_card(d2d, left, false, None);
    draw_frost_card(d2d, right, false, None);

    // Left: #f87171 uppercase Indonesian month + < > nav
    let hdr_txt = format!(
        "{} {}",
        month_name_id(state.view_month).to_uppercase(),
        state.view_year
    );
    d2d.draw_text(
        &hdr_txt,
        Rect::new(left.x + 14.0, left.y + 10.0, 128.0, 20.0),
        &TextStyle::default()
            .size(11.0)
            .bold()
            .color(Color::rgb(248, 113, 113))
            .vcenter(),
    );

    if draw_icon_btn(
        d2d,
        hits,
        "cal_prev",
        Rect::new(left.right() - 52.0, left.y + 10.0, 20.0, 20.0),
        IconId::ChevronLeft,
        12.0,
        Color::white(0.70),
        Color::white(0.09),
        Some("Bulan sebelumnya"),
        input,
    ) {
        if state.view_month == 1 {
            state.view_month = 12;
            state.view_year -= 1;
        } else {
            state.view_month -= 1;
        }
    }
    if draw_icon_btn(
        d2d,
        hits,
        "cal_next",
        Rect::new(left.right() - 28.0, left.y + 10.0, 20.0, 20.0),
        IconId::ChevronRight,
        12.0,
        Color::white(0.70),
        Color::white(0.09),
        Some("Bulan berikutnya"),
        input,
    ) {
        if state.view_month == 12 {
            state.view_month = 1;
            state.view_year += 1;
        } else {
            state.view_month += 1;
        }
    }

    // Weekday row (Mon-Sun: S S R K J S M — Minggu colored red)
    let gx = left.x + 12.0;
    let cw = (left.w - 24.0) / 7.0;
    let dows = ["S", "S", "R", "K", "J", "S", "M"];
    for (i, d) in dows.iter().enumerate() {
        let is_sun_hdr = i == 6;
        d2d.draw_text(
            d,
            Rect::new(gx + i as f32 * cw, left.y + 33.0, cw, 16.0),
            &TextStyle::default()
                .size(9.5)
                .semibold()
                .color(if is_sun_hdr {
                    Color::rgb(248, 113, 113)
                } else {
                    Color::white(0.35)
                })
                .center()
                .vcenter(),
        );
    }

    // 6x7 date cells
    let today = Local::now().date_naive();
    let first_of_month =
        NaiveDate::from_ymd_opt(state.view_year, state.view_month, 1).unwrap_or(today);
    let offset = first_of_month.weekday().num_days_from_monday() as i64; // Mon=0 .. Sun=6
    let dim = days_in_month(state.view_year, state.view_month);
    let mut gy = left.y + 50.0;
    let ch = 23.0;

    for row in 0..6 {
        for col in 0..7 {
            let cell_idx = (row * 7 + col) as i64 - offset + 1;
            if cell_idx < 1 || cell_idx > dim as i64 {
                continue;
            }
            let day = cell_idx as u32;
            let Some(date) = NaiveDate::from_ymd_opt(state.view_year, state.view_month, day) else {
                continue;
            };
            let date_iso = date.format("%Y-%m-%d").to_string();
            let has_evt = events.iter().any(|e| e.date == date_iso);
            let is_today = date == today;
            let is_sel = date == state.selected;
            let is_sunday = col == 6;
            let holiday = holiday_for(date);

            let cr = Rect::new(gx + col as f32 * cw, gy, cw, ch);
            let dh = hits.register(
                format!("cal_day_{}", day),
                cr,
                CursorKind::Pointer,
                holiday.map(|h| {
                    if h.is_cuti {
                        format!("Cuti Bersama: {}", h.name)
                    } else {
                        format!("Libur Nasional: {}", h.name)
                    }
                }),
                input,
            );
            if is_today {
                d2d.fill_circle(cr.center_x(), cr.center_y(), 10.0, Color::rgb(244, 244, 245));
                if holiday.is_some() {
                    d2d.stroke_circle(cr.center_x(), cr.center_y(), 11.5, 1.2, Color::rgb(248, 113, 113));
                } else if is_sel {
                    d2d.stroke_circle(cr.center_x(), cr.center_y(), 11.5, 1.0, Color::rgb(56, 189, 248));
                }
            } else if is_sel {
                if let Some(h) = holiday {
                    let (fill, stroke) = if h.is_cuti {
                        (Color::rgba(251, 191, 36, 0.28), Color::rgba(251, 191, 36, 0.72))
                    } else {
                        (Color::rgba(248, 113, 113, 0.30), Color::rgba(248, 113, 113, 0.78))
                    };
                    d2d.fill_circle(cr.center_x(), cr.center_y(), 10.0, fill);
                    d2d.stroke_circle(cr.center_x(), cr.center_y(), 10.8, 1.0, stroke);
                } else {
                    d2d.fill_circle(cr.center_x(), cr.center_y(), 10.0, Color::white(0.18));
                }
            } else if let Some(h) = holiday {
                let fill = if h.is_cuti {
                    Color::rgba(251, 191, 36, if dh.hovered { 0.24 } else { 0.14 })
                } else {
                    Color::rgba(248, 113, 113, if dh.hovered { 0.26 } else { 0.17 })
                };
                d2d.fill_circle(cr.center_x(), cr.center_y(), 10.0, fill);
            } else if dh.hovered {
                d2d.fill_circle(cr.center_x(), cr.center_y(), 10.0, Color::white(0.08));
            }

            let fg = if is_today {
                Color::rgb(9, 9, 11)
            } else if let Some(h) = holiday {
                if h.is_cuti {
                    Color::rgb(253, 224, 71)
                } else {
                    Color::rgb(252, 165, 165)
                }
            } else if is_sunday {
                Color::rgb(248, 113, 113)
            } else if is_sel {
                Color::white(0.98)
            } else {
                Color::white(0.80)
            };
            d2d.draw_text(
                &day.to_string(),
                cr,
                &TextStyle::default().size(10.0).medium().color(fg).center().vcenter(),
            );
            if has_evt && holiday.is_some() {
                d2d.fill_circle(cr.center_x() - 2.3, cr.bottom() - 2.5, 1.6, Color::rgb(56, 189, 248));
                d2d.fill_circle(cr.center_x() + 2.3, cr.bottom() - 2.5, 1.6, Color::rgb(248, 113, 113));
            } else if has_evt {
                d2d.fill_circle(
                    cr.center_x(),
                    cr.bottom() - 2.5,
                    1.8,
                    if is_today {
                        Color::rgb(14, 165, 233)
                    } else {
                        Color::rgb(56, 189, 248)
                    },
                );
            }
            if dh.clicked {
                state.selected = date;
            }
        }
        gy += ch;
    }

    // Right panel: selected date heading + optional national holiday pill + inline event creator + scrollable list
    let sel_iso = state.selected.format("%Y-%m-%d").to_string();
    let heading = format_tanggal_id(state.selected);
    let sel_is_sun = state.selected.weekday().num_days_from_monday() == 6;
    let sel_holiday = holiday_for(state.selected);
    d2d.draw_text(
        &heading,
        Rect::new(right.x + 14.0, right.y + 10.0, right.w - 28.0, 20.0),
        &TextStyle::default()
            .size(12.0)
            .semibold()
            .color(if sel_holiday.is_some() || sel_is_sun {
                Color::rgb(252, 165, 165)
            } else {
                Color::white(0.92)
            })
            .vcenter(),
    );

    let mut panel_y = right.y + 34.0;
    if let Some(h) = sel_holiday {
        let banner_r = Rect::new(right.x + 12.0, panel_y, right.w - 24.0, 20.0);
        let (bg, border, fg, prefix) = if h.is_cuti {
            (
                Color::rgba(251, 191, 36, 0.14),
                Color::rgba(251, 191, 36, 0.36),
                Color::rgb(253, 224, 71),
                "Cuti Bersama",
            )
        } else {
            (
                Color::rgba(248, 113, 113, 0.15),
                Color::rgba(248, 113, 113, 0.38),
                Color::rgb(252, 165, 165),
                "Libur Nasional",
            )
        };
        d2d.fill_rounded_rect(banner_r, 6.0, bg);
        d2d.stroke_rounded_rect(banner_r, 6.0, 1.0, border);
        d2d.draw_text(
            &format!("{} • {}", prefix, h.name),
            banner_r.inset(8.0, 0.0),
            &TextStyle::default()
                .size(10.0)
                .semibold()
                .color(fg)
                .vcenter()
                .ellipsis(),
        );
        panel_y += 24.0;
    }

    let add_r = Rect::new(right.x + 12.0, panel_y, right.w - 24.0, 26.0);
    let add_act = state.add_input.draw(
        d2d,
        hits,
        "cal_add_inp",
        add_r,
        "Tambah event - mis. Makan 20:00",
        false,
        None,
        input,
    );
    if add_act.submitted {
        if let Some((title, start, end, at_ms, end_at, note)) =
            parse_event_line(&state.add_input.text, state.selected)
        {
            let id = format!("cal{}", now_unix_ms());
            let mut next_cal = events.to_vec();
            next_cal.push(CalendarEvent {
                id: id.clone(),
                title: title.clone(),
                at: at_ms,
                date: sel_iso.clone(),
                time: start,
                end_time: end,
                end_at,
                note,
            });
            if let Ok(v) = serde_json::to_value(next_cal) {
                store.set(Some(ui), "calendarEvents", v);
            }

            let mut next_rem = reminders.to_vec();
            next_rem.push(ReminderItem {
                id,
                at: at_ms,
                message: title,
            });
            if let Ok(v) = serde_json::to_value(next_rem) {
                store.set(Some(ui), "reminders", v);
            }
            state.add_input.clear();
        }
    }

    let day_events: Vec<&CalendarEvent> = {
        let mut v: Vec<&CalendarEvent> = events.iter().filter(|e| e.date == sel_iso).collect();
        v.sort_by_key(|e| e.at);
        v
    };

    let list_top = add_r.bottom() + 6.0;
    let list_r = Rect::new(
        right.x + 10.0,
        list_top,
        right.w - 20.0,
        (right.bottom() - 8.0 - list_top).max(40.0),
    );
    if day_events.is_empty() {
        d2d.draw_text(
            "Tidak ada acara di tanggal ini",
            list_r,
            &TextStyle::default()
                .size(11.0)
                .color(Color::white(0.35))
                .center()
                .vcenter(),
        );
        return;
    }

    let content_h = day_events.len() as f32 * 30.0;
    state.agenda_scroll.update_bounds(list_r.h, content_h, input);
    state.agenda_scroll.handle_wheel(list_r, false, input);

    d2d.push_clip_rect(list_r);
    let mut ey = list_r.y - state.agenda_scroll.offset;
    for (idx, ev) in day_events.iter().enumerate() {
        let row = Rect::new(list_r.x, ey, list_r.w - 6.0, 26.0);
        if row.bottom() >= list_r.y && row.y <= list_r.bottom() {
            let rh = hits.register(format!("cal_evt_{}", idx), row, CursorKind::Arrow, None, input);
            if rh.hovered {
                d2d.fill_rounded_rect(row, 8.0, Color::white(0.06));
            }
            let time_label = if !ev.end_time.is_empty() {
                format!("{}-{}", ev.time, ev.end_time)
            } else {
                ev.time.clone()
            };
            let pill_w = if ev.end_time.is_empty() { 44.0 } else { 72.0 };
            let pill = Rect::new(row.x + 4.0, row.center_y() - 9.0, pill_w, 18.0);
            d2d.fill_rounded_rect(pill, 6.0, Color::rgba(251, 146, 60, 0.15));
            d2d.draw_text(
                &time_label,
                pill,
                &TextStyle::default()
                    .size(9.5)
                    .semibold()
                    .color(Color::rgb(251, 191, 36))
                    .center()
                    .vcenter(),
            );

            let meeting = system::detect_meeting_link(&ev.note)
                .or_else(|| system::detect_meeting_link(&ev.title));
            let join_w = if meeting.is_some() { 48.0 } else { 0.0 };
            d2d.draw_text(
                &ev.title,
                Rect::new(
                    pill.right() + 8.0,
                    row.y,
                    (row.right() - pill.right() - 32.0 - join_w).max(40.0),
                    row.h,
                ),
                &TextStyle::default()
                    .size(11.5)
                    .color(Color::white(0.90))
                    .vcenter()
                    .ellipsis(),
            );

            if let Some(lk) = meeting {
                let jr = Rect::new(row.right() - 68.0, row.center_y() - 9.0, 42.0, 18.0);
                let jh = hits.register(
                    format!("cal_join_{}", idx),
                    jr,
                    CursorKind::Pointer,
                    Some(format!("Gabung via {}", lk.provider.label())),
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
                    let lc = lk.clone();
                    rt.spawn(async move {
                        let _ = system::join_meeting_smart(lc).await;
                    });
                }
            }

            let del_r = Rect::new(row.right() - 22.0, row.center_y() - 9.0, 18.0, 18.0);
            let row_hovered = input.mouse_inside
                && list_r.contains(input.mouse_x, input.mouse_y)
                && row.contains(input.mouse_x, input.mouse_y);
            if row_hovered
                && draw_icon_btn(
                    d2d,
                    hits,
                    &format!("cal_del_{}", idx),
                    del_r,
                    IconId::Close,
                    10.5,
                    Color::white(0.55),
                    Color::rgba(239, 68, 68, 0.25),
                    Some("Hapus acara"),
                    input,
                )
            {
                let next_cal: Vec<CalendarEvent> =
                    events.iter().filter(|e| e.id != ev.id).cloned().collect();
                let next_rem: Vec<ReminderItem> =
                    reminders.iter().filter(|r| r.id != ev.id).cloned().collect();
                if let Ok(v) = serde_json::to_value(next_cal) {
                    store.set(Some(ui), "calendarEvents", v);
                }
                if let Ok(v) = serde_json::to_value(next_rem) {
                    store.set(Some(ui), "reminders", v);
                }
            }
        }
        ey += 30.0;
    }
    d2d.pop_clip_rect();
    state.agenda_scroll.draw_v_bar(d2d, list_r, input);
}
