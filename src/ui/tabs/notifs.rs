use crate::backend::notif;
use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::events::UiSender;
use crate::state::{now_unix_ms, NotifEntry, Settings};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::popups::notif_primary_action_label;
use crate::ui::widgets::{
    draw_icon_btn, draw_pill_btn, CursorKind, HitRegistry, ScrollState, TextInputState,
    TextInputStyle, UiInput,
};
use chrono::{DateTime, Local};
use serde_json::{Map, Value};

#[derive(Default)]
pub struct NotifsTabState {
    pub scroll: ScrollState,
    pub replying_idx: Option<usize>,
    pub reply_input: TextInputState,
}

fn display_app_name<'a>(app: &'a str, site: &'a str) -> &'a str {
    let al = app.to_lowercase();
    let sl = site.to_lowercase();
    if sl.contains("whatsapp") || al.contains("whatsapp") {
        "WhatsApp"
    } else if al.contains("zapfast") {
        "ZapFast"
    } else if sl.contains("discord") || al.contains("discord") {
        "Discord"
    } else if sl.contains("telegram") || al.contains("telegram") {
        "Telegram"
    } else if sl.contains("gemini.google") || al.contains("gemini.google") {
        "Gemini"
    } else if al.contains("antigravity") {
        "Antigravity"
    } else if al.contains("zcode") {
        "Zcode"
    } else if al.contains("nvapp") || al.contains("nvidia") {
        "NVIDIA App"
    } else if al.contains("defender") || al.contains("securitycenter") {
        "Windows Security"
    } else if al.starts_with("zentoast") && !site.trim().is_empty() {
        site.trim()
            .strip_prefix("via ")
            .or_else(|| site.trim().strip_prefix("Via "))
            .unwrap_or(site.trim())
    } else {
        app
    }
}

fn monogram_color(name: &str) -> Color {
    let palette = [
        Color::rgb(56, 189, 248),
        Color::rgb(52, 211, 153),
        Color::rgb(251, 146, 60),
        Color::rgb(168, 85, 247),
        Color::rgb(244, 114, 182),
        Color::rgb(250, 204, 21),
    ];
    let h = name.bytes().fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
    palette[h % palette.len()]
}

fn fmt_time(ms: i64) -> String {
    let diff_s = ((now_unix_ms() - ms) / 1000).max(0);
    if diff_s < 60 {
        return "baru".to_string();
    }
    if diff_s < 3600 {
        return format!("{}m", diff_s / 60);
    }
    if let Some(dt) = DateTime::from_timestamp_millis(ms) {
        let local = dt.with_timezone(&Local);
        let today = Local::now().date_naive();
        if local.date_naive() == today {
            local.format("%H:%M").to_string()
        } else if (today - local.date_naive()).num_days() == 1 {
            "Kemarin".to_string()
        } else {
            local.format("%d/%m").to_string()
        }
    } else {
        String::new()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_notifs(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    notifs: &[NotifEntry],
    state: &mut NotifsTabState,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    let hdr = Rect::new(notch_rect.x + 14.0, notch_rect.y + 38.0, notch_rect.w - 28.0, 24.0);

    // DND button (#ef4444 active)
    if draw_pill_btn(
        d2d,
        hits,
        "notifs_dnd",
        Rect::new(hdr.x, hdr.y, 112.0, 22.0),
        if settings.dnd { "DND Aktif" } else { "Jangan Ganggu" },
        settings.dnd,
        Some(Color::rgb(239, 68, 68)),
        input,
    ) {
        let mut p = Map::new();
        p.insert("dnd".to_string(), Value::Bool(!settings.dnd));
        store.patch_settings(Some(ui), p);
    }

    if notifs.iter().any(|n| !n.read)
        && draw_pill_btn(
            d2d,
            hits,
            "notifs_read_all",
            Rect::new(hdr.right() - 108.0, hdr.y, 78.0, 22.0),
            "Baca semua",
            false,
            None,
            input,
        )
    {
        let next: Vec<NotifEntry> = notifs
            .iter()
            .cloned()
            .map(|mut n| {
                n.read = true;
                n
            })
            .collect();
        if let Ok(v) = serde_json::to_value(next) {
            store.set(Some(ui), "dnNotifList", v);
        }
    }

    if !notifs.is_empty()
        && draw_icon_btn(
            d2d,
            hits,
            "notifs_clear",
            Rect::new(hdr.right() - 24.0, hdr.y, 24.0, 22.0),
            IconId::Trash,
            13.0,
            Color::white(0.55),
            Color::rgba(239, 68, 68, 0.22),
            Some("Bersihkan semua notifikasi"),
            input,
        )
    {
        store.set(Some(ui), "dnNotifList", Value::Array(Vec::new()));
    }

    let inner = Rect::new(
        notch_rect.x + 16.0,
        hdr.bottom() + 8.0,
        notch_rect.w - 32.0,
        notch_rect.bottom() - hdr.bottom() - 20.0,
    );

    if notifs.is_empty() {
        d2d.draw_text(
            "Belum ada notifikasi",
            inner,
            &TextStyle::default()
                .size(12.0)
                .color(Color::white(0.36))
                .center()
                .vcenter(),
        );
        return;
    }

    let base_row_h = 46.0;
    let total_h: f32 = notifs
        .iter()
        .enumerate()
        .map(|(i, _)| if state.replying_idx == Some(i) { base_row_h + 34.0 } else { base_row_h })
        .sum();
    state.scroll.update_bounds(inner.h, total_h, input);
    state.scroll.handle_wheel(inner, false, input);

    d2d.push_clip_rect(inner);
    let mut y = inner.y - state.scroll.offset;
    for (idx, n) in notifs.iter().enumerate() {
        let is_replying = state.replying_idx == Some(idx) && !n.inputs.is_empty();
        let this_h = if is_replying { base_row_h + 34.0 } else { base_row_h };
        let row = Rect::new(inner.x, y, inner.w - 6.0, this_h - 4.0);
        if row.bottom() >= inner.y && row.y <= inner.bottom() {
            let row_top = Rect::new(row.x, row.y, row.w, base_row_h - 4.0);
            let row_hovered = input.mouse_inside
                && inner.contains(input.mouse_x, input.mouse_y)
                && row.contains(input.mouse_x, input.mouse_y);
            if row_hovered || is_replying {
                d2d.fill_rounded_rect(row, 10.0, Color::white(0.055));
            }
            if !n.read {
                d2d.fill_circle(row_top.x + 5.0, row_top.center_y(), 2.4, Color::rgb(56, 189, 248));
            }

            let app_disp = display_app_name(&n.app, &n.site);
            let av_r = Rect::new(row_top.x + 13.0, row_top.center_y() - 13.0, 26.0, 26.0);
            let logo_path = notif::normalize_logo_path(&n.logo);
            if !logo_path.is_empty() && std::path::Path::new(&logo_path).exists() {
                d2d.draw_image(&logo_path, av_r, 13.0, 1.0, true);
            } else {
                let col = monogram_color(app_disp);
                d2d.fill_circle(av_r.center_x(), av_r.center_y(), 13.0, col.with_alpha(0.20));
                let initial = app_disp
                    .chars()
                    .next()
                    .unwrap_or('N')
                    .to_uppercase()
                    .to_string();
                d2d.draw_text(
                    &initial,
                    av_r,
                    &TextStyle::default()
                        .size(11.0)
                        .bold()
                        .color(col)
                        .center()
                        .vcenter(),
                );
            }

            let right_reserve = 96.0;
            let tx = av_r.right() + 9.0;
            let tw = (row_top.right() - tx - right_reserve).max(72.0);
            let time_s = fmt_time(n.at);
            let title_line = if n.title.is_empty() { app_disp } else { &n.title };
            let title_show = if time_s.is_empty() {
                title_line.to_string()
            } else {
                format!("{} · {}", title_line, time_s)
            };
            d2d.draw_text(
                &title_show,
                Rect::new(tx, row_top.y + 4.0, tw, 16.0),
                &TextStyle::default()
                    .size(11.5)
                    .semibold()
                    .color(Color::white(if n.read { 0.65 } else { 0.94 }))
                    .ellipsis(),
            );
            let sub = if n.body.is_empty() { app_disp } else { &n.body };
            d2d.draw_text(
                sub,
                Rect::new(tx, row_top.y + 21.0, tw, 16.0),
                &TextStyle::default()
                    .size(10.5)
                    .color(Color::white(if n.read { 0.40 } else { 0.64 }))
                    .ellipsis(),
            );

            // Consistent right-aligned action rail (prevents hover flicker)
            let primary_lbl = notif_primary_action_label(
                app_disp,
                &n.site,
                &n.title,
                &n.body,
                !n.inputs.is_empty(),
            );
            let mut acted = false;
            if draw_pill_btn(
                d2d,
                hits,
                &format!("notif_open_{}", idx),
                Rect::new(row_top.right() - 90.0, row_top.center_y() - 10.0, 44.0, 20.0),
                primary_lbl,
                is_replying,
                None,
                input,
            ) {
                acted = true;
                if primary_lbl == "Balas" && !n.inputs.is_empty() {
                    if state.replying_idx == Some(idx) {
                        state.replying_idx = None;
                    } else {
                        state.replying_idx = Some(idx);
                        state.reply_input.clear();
                        hits.focused_input = Some(format!("input:notif_tab_reply_{}", idx));
                        // Remember the chat window before the notch takes focus.
                        crate::backend::win::capture_paste_target(ui.get_hwnd());
                    }
                } else {
                    open_notif_item(n, idx, notifs, store, ui, rt);
                }
            } else if draw_icon_btn(
                d2d,
                hits,
                &format!("notif_read_{}", idx),
                Rect::new(row_top.right() - 42.0, row_top.center_y() - 10.0, 19.0, 19.0),
                IconId::Check,
                11.0,
                Color::white(if n.read { 0.30 } else { 0.72 }),
                Color::white(0.10),
                Some("Tandai baca"),
                input,
            ) {
                acted = true;
                let mut next = notifs.to_vec();
                if let Some(item) = next.get_mut(idx) {
                    item.read = true;
                }
                if let Ok(v) = serde_json::to_value(next) {
                    store.set(Some(ui), "dnNotifList", v);
                }
            } else if draw_icon_btn(
                d2d,
                hits,
                &format!("notif_del_{}", idx),
                Rect::new(row_top.right() - 21.0, row_top.center_y() - 10.0, 19.0, 19.0),
                IconId::Close,
                10.5,
                Color::white(if row_hovered { 0.75 } else { 0.42 }),
                Color::rgba(239, 68, 68, 0.25),
                Some("Hapus"),
                input,
            ) {
                acted = true;
                if state.replying_idx == Some(idx) {
                    state.replying_idx = None;
                }
                let mut next = notifs.to_vec();
                next.remove(idx);
                if let Ok(v) = serde_json::to_value(next) {
                    store.set(Some(ui), "dnNotifList", v);
                }
            }

            let rh = hits.register(
                format!("notif_row_{}", idx),
                Rect::new(row_top.x, row_top.y, (row_top.w - 94.0).max(40.0), row_top.h),
                CursorKind::Pointer,
                None,
                input,
            );
            if !acted && rh.clicked {
                open_notif_item(n, idx, notifs, store, ui, rt);
            }

            if is_replying {
                let r_inp = Rect::new(tx, row_top.bottom() + 4.0, (row.w - tx + row.x - 68.0).max(90.0), 26.0);
                let cap = TextInputStyle::capsule(26.0);
                let ra = state.reply_input.draw_styled(
                    d2d,
                    hits,
                    &format!("notif_tab_reply_{}", idx),
                    r_inp,
                    "Ketik balasan...",
                    false,
                    Some(
                        TextStyle::default()
                            .size(11.0)
                            .color(Color::white(0.92))
                            .vcenter()
                            .nowrap(),
                    ),
                    &cap,
                    input,
                );
                let send_hit = draw_pill_btn(
                    d2d,
                    hits,
                    &format!("notif_tab_send_{}", idx),
                    Rect::new(r_inp.right() + 6.0, r_inp.y + 1.0, 56.0, 24.0),
                    "Kirim",
                    true,
                    Some(Color::rgb(52, 211, 153)),
                    input,
                );
                if ra.escaped {
                    state.replying_idx = None;
                    state.reply_input.clear();
                } else if (ra.submitted || send_hit) && !state.reply_input.text.trim().is_empty() {
                    let msg_txt = state.reply_input.text.trim().to_string();
                    let hint = if n.raw_app.trim().is_empty() {
                        n.app.clone()
                    } else {
                        n.raw_app.clone()
                    };
                    let act_args = n
                        .actions
                        .iter()
                        .find(|a| !a.input_id.is_empty())
                        .map(|a| a.args.clone())
                        .unwrap_or_else(|| n.launch.clone());
                    let inp_id = n.inputs.first().map(|i| i.id.clone()).unwrap_or_default();
                    let url = notif::target_url_of(&n.launch, &n.jid, &n.site);
                    let prof = if n.profile.trim().is_empty() {
                        notif::parse_profile(&n.launch)
                    } else {
                        n.profile.clone()
                    };
                    let sender = n.title.clone();
                    let self_hwnd = ui.get_hwnd();
                    let ui_tx = ui.clone();
                    rt.spawn(async move {
                        if system::notif_send_reply(&hint, &act_args, &inp_id, &msg_txt).await {
                            return;
                        }
                        let opened = system::notif_open_rich(&hint, &act_args, &url, &prof, &sender).await;
                        let routed = opened.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                        if !routed {
                            return;
                        }
                        let txt = msg_txt.clone();
                        let pasted = tokio::task::spawn_blocking(move || {
                            std::thread::sleep(std::time::Duration::from_millis(1500));
                            crate::backend::win::paste_text(&txt, self_hwnd).is_ok()
                        })
                        .await
                        .unwrap_or(false);
                        let msg = if pasted {
                            "Balasan ditempel di chat — periksa lalu kirim manual."
                        } else {
                            "Chat dibuka. Balasan tidak terkirim otomatis — ketik ulang di chat."
                        };
                        ui_tx.send(crate::events::UiEvent::ConvoAiDoneNotification {
                            title: "Balasan WhatsApp".to_string(),
                            body: msg.to_string(),
                        });
                    });
                    state.reply_input.clear();
                    state.replying_idx = None;
                }
            }
        }
        y += this_h;
    }
    d2d.pop_clip_rect();
    state.scroll.draw_v_bar(d2d, inner, input);
}

fn open_notif_item(
    n: &NotifEntry,
    idx: usize,
    notifs: &[NotifEntry],
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
) {
    let hint = if n.raw_app.trim().is_empty() {
        n.app.clone()
    } else {
        n.raw_app.clone()
    };
    let url = notif::target_url_of(&n.launch, &n.jid, &n.site);
    let prof = if n.profile.trim().is_empty() {
        notif::parse_profile(&n.launch)
    } else {
        n.profile.clone()
    };
    let launch = n.launch.clone();
    let sender = n.title.clone();
    rt.spawn(async move {
        system::notif_open_rich(&hint, &launch, &url, &prof, &sender).await;
    });
    let mut next = notifs.to_vec();
    if let Some(item) = next.get_mut(idx) {
        item.read = true;
    }
    if let Ok(v) = serde_json::to_value(next) {
        store.set(Some(ui), "dnNotifList", v);
    }
}
