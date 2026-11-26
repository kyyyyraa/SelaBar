use crate::backend::clipboard::PastedAttachment;
use crate::backend::BackendServices;
use crate::events::{PttMode, UiEvent};
use crate::logic::executor;
use crate::logic::harness::{self, ParsedInput};
use crate::logic::router::{convo_remember, convo_remember_with_cmd, RouterSurface};
use crate::state::{ConvoEntry, Settings};
use crate::ui::companion::{CompanionMood, CompanionRenderer, CompanionState};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{
    append_unique_attachments, draw_attachment_chips, CursorKind, HitRegistry, ScrollState,
    TextInputState, TextInputStyle, UiInput,
};

#[derive(Default)]
pub struct ConvoTabState {
    pub input: TextInputState,
    pub attachments: Vec<PastedAttachment>,
    pub scroll: ScrollState,
    pub last_len: usize,
    pub last_thinking: bool,
    pub last_live_cmd: String,
    pub scroll_pending_bottom: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_conversation(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    settings: &Settings,
    custom_photo: &str,
    entries: &[ConvoEntry],
    thinking: bool,
    live_cmd: &str,
    live_running: bool,
    live_ok: Option<bool>,
    voice_live: bool,
    state: &mut ConvoTabState,
    comp_renderer: &mut CompanionRenderer,
    comp_state: &mut CompanionState,
    svc: &BackendServices,
    input: &UiInput,
) {
    state.input.multiline = true;
    state.input.rich_paste = true;

    let inp_txt_st = TextStyle::default()
        .size(11.5)
        .color(Color::white(0.92))
        .line_height(16.0);
    let full_bar_w = (notch_rect.w - 32.0).max(120.0);
    let inp_avail_w = (full_bar_w - 34.0).max(60.0);
    let inp_h = state.input.measure_composer_height(
        d2d,
        inp_avail_w,
        14.0,
        62.0,
        &inp_txt_st,
        30.0,
        76.0,
    );
    let att_h = if !state.attachments.is_empty() { 34.0 } else { 0.0 };

    let bar = Rect::new(
        notch_rect.x + 16.0,
        notch_rect.bottom() - 12.0 - inp_h,
        full_bar_w,
        inp_h,
    );

    // Unboxed message stream sitting directly on #0b0b0d
    let area_top = notch_rect.y + 42.0;
    let area = Rect::new(
        notch_rect.x + 16.0,
        area_top,
        notch_rect.w - 32.0,
        (bar.y - att_h - 8.0 - area_top).max(60.0),
    );

    let stable_area_w = (560.0_f32 - 32.0).max(200.0);
    let max_bubble_w = (area.w * 0.78).max(120.0);
    let wrap_bubble_w = (stable_area_w * 0.78).max(max_bubble_w);
    let b_style = TextStyle::default().size(11.5).line_height(15.5);
    let mut bubbles: Vec<(bool, Rect, &str, &str, Option<bool>)> =
        Vec::with_capacity(entries.len());
    let mut y_acc = 4.0f32;
    for e in entries {
        let is_user = e.role == "user";
        if !is_user && !e.cmd.is_empty() {
            y_acc += 26.0;
        }
        let (tw, th) = d2d.measure_text(&e.text, wrap_bubble_w - 26.0, &b_style);
        let bw = (tw + 28.0).clamp(48.0, max_bubble_w);
        let bh = (th + 14.0).max(28.0);
        let bx = if is_user { area.w - bw - 8.0 } else { 0.0 };
        bubbles.push((
            is_user,
            Rect::new(area.x + bx, y_acc, bw, bh),
            &e.text,
            &e.cmd,
            e.cmd_ok,
        ));
        y_acc += bh + 8.0;
    }
    if thinking {
        y_acc += 34.0;
    }

    if entries.len() != state.last_len
        || thinking != state.last_thinking
        || live_cmd != state.last_live_cmd
    {
        state.last_len = entries.len();
        state.last_thinking = thinking;
        state.last_live_cmd = live_cmd.to_string();
        state.scroll_pending_bottom = true;
    }

    state.scroll.update_bounds(area.h, y_acc, input);
    if state.scroll_pending_bottom {
        state.scroll.scroll_to_end();
        state.scroll_pending_bottom = false;
    }
    state.scroll.handle_wheel(area, false, input);

    if entries.is_empty() && !thinking {
        d2d.draw_text(
            "Mulai percakapan atau ketik /help",
            area,
            &TextStyle::default()
                .size(12.0)
                .color(Color::white(0.32))
                .center()
                .vcenter(),
        );
    } else {
        d2d.push_clip_rect(area);
        for (is_user, rel_r, txt, cmd, cmd_ok) in bubbles {
            let br = Rect::new(rel_r.x, area.y + rel_r.y - state.scroll.offset, rel_r.w, rel_r.h);
            if !is_user && !cmd.is_empty() {
                let pr = Rect::new(
                    area.x,
                    br.y - 26.0,
                    max_bubble_w.min(area.w - 16.0),
                    22.0,
                );
                if pr.bottom() >= area.y && pr.y <= area.bottom() {
                    d2d.fill_rounded_rect(pr, 8.0, Color::rgb(20, 20, 24));
                    d2d.stroke_rounded_rect(pr, 8.0, 1.0, Color::white(0.08));
                    d2d.draw_text(
                        &format!("$ {}", cmd),
                        Rect::new(pr.x + 8.0, pr.y, pr.w - 72.0, pr.h),
                        &TextStyle::default()
                            .size(10.0)
                            .mono()
                            .color(Color::white(0.84))
                            .vcenter()
                            .ellipsis(),
                    );
                    let ok_flag = cmd_ok.unwrap_or(true);
                    d2d.draw_text(
                        if ok_flag { "✓ Done" } else { "× Gagal" },
                        Rect::new(pr.right() - 62.0, pr.y, 54.0, pr.h),
                        &TextStyle::default()
                            .size(10.0)
                            .semibold()
                            .color(if ok_flag {
                                Color::rgb(52, 211, 153)
                            } else {
                                Color::rgb(248, 113, 113)
                            })
                            .right()
                            .vcenter(),
                    );
                }
            }
            if br.bottom() < area.y || br.y > area.bottom() {
                continue;
            }
            if is_user {
                d2d.fill_rounded_rect(br, 12.0, Color::white(0.11));
                d2d.stroke_rounded_rect(br, 12.0, 1.0, Color::white(0.08));
            } else {
                d2d.fill_rounded_rect(br, 12.0, Color::white(0.045));
                d2d.stroke_rounded_rect(br, 12.0, 1.0, Color::white(0.09));
            }
            d2d.draw_text(
                txt,
                br.inset(11.0, 7.0),
                &b_style.clone().color(Color::white(0.92)),
            );
        }

        if thinking {
            let ty = area.y + y_acc - 30.0 - state.scroll.offset;
            if !live_cmd.is_empty() {
                let pr = Rect::new(
                    area.x,
                    ty,
                    max_bubble_w.min(area.w - 16.0),
                    24.0,
                );
                d2d.fill_rounded_rect(pr, 8.0, Color::rgb(20, 20, 24));
                d2d.stroke_rounded_rect(pr, 8.0, 1.0, Color::white(0.09));
                let cmd_right_pad = if live_running { 36.0 } else { 86.0 };
                d2d.draw_text(
                    &format!("$ {}", live_cmd),
                    Rect::new(pr.x + 8.0, pr.y, pr.w - cmd_right_pad, pr.h),
                    &TextStyle::default()
                        .size(10.0)
                        .mono()
                        .color(Color::white(0.86))
                        .vcenter()
                        .ellipsis(),
                );
                if live_running {
                    d2d.draw_spinning_box(
                        pr.right() - 15.0,
                        pr.center_y(),
                        8.5,
                        input.now_ms,
                        Color::rgb(251, 191, 36),
                    );
                } else {
                    let (st_lbl, st_col) = if live_ok.unwrap_or(true) {
                        ("✓ Done", Color::rgb(52, 211, 153))
                    } else {
                        ("× Gagal", Color::rgb(248, 113, 113))
                    };
                    d2d.draw_text(
                        st_lbl,
                        Rect::new(pr.right() - 78.0, pr.y, 70.0, pr.h),
                        &TextStyle::default()
                            .size(10.0)
                            .semibold()
                            .color(st_col)
                            .right()
                            .vcenter(),
                    );
                }
            } else {
                let tb = Rect::new(area.x, ty, 96.0, 24.0);
                d2d.fill_rounded_rect(tb, 8.0, Color::rgb(20, 20, 24));
                d2d.stroke_rounded_rect(tb, 8.0, 1.0, Color::white(0.09));
                d2d.draw_spinning_box(
                    tb.x + 14.0,
                    tb.center_y(),
                    8.5,
                    input.now_ms,
                    Color::rgb(251, 191, 36),
                );
                d2d.draw_text(
                    "Thinking",
                    Rect::new(tb.x + 26.0, tb.y, tb.w - 34.0, tb.h),
                    &TextStyle::default()
                        .size(10.5)
                        .color(Color::white(0.68))
                        .vcenter(),
                );
            }
        }
        d2d.pop_clip_rect();
        state.scroll.draw_v_bar(d2d, area, input);
    }

    // Bottom composer bar: 26x26 dark circle w/ 22px Companion + expandable capsule + Mic & Send buttons
    let comp_circle = Rect::new(bar.x, bar.bottom() - 29.0, 26.0, 26.0);
    d2d.fill_circle(
        comp_circle.center_x(),
        comp_circle.center_y(),
        13.0,
        Color::white(0.07),
    );
    comp_state.mood = if thinking {
        CompanionMood::Think
    } else if voice_live {
        CompanionMood::Speaking
    } else {
        CompanionMood::Idle
    };
    let comp_clicked = comp_renderer.draw(
        d2d,
        hits,
        "convo_mini_comp",
        comp_circle,
        22.0,
        &settings.avatar,
        settings.accent_hex(),
        custom_photo,
        comp_state,
        input,
    );
    CompanionRenderer::cycle_avatar_on_click(
        comp_clicked,
        settings,
        custom_photo,
        &svc.store,
        &svc.ui,
    );

    let inp_x = comp_circle.right() + 8.0;
    let inp_r = Rect::new(inp_x, bar.y, (bar.right() - inp_x).max(60.0), inp_h);
    if !state.attachments.is_empty() {
        let chip_strip = Rect::new(inp_r.x, inp_r.y - 30.0, inp_r.w, 22.0);
        draw_attachment_chips(
            d2d,
            hits,
            "convo_att",
            chip_strip,
            &mut state.attachments,
            input,
        );
    }

    let mut cap_style = TextInputStyle::capsule(inp_h);
    if inp_h > 38.0 {
        cap_style.radius = 13.0;
    }
    cap_style.pad_right = 62.0;
    let act = state.input.draw_styled(
        d2d,
        hits,
        "convo_inp",
        inp_r,
        "Pesan atau /help...",
        false,
        Some(inp_txt_st),
        &cap_style,
        input,
    );
    if !act.pasted_attachments.is_empty() {
        append_unique_attachments(&mut state.attachments, act.pasted_attachments, 6);
    }

    let mic_r = Rect::new(inp_r.right() - 52.0, inp_r.bottom() - 27.0, 22.0, 22.0);
    let mh = hits.register(
        "convo_mic_btn",
        mic_r,
        CursorKind::Pointer,
        Some(format!("Live Voice AI ({})", settings.hotkey_voice)),
        input,
    );
    d2d.fill_circle(
        mic_r.center_x(),
        mic_r.center_y(),
        11.0,
        Color::white(if mh.hovered { 0.16 } else { 0.08 }),
    );
    d2d.draw_icon(
        IconId::Mic,
        Rect::new(mic_r.center_x() - 5.5, mic_r.center_y() - 5.5, 11.0, 11.0),
        Color::white(if mh.hovered { 0.96 } else { 0.72 }),
        2.0,
        1.0,
    );
    if mh.clicked {
        executor::start_voice_session(svc.clone(), PttMode::Command, true);
        return;
    }

    let can_send = !state.input.text.trim().is_empty() || !state.attachments.is_empty();
    let send_r = Rect::new(inp_r.right() - 27.0, inp_r.bottom() - 27.0, 22.0, 22.0);
    let sh = hits.register(
        "convo_send_btn",
        send_r,
        CursorKind::Pointer,
        Some("Kirim pesan".to_string()),
        input,
    );
    d2d.fill_circle(
        send_r.center_x(),
        send_r.center_y(),
        11.0,
        if can_send {
            Color::white(if sh.hovered { 1.0 } else { 0.92 })
        } else {
            Color::white(if sh.hovered { 0.24 } else { 0.14 })
        },
    );
    d2d.draw_icon(
        IconId::ArrowUp,
        Rect::new(send_r.center_x() - 6.0, send_r.center_y() - 6.0, 12.0, 12.0),
        if can_send {
            Color::rgb(11, 11, 13)
        } else {
            Color::white(0.76)
        },
        2.1,
        1.0,
    );

    if (act.submitted || sh.clicked) && can_send {
        let raw_msg = state.input.text.trim().to_string();
        let atts = std::mem::take(&mut state.attachments);
        state.input.clear();
        state.scroll_pending_bottom = true;
        let svc_c = svc.clone();
        if !atts.is_empty() {
            let prompt = if raw_msg.is_empty() {
                "Analisis lampiran ini.".to_string()
            } else {
                raw_msg.clone()
            };
            let sum = executor::format_attachment_summary(&atts);
            let user_log = if raw_msg.is_empty() {
                sum
            } else {
                format!("{} {}", sum, raw_msg)
            };
            convo_remember(&svc.store, Some(&svc.ui), "user", &user_log);
            svc.ui.send(UiEvent::ConvoThinking(true));
            svc.rt.spawn(async move {
                let res = executor::handle_text_command_with_attachments(
                    &svc_c,
                    &prompt,
                    &atts,
                    true,
                    RouterSurface::Conversation,
                )
                .await;
                if !res.text.trim().is_empty() {
                    convo_remember_with_cmd(
                        &svc_c.store,
                        Some(&svc_c.ui),
                        "assistant",
                        &res.text,
                        &res.cmd,
                        res.cmd_ok,
                    );
                }
                svc_c.ui.send(UiEvent::ConvoThinking(false));
            });
            return;
        }
        match harness::parse(&raw_msg) {
            ParsedInput::Command { name, args } => {
                convo_remember(&svc.store, Some(&svc.ui), "user", &raw_msg);
                svc.rt.spawn(async move {
                    let out = harness::run_command(&svc_c, &name, &args).await;
                    if !out.trim().is_empty() {
                        convo_remember(&svc_c.store, Some(&svc_c.ui), "assistant", &out);
                    }
                });
            }
            ParsedInput::Chat { text } => {
                convo_remember(&svc.store, Some(&svc.ui), "user", &text);
                svc.ui.send(UiEvent::ConvoThinking(true));
                svc.rt.spawn(async move {
                    let res = executor::handle_text_command_detailed(
                        &svc_c,
                        &text,
                        true,
                        RouterSurface::Conversation,
                    )
                    .await;
                    if !res.text.trim().is_empty() {
                        convo_remember_with_cmd(
                            &svc_c.store,
                            Some(&svc_c.ui),
                            "assistant",
                            &res.text,
                            &res.cmd,
                            res.cmd_ok,
                        );
                    }
                    svc_c.ui.send(UiEvent::ConvoThinking(false));
                });
            }
        }
    }
}
