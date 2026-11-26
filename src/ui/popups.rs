use crate::backend::{approval_gate, clipboard, ocr, screenshot, system, BackendServices};
use crate::events::{
    ApprovalRequest, MeetingLink, NotifPayload, PrivacySnapshot, PttMode, ScheduleProposal,
    StepProgress, UiEvent, VoiceStepStatus,
};
use crate::logic::ask::{self, ASK_CHIPS};
use crate::logic::executor;
use crate::state::{now_unix_ms, ReminderItem, Settings};
use crate::ui::companion::{CompanionMood, CompanionRenderer, CompanionState};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{
    append_unique_attachments, draw_attachment_chips, draw_icon_btn, draw_pill_btn, CursorKind,
    HitRegistry, ScrollState, TextInputState, TextInputStyle, UiInput,
};
use regex::Regex;
use std::f32::consts::PI;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum PopupState {
    None,
    Ask {
        run_id: u64,
        query: String,
        loading: bool,
        ok: bool,
        answer: String,
        proposal: Option<ScheduleProposal>,
        proposal_saved: bool,
        harness_cmd: String,
        harness_running: bool,
        harness_ok: Option<bool>,
        inline_approval: Option<ApprovalRequest>,
    },
    Voice {
        status: String,
        text: String,
        transcript: String,
        rms: [f32; 5],
        steps: Vec<VoiceStepStatus>,
        live: bool,
        expires_at_ms: u64,
    },
    WhisperSetup {
        expires_at_ms: u64,
    },
    Approval {
        req: ApprovalRequest,
        expires_at_ms: u64,
    },
    Capture {
        path: String,
        renaming: bool,
        flash_start_ms: u64,
        status_msg: String,
        expires_at_ms: u64,
    },
    Reminder {
        message: String,
        at_ms: i64,
        start_ms: u64,
        expires_at_ms: u64,
    },
    AgentCalendarAlert {
        event_id: String,
        title: String,
        time_label: String,
        minutes_before: u32,
        note: String,
        meeting_link: Option<MeetingLink>,
        start_ms: u64,
        expires_at_ms: u64,
    },
    Notif {
        payload: NotifPayload,
        replying: bool,
        expires_at_ms: u64,
    },
    Meeting {
        title: String,
        subtitle: String,
        link: MeetingLink,
        expires_at_ms: u64,
    },
    Done {
        duration_sec: u32,
        expires_at_ms: u64,
    },
    Offline {
        expires_at_ms: u64,
    },
}

impl Default for PopupState {
    fn default() -> Self {
        Self::None
    }
}

impl PopupState {
    pub fn is_active(&self) -> bool {
        !matches!(self, PopupState::None)
    }

    pub fn is_expired(&self, now_ms: u64) -> bool {
        match self {
            PopupState::Notif {
                replying: true, ..
            } => false,
            PopupState::Voice { expires_at_ms, .. }
            | PopupState::WhisperSetup { expires_at_ms }
            | PopupState::Approval { expires_at_ms, .. }
            | PopupState::Capture { expires_at_ms, .. }
            | PopupState::Reminder { expires_at_ms, .. }
            | PopupState::AgentCalendarAlert { expires_at_ms, .. }
            | PopupState::Notif { expires_at_ms, .. }
            | PopupState::Meeting { expires_at_ms, .. }
            | PopupState::Done { expires_at_ms, .. }
            | PopupState::Offline { expires_at_ms, .. } => *expires_at_ms > 0 && now_ms >= *expires_at_ms,
            PopupState::None | PopupState::Ask { .. } => false,
        }
    }
}

#[derive(Default)]
pub struct PopupUiState {
    pub ask_input: TextInputState,
    pub ask_attachments: Vec<clipboard::PastedAttachment>,
    pub ask_scroll: ScrollState,
    pub voice_scroll: ScrollState,
    pub rename_input: TextInputState,
    pub notif_reply_input: TextInputState,
    pub next_run_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PopupAction {
    pub dismiss: bool,
    pub restart_focus: bool,
    pub focus_ask_on_open: bool,
    pub switch_to_ask: bool,
    pub open_chat: bool,
    pub open_voice_settings: bool,
    pub open_files: bool,
    pub open_calendar: bool,
    pub show_toast_clipboard: bool,
    pub ask_ai_image_path: Option<String>,
}

pub fn notif_primary_action_label(
    app: &str,
    site: &str,
    title: &str,
    body: &str,
    has_inputs: bool,
) -> &'static str {
    let call_txt = format!("{} {} {}", app, title, body).to_lowercase();
    if [
        "panggilan masuk",
        "incoming call",
        "is calling",
        "menelepon",
        "panggilan suara",
        "panggilan video",
        "voice call",
        "video call",
    ]
    .iter()
    .any(|k| call_txt.contains(k))
    {
        return "Buka";
    }
    if has_inputs {
        return "Balas";
    }
    let combined = format!("{} {} {}", app, site, title).to_lowercase();
    if [
        "whatsapp", "zapfast", "discord", "telegram", "instagram", "line", "slack", "messenger",
    ]
    .iter()
    .any(|k| combined.contains(k))
    {
        return "Balas";
    }
    let sys_txt = format!("{} {}", combined, body.to_lowercase());
    if [
        "windows update",
        "security",
        "defender",
        "antivirus",
        "microsoft store",
        "pembaruan",
        "update available",
        "keamanan",
        "restart required",
    ]
    .iter()
    .any(|k| sys_txt.contains(k))
    {
        return "Cek";
    }
    "Buka"
}

pub fn extract_otp_code(text: &str) -> Option<String> {
    static CTX_RE: OnceLock<Regex> = OnceLock::new();
    static DIG_RE: OnceLock<Regex> = OnceLock::new();
    let low = text.to_lowercase();
    let has_ctx = CTX_RE
        .get_or_init(|| {
            Regex::new(r"\b(otp|kode|code|verif|pin|token|passcode)\b").unwrap()
        })
        .is_match(&low);
    if !has_ctx {
        return None;
    }
    let dig = DIG_RE.get_or_init(|| Regex::new(r"\b(\d{4,8})\b").unwrap());
    dig.captures(text).map(|c| c[1].to_string())
}

pub fn draw_privacy_dots(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id_prefix: &str,
    mut right_x: f32,
    center_y: f32,
    privacy: &PrivacySnapshot,
    input: &UiInput,
) -> f32 {
    if privacy.mic {
        right_x -= 11.0;
        let cx = right_x + 4.5;
        let col = Color::rgb(249, 115, 22);
        d2d.fill_circle(cx, center_y, 6.2, col.with_alpha(0.24));
        d2d.fill_circle(cx, center_y, 3.5, col);
        let tip = if privacy.mic_apps.is_empty() {
            "Mikrofon sedang aktif".to_string()
        } else {
            format!("Mikrofon aktif: {}", privacy.mic_apps.join(", "))
        };
        let _ = hits.register(
            format!("{}_mic", id_prefix),
            Rect::new(cx - 6.0, center_y - 6.0, 12.0, 12.0),
            CursorKind::Arrow,
            Some(tip),
            input,
        );
    }
    if privacy.camera {
        right_x -= 11.0;
        let cx = right_x + 4.5;
        let col = Color::rgb(34, 197, 94);
        d2d.fill_circle(cx, center_y, 6.2, col.with_alpha(0.24));
        d2d.fill_circle(cx, center_y, 3.5, col);
        let tip = if privacy.camera_apps.is_empty() {
            "Kamera sedang aktif".to_string()
        } else {
            format!("Kamera aktif: {}", privacy.camera_apps.join(", "))
        };
        let _ = hits.register(
            format!("{}_cam", id_prefix),
            Rect::new(cx - 6.0, center_y - 6.0, 12.0, 12.0),
            CursorKind::Arrow,
            Some(tip),
            input,
        );
    }
    right_x
}

fn is_voice_listening(status: &str) -> bool {
    status.eq_ignore_ascii_case("listening") || status.eq_ignore_ascii_case("Mendengarkan")
}

fn is_voice_done(status: &str) -> bool {
    status.eq_ignore_ascii_case("done") || status.eq_ignore_ascii_case("Selesai")
}

fn is_voice_error(status: &str) -> bool {
    status.eq_ignore_ascii_case("error") || status.eq_ignore_ascii_case("Gagal")
}

fn measure_ask_chip_rows(d2d: &mut D2dEngine, max_w: f32) -> usize {
    let st = TextStyle::default().size(10.5).medium();
    let mut cx = 0.0_f32;
    let mut rows = 1usize;
    for chip in ASK_CHIPS.iter() {
        let (tw, _) = d2d.measure_text(chip, 300.0, &st);
        let cw = tw + 18.0;
        if cx > 0.0 && cx + cw > max_w {
            rows += 1;
            cx = cw + 6.0;
        } else {
            cx += cw + 6.0;
        }
    }
    rows
}

fn notif_effective_body<'a>(payload: &'a NotifPayload, buf: &'a mut String) -> &'a str {
    if payload.is_call && payload.body.trim().is_empty() {
        *buf = format!("Panggilan masuk • {}", payload.app);
        buf.as_str()
    } else {
        payload.body.as_str()
    }
}

pub fn notif_popup_height(body_measured_h: f32, replying: bool) -> f32 {
    let bh = if body_measured_h <= 0.5 {
        0.0
    } else {
        body_measured_h.clamp(15.5, 78.0)
    };
    let text_bottom = if bh > 0.0 { 23.0 + bh } else { 21.0 };
    let content_h = text_bottom.max(38.0);
    let btn_h = if replying { 28.0 } else { 24.0 };
    let max_h = if replying { 166.0 } else { 162.0 };
    (28.0 + content_h + 8.0 + btn_h).ceil().clamp(98.0, max_h)
}

pub fn popup_target_size(
    popup: &PopupState,
    state: &PopupUiState,
    d2d: &mut D2dEngine,
) -> (f32, f32) {
    match popup {
        PopupState::None => (240.0, 30.0),
        PopupState::Ask {
            query,
            answer,
            loading,
            proposal,
            harness_cmd,
            inline_approval,
            ..
        } => {
            let ask_w = 556.0;
            let content_w = ask_w - 56.0 - 18.0;
            let inp_st = TextStyle::default()
                .size(12.0)
                .color(Color::white(0.92))
                .line_height(16.5);
            let inp_h = if state.ask_input.multiline {
                state
                    .ask_input
                    .measure_composer_height(d2d, content_w, 14.0, 66.0, &inp_st, 34.0, 86.0)
            } else {
                34.0
            };
            let att_h = if !state.ask_attachments.is_empty() { 34.0 } else { 0.0 };
            let composer_extra = (inp_h - 34.0) + att_h;
            if inline_approval.is_some() {
                return (ask_w, (192.0 + composer_extra).min(380.0));
            }
            let harness_extra = if !harness_cmd.is_empty() { 30.0 } else { 0.0 };
            if answer.is_empty() && !*loading && query.is_empty() {
                let chip_rows = measure_ask_chip_rows(d2d, content_w);
                let base_h = if chip_rows <= 1 { 134.0 } else { 134.0 + (chip_rows as f32 - 1.0) * 30.0 };
                (ask_w, (base_h + composer_extra).min(380.0))
            } else if *loading && answer.is_empty() {
                (ask_w, (134.0 + harness_extra + composer_extra).min(380.0))
            } else {
                let st = TextStyle::default().size(12.0).line_height(17.0);
                let (_, th) = d2d.measure_text(answer, content_w - 8.0, &st);
                let extra = (if proposal.is_some() { 28.0 } else { 0.0 }) + harness_extra;
                let h = (118.0 + th.min(126.0) + extra).clamp(150.0, 296.0);
                (ask_w, (h + composer_extra).min(380.0))
            }
        }
        PopupState::Voice {
            status,
            text,
            live,
            ..
        } => {
            if is_voice_listening(status) {
                if *live {
                    (364.0, 68.0)
                } else {
                    (340.0, 68.0)
                }
            } else if is_voice_done(status) || is_voice_error(status) {
                let st = TextStyle::default().size(12.5).semibold().line_height(17.0);
                let (_, th) = d2d.measure_text(text, 312.0, &st);
                let h = (50.0 + th.min(68.0)).clamp(76.0, 128.0);
                (430.0, h)
            } else {
                (430.0, 76.0)
            }
        }
        PopupState::WhisperSetup { .. } => (420.0, 96.0),
        PopupState::Approval { req, .. } => {
            let det_st = TextStyle::default().size(11.0).line_height(15.0);
            let dh = if req.detail.trim().is_empty() {
                0.0
            } else {
                d2d.measure_text(&req.detail, 256.0, &det_st).1.min(46.0)
            };
            let h = (62.0 + dh).clamp(94.0, 142.0);
            (420.0, h)
        }
        PopupState::Capture { .. } => (530.0, 150.0),
        PopupState::Reminder { .. } => (402.0, 132.0),
        PopupState::AgentCalendarAlert { .. } => (428.0, 122.0),
        PopupState::Notif {
            payload, replying, ..
        } => {
            let w: f32 = if *replying { 418.0 } else { 412.0 };
            let tw = (w - 28.0 - 50.0).max(140.0);
            let mut body_buf = String::new();
            let body_s = notif_effective_body(payload, &mut body_buf);
            let body_h = if body_s.trim().is_empty() {
                0.0
            } else {
                d2d.measure_text(
                    body_s,
                    tw,
                    &TextStyle::default().size(11.0).line_height(15.5),
                )
                .1
            };
            (w, notif_popup_height(body_h, *replying))
        }
        PopupState::Meeting { .. } => (396.0, 62.0),
        PopupState::Done { .. } => (340.0, 124.0),
        PopupState::Offline { .. } => (376.0, 66.0),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_popup(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    popup: &mut PopupState,
    state: &mut PopupUiState,
    settings: &Settings,
    custom_photo: &str,
    accent: Color,
    privacy: &PrivacySnapshot,
    comp_renderer: &mut CompanionRenderer,
    comp_state: &mut CompanionState,
    svc: &BackendServices,
    input: &UiInput,
) -> PopupAction {
    let mut act = PopupAction::default();
    let esc_pressed = input
        .keys
        .iter()
        .any(|k| k.vk == crate::ui::widgets::VK_ESCAPE);
    let area = notch_rect.inset(14.0, 14.0);
    if (privacy.mic || privacy.camera)
        && !matches!(
            popup,
            PopupState::Meeting { .. } | PopupState::AgentCalendarAlert { .. }
        )
    {
        let _ = draw_privacy_dots(
            d2d,
            hits,
            "pop_priv",
            notch_rect.right() - 12.0,
            notch_rect.y + 12.0,
            privacy,
            input,
        );
    }

    match popup {
        PopupState::None => {}
        PopupState::Ask {
            run_id,
            query,
            loading,
            ok,
            answer,
            proposal,
            proposal_saved,
            harness_cmd,
            harness_running,
            harness_ok,
            inline_approval,
        } => {
            let comp_r = Rect::new(notch_rect.x + 16.0, notch_rect.y + 12.0, 32.0, 32.0);
            comp_state.mood = if *loading || *harness_running {
                CompanionMood::Think
            } else {
                CompanionMood::Idle
            };
            let ask_comp_clicked = comp_renderer.draw(
                d2d,
                hits,
                "ask_comp",
                comp_r,
                28.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                ask_comp_clicked,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            let lx = notch_rect.x + 56.0;
            let lw = (notch_rect.right() - lx - 18.0).max(180.0);
            let ly = notch_rect.y + 14.0;
            state.ask_input.multiline = true;
            state.ask_input.rich_paste = true;
            let ask_txt_st = TextStyle::default()
                .size(12.0)
                .color(Color::white(0.92))
                .line_height(16.5);
            let inp_h = state.ask_input.measure_composer_height(
                d2d,
                lw,
                14.0,
                66.0,
                &ask_txt_st,
                34.0,
                86.0,
            );
            let att_h = if !state.ask_attachments.is_empty() {
                34.0
            } else {
                0.0
            };
            let inp_r = Rect::new(lx, notch_rect.bottom() - 14.0 - inp_h, lw, inp_h);
            let comp_bar_y = inp_r.y - att_h;
            let mut fire_query: Option<String> = None;

            if let Some(req) = inline_approval.clone() {
                let card = Rect::new(lx, ly, lw, (comp_bar_y - 10.0 - ly).max(96.0));
                d2d.fill_rounded_rect(card, 12.0, Color::rgb(20, 20, 24));
                d2d.stroke_rounded_rect(card, 12.0, 1.0, Color::white(0.11));

                let hdr_icon = if req.tool == "calendar_add" {
                    IconId::Calendar
                } else {
                    IconId::Terminal
                };
                d2d.draw_icon(
                    hdr_icon,
                    Rect::new(card.x + 11.0, card.y + 10.0, 14.0, 14.0),
                    Color::rgb(251, 191, 36),
                    2.0,
                    1.0,
                );
                let title_txt = if req.title.is_empty() {
                    "Perlu izin Anda"
                } else {
                    req.title.as_str()
                };
                d2d.draw_text(
                    title_txt,
                    Rect::new(card.x + 30.0, card.y + 8.0, card.w - 42.0, 18.0),
                    &TextStyle::default()
                        .size(12.0)
                        .semibold()
                        .color(Color::white(0.95))
                        .vcenter()
                        .ellipsis(),
                );

                let cmd_box = Rect::new(card.x + 10.0, card.y + 30.0, card.w - 20.0, (card.h - 68.0).max(26.0));
                d2d.fill_rounded_rect(cmd_box, 8.0, Color::rgb(10, 10, 12));
                d2d.stroke_rounded_rect(cmd_box, 8.0, 1.0, Color::white(0.08));
                let shown_det = if req.tool == "calendar_add" {
                    req.detail.clone()
                } else {
                    format!("$ {}", req.detail)
                };
                d2d.draw_text(
                    &shown_det,
                    cmd_box.inset(8.0, 4.0),
                    &TextStyle::default()
                        .size(11.0)
                        .mono()
                        .color(Color::white(0.86))
                        .vcenter()
                        .ellipsis(),
                );

                let btn_y = card.bottom() - 30.0;
                let allow_lbl = if req.tool == "calendar_add" {
                    "Simpan  ↵"
                } else {
                    "Run  ↵"
                };
                if draw_pill_btn(
                    d2d,
                    hits,
                    "appr_ask_deny",
                    Rect::new(card.right() - 178.0, btn_y, 82.0, 23.0),
                    "Deny  esc",
                    false,
                    None,
                    input,
                ) {
                    if !svc.approval.respond(&req.id, "deny") {
                        svc.pipe.respond(&req.id, "deny");
                    }
                    *inline_approval = None;
                }
                let allow_clicked = draw_pill_btn(
                    d2d,
                    hits,
                    "appr_ask_allow",
                    Rect::new(card.right() - 88.0, btn_y, 78.0, 23.0),
                    allow_lbl,
                    true,
                    Some(Color::rgb(52, 211, 153)),
                    input,
                );
                let enter_hit = input
                    .keys
                    .iter()
                    .any(|k| k.vk == crate::ui::widgets::VK_RETURN && !k.ctrl && !k.alt);
                if allow_clicked || (enter_hit && state.ask_input.text.is_empty()) {
                    if !svc.approval.respond(&req.id, "allow") {
                        svc.pipe.respond(&req.id, "allow");
                    }
                    *inline_approval = None;
                }
            } else if !*loading && answer.is_empty() && query.is_empty() {
                d2d.draw_text(
                    "How can I help?",
                    Rect::new(lx, ly, lw, 22.0),
                    &TextStyle::default()
                        .size(13.5)
                        .semibold()
                        .color(Color::white(0.95))
                        .vcenter(),
                );

                let chip_txt_st = TextStyle::default().size(10.5).medium();
                let mut cx = lx;
                let mut cy = ly + 28.0;
                for (idx, chip) in ASK_CHIPS.iter().enumerate() {
                    let (tw, _) = d2d.measure_text(chip, 300.0, &chip_txt_st);
                    let cw = tw + 18.0;
                    if cx > lx && cx + cw > lx + lw {
                        cx = lx;
                        cy += 28.0;
                    }
                    let cr = Rect::new(cx, cy, cw, 24.0);
                    let ch = hits.register(
                        format!("ask_chip_{}", idx),
                        cr,
                        CursorKind::Pointer,
                        None,
                        input,
                    );
                    d2d.fill_rounded_rect(
                        cr,
                        12.0,
                        Color::white(if ch.hovered { 0.12 } else { 0.06 }),
                    );
                    d2d.stroke_rounded_rect(
                        cr,
                        12.0,
                        1.0,
                        Color::white(if ch.hovered { 0.20 } else { 0.10 }),
                    );
                    d2d.draw_text(
                        chip,
                        cr,
                        &chip_txt_st
                            .clone()
                            .color(Color::white(if ch.hovered { 0.94 } else { 0.78 }))
                            .center()
                            .vcenter(),
                    );
                    if ch.clicked {
                        fire_query = Some((*chip).to_string());
                    }
                    cx += cw + 6.0;
                }
            } else {
                let mut cur_y = ly;
                if !query.is_empty() {
                    d2d.draw_text(
                        &format!("“{}”", query),
                        Rect::new(lx, cur_y, lw, 16.0),
                        &TextStyle::default()
                            .size(11.0)
                            .color(Color::white(0.45))
                            .vcenter()
                            .ellipsis(),
                    );
                    cur_y += 20.0;
                }
                if !harness_cmd.is_empty() {
                    let pill = Rect::new(lx, cur_y, lw, 24.0);
                    d2d.fill_rounded_rect(pill, 8.0, Color::rgb(20, 20, 24));
                    d2d.stroke_rounded_rect(pill, 8.0, 1.0, Color::white(0.09));
                    let status_w = if *harness_running { 26.0 } else { 64.0 };
                    d2d.draw_text(
                        &format!("$ {}", harness_cmd),
                        Rect::new(pill.x + 9.0, pill.y, pill.w - status_w - 14.0, pill.h),
                        &TextStyle::default()
                            .size(10.5)
                            .mono()
                            .color(Color::white(0.84))
                            .vcenter()
                            .ellipsis(),
                    );
                    if *harness_running {
                        d2d.draw_spinning_box(
                            pill.right() - 15.0,
                            pill.center_y(),
                            8.5,
                            input.now_ms,
                            Color::rgb(251, 191, 36),
                        );
                    } else {
                        let (st_txt, st_col) = if *harness_ok == Some(false) {
                            ("× Failed", Color::rgb(248, 113, 113))
                        } else {
                            ("✓ Done", Color::rgb(52, 211, 153))
                        };
                        d2d.draw_text(
                            st_txt,
                            Rect::new(pill.right() - status_w - 6.0, pill.y, status_w, pill.h),
                            &TextStyle::default()
                                .size(10.5)
                                .semibold()
                                .color(st_col)
                                .right()
                                .vcenter(),
                        );
                    }
                    cur_y += 28.0;
                }

                let prop_h = if proposal.is_some() { 28.0 } else { 0.0 };
                let ans_h = (comp_bar_y - 8.0 - cur_y - prop_h).max(24.0);
                let ans_r = Rect::new(lx, cur_y, lw, ans_h);

                if *loading {
                    if !(*harness_running && !harness_cmd.is_empty()) {
                        d2d.draw_spinning_box(
                            ans_r.x + 8.0,
                            ans_r.y + 10.0,
                            9.0,
                            input.now_ms,
                            Color::rgb(251, 191, 36),
                        );
                        d2d.draw_text(
                            "Thinking",
                            Rect::new(ans_r.x + 20.0, ans_r.y, (ans_r.w - 20.0).max(40.0), 20.0),
                            &TextStyle::default()
                                .size(11.5)
                                .color(Color::white(0.62))
                                .vcenter(),
                        );
                    }
                } else {
                    let st = TextStyle::default()
                        .size(12.0)
                        .color(if *ok {
                            Color::white(0.90)
                        } else {
                            Color::rgb(255, 110, 90)
                        })
                        .line_height(17.0);
                    let (_, ch) = d2d.measure_text(answer, ans_r.w - 6.0, &st);
                    state.ask_scroll.update_bounds(ans_r.h, ch + 4.0, input);
                    state.ask_scroll.handle_wheel(ans_r, false, input);
                    d2d.push_clip_rect(ans_r);
                    d2d.draw_text(
                        answer,
                        Rect::new(
                            ans_r.x,
                            ans_r.y - state.ask_scroll.offset,
                            ans_r.w - 6.0,
                            ch + 4.0,
                        ),
                        &st,
                    );
                    d2d.pop_clip_rect();
                    state.ask_scroll.draw_v_bar(d2d, ans_r, input);
                }

                if let Some(p) = proposal.as_ref() {
                    let py = comp_bar_y - 6.0 - 22.0;
                    d2d.draw_text(
                        &format!("{} ({})", p.title, p.at_iso.replace('T', " ")),
                        Rect::new(lx, py, lw - 166.0, 22.0),
                        &TextStyle::default()
                            .size(10.5)
                            .color(Color::rgb(147, 197, 253))
                            .vcenter()
                            .ellipsis(),
                    );
                    if draw_pill_btn(
                        d2d,
                        hits,
                        "ask_add_cal",
                        Rect::new(lx + lw - 158.0, py, 158.0, 22.0),
                        if *proposal_saved {
                            "✓ Tersimpan di Calendar"
                        } else {
                            "+ Tambah ke Calendar"
                        },
                        *proposal_saved,
                        Some(Color::rgb(56, 189, 248)),
                        input,
                    ) && !*proposal_saved
                    {
                        ask::accept_proposal_to_calendar(&svc.store, &svc.ui, p);
                        *proposal_saved = true;
                    }
                }
            }

            if !state.ask_attachments.is_empty() {
                let chip_strip = Rect::new(lx, inp_r.y - 30.0, lw, 22.0);
                draw_attachment_chips(
                    d2d,
                    hits,
                    "ask_att",
                    chip_strip,
                    &mut state.ask_attachments,
                    input,
                );
            }

            let mut ask_box_style = TextInputStyle::capsule(inp_h);
            if inp_h > 40.0 {
                ask_box_style.radius = 14.0;
            }
            ask_box_style.pad_right = 66.0;
            let ia = state.ask_input.draw_styled(
                d2d,
                hits,
                "popup_ask_inp",
                inp_r,
                "Ask anything...",
                false,
                Some(ask_txt_st),
                &ask_box_style,
                input,
            );
            if !ia.pasted_attachments.is_empty() {
                append_unique_attachments(&mut state.ask_attachments, ia.pasted_attachments, 6);
            }
            if ia.escaped || esc_pressed {
                if let Some(req) = inline_approval.take() {
                    if !svc.approval.respond(&req.id, "deny") {
                        svc.pipe.respond(&req.id, "deny");
                    }
                    svc.ask_ctrl.set_pending_approval(None);
                } else {
                    svc.ask_ctrl.cancel_active(&svc.approval, &svc.pipe);
                    state.next_run_id += 1;
                    state.ask_attachments.clear();
                    hits.focused_input = None;
                    act.dismiss = true;
                }
                return act;
            }
            let can_submit =
                !state.ask_input.text.trim().is_empty() || !state.ask_attachments.is_empty();
            if ia.submitted && can_submit && inline_approval.is_none() {
                fire_query = Some(state.ask_input.text.trim().to_string());
            }

            // Mic voice button inside composer right edge
            let mic_r = Rect::new(inp_r.right() - 56.0, inp_r.bottom() - 29.0, 24.0, 24.0);
            let mh = hits.register(
                "ask_mic_btn",
                mic_r,
                CursorKind::Pointer,
                Some(format!("Live Voice AI ({})", settings.hotkey_voice)),
                input,
            );
            d2d.fill_circle(
                mic_r.center_x(),
                mic_r.center_y(),
                12.0,
                Color::white(if mh.hovered { 0.16 } else { 0.08 }),
            );
            d2d.draw_icon(
                IconId::Mic,
                Rect::new(mic_r.center_x() - 6.0, mic_r.center_y() - 6.0, 12.0, 12.0),
                Color::white(if mh.hovered { 0.95 } else { 0.72 }),
                2.0,
                1.0,
            );
            if mh.clicked {
                svc.ask_ctrl.cancel_active(&svc.approval, &svc.pipe);
                executor::start_voice_session(svc.clone(), PttMode::Command, true);
                return act;
            }

            // Embedded circular white send / stop button inside right edge of capsule
            let send_r = Rect::new(inp_r.right() - 29.0, inp_r.bottom() - 29.0, 24.0, 24.0);
            let sh = hits.register(
                "ask_send_btn",
                send_r,
                CursorKind::Pointer,
                Some((if *loading { "Hentikan" } else { "Kirim" }).to_string()),
                input,
            );
            let has_text = can_submit;
            let circle_col = if *loading || has_text {
                Color::white(if sh.hovered { 1.0 } else { 0.92 })
            } else {
                Color::white(if sh.hovered { 0.24 } else { 0.16 })
            };
            d2d.fill_circle(send_r.center_x(), send_r.center_y(), 12.0, circle_col);
            if *loading {
                d2d.fill_rounded_rect(
                    Rect::new(send_r.center_x() - 4.0, send_r.center_y() - 4.0, 8.0, 8.0),
                    1.5,
                    Color::rgb(11, 11, 13),
                );
            } else {
                d2d.draw_icon(
                    IconId::ArrowUp,
                    Rect::new(send_r.center_x() - 6.5, send_r.center_y() - 6.5, 13.0, 13.0),
                    if has_text {
                        Color::rgb(11, 11, 13)
                    } else {
                        Color::white(0.82)
                    },
                    2.2,
                    1.0,
                );
            }
            if sh.clicked {
                if *loading {
                    svc.ask_ctrl.cancel_active(&svc.approval, &svc.pipe);
                    state.next_run_id += 1;
                    *run_id = state.next_run_id;
                    *loading = false;
                    *ok = true;
                    *harness_running = false;
                    if !harness_cmd.is_empty() && harness_ok.is_none() {
                        *harness_ok = Some(false);
                    }
                    *inline_approval = None;
                    if answer.trim().is_empty() {
                        *answer = "Dihentikan.".to_string();
                    }
                    fire_query = None;
                } else if has_text {
                    fire_query = Some(state.ask_input.text.trim().to_string());
                }
            }

            if let Some(q) = fire_query {
                let atts = std::mem::take(&mut state.ask_attachments);
                let display_q = if atts.is_empty() {
                    q.clone()
                } else {
                    let sum = executor::format_attachment_summary(&atts);
                    if q.is_empty() {
                        sum
                    } else {
                        format!("{} {}", sum, q)
                    }
                };
                state.ask_input.clear();
                state.ask_scroll.offset = 0.0;
                state.next_run_id += 1;
                let rid = state.next_run_id;
                *run_id = rid;
                *query = display_q;
                *loading = true;
                *ok = true;
                answer.clear();
                *proposal = None;
                *proposal_saved = false;
                harness_cmd.clear();
                *harness_running = false;
                *harness_ok = None;
                *inline_approval = None;
                let cancel = svc.ask_ctrl.start_run(rid, &svc.approval, &svc.pipe);
                let svc_c = svc.clone();
                let jh = svc.rt.spawn(async move {
                    ask::run_ask_query_with_attachments(svc_c, rid, q, atts, Some(cancel)).await;
                });
                svc.ask_ctrl.attach_abort(rid, jh.abort_handle());
            }
        }

        PopupState::Voice {
            status,
            text,
            transcript,
            rms,
            steps,
            live,
            ..
        } => {
            let listening = is_voice_listening(status);
            let speaking =
                status.eq_ignore_ascii_case("speaking") || status.eq_ignore_ascii_case("Berbicara");
            let transcribing = status.eq_ignore_ascii_case("transcribing")
                || status.eq_ignore_ascii_case("Mentranskripsi");
            let done = is_voice_done(status);
            let err = is_voice_error(status);

            if listening {
                let comp_r = Rect::new(notch_rect.x + 18.0, notch_rect.center_y() - 16.0, 32.0, 32.0);
                comp_state.mood = CompanionMood::Hop;
                let v_clicked = comp_renderer.draw(
                    d2d,
                    hits,
                    "voice_comp",
                    comp_r,
                    30.0,
                    &settings.avatar,
                    settings.accent_hex(),
                    custom_photo,
                    comp_state,
                    input,
                );
                CompanionRenderer::cycle_avatar_on_click(
                    v_clicked,
                    settings,
                    custom_photo,
                    &svc.store,
                    &svc.ui,
                );

                let stop_r = Rect::new(notch_rect.right() - 42.0, notch_rect.center_y() - 14.0, 28.0, 28.0);
                let kb_r = Rect::new(stop_r.x - 34.0, notch_rect.center_y() - 14.0, 28.0, 28.0);
                // 5-bar live RMS audio waveform beside Listening label
                let bars_x = kb_r.x - 38.0;
                let t = input.now_ms as f32 / 1000.0;
                for i in 0..5 {
                    let idle_wave = 0.14 + 0.08 * ((t * 4.5 + i as f32 * 0.9).sin() * 0.5 + 0.5);
                    let lvl = rms[i].max(idle_wave).clamp(0.08, 1.0);
                    let bh = 4.0 + lvl * 18.0;
                    let bx = bars_x + i as f32 * 6.0;
                    let by = notch_rect.center_y() - bh * 0.5;
                    let col = if rms[i] > 0.18 {
                        Color::rgb(52, 211, 153)
                    } else {
                        Color::white(0.68)
                    };
                    d2d.fill_rounded_rect(Rect::new(bx, by, 3.0, bh), 1.5, col);
                }

                let lbl_x = if *live {
                    let dx = comp_r.right() + 11.0;
                    let dy = notch_rect.center_y();
                    let pulse = 0.45 + 0.45 * (t * 4.0).sin().abs();
                    d2d.fill_circle(dx, dy, 5.5, Color::rgb(52, 211, 153).with_alpha(pulse * 0.35));
                    d2d.fill_circle(dx, dy, 3.0, Color::rgb(52, 211, 153));
                    comp_r.right() + 20.0
                } else {
                    comp_r.right() + 12.0
                };
                let lbl_r = Rect::new(
                    lbl_x,
                    notch_rect.y + 4.0,
                    (bars_x - lbl_x - 8.0).max(70.0),
                    notch_rect.h - 10.0,
                );
                d2d.draw_text(
                    if *live { "Live listening..." } else { "Listening..." },
                    lbl_r,
                    &TextStyle::default()
                        .size(14.5)
                        .medium()
                        .color(Color::white(0.78))
                        .vcenter()
                        .ellipsis(),
                );

                let kh = hits.register(
                    "voice_to_ask",
                    kb_r,
                    CursorKind::Pointer,
                    Some("Ketik perintah (Ask AI)".to_string()),
                    input,
                );
                d2d.fill_circle(
                    kb_r.center_x(),
                    kb_r.center_y(),
                    14.0,
                    Color::white(if kh.hovered { 0.16 } else { 0.08 }),
                );
                d2d.draw_icon(
                    IconId::Keyboard,
                    Rect::new(kb_r.center_x() - 7.0, kb_r.center_y() - 7.0, 14.0, 14.0),
                    Color::white(if kh.hovered { 0.94 } else { 0.72 }),
                    2.0,
                    1.0,
                );
                if kh.clicked {
                    svc.voice.cancel_all_voice();
                    svc.ui.send(UiEvent::VoiceLiveActive(false));
                    act.switch_to_ask = true;
                    return act;
                }

                let sh = hits.register(
                    "voice_listen_stop",
                    stop_r,
                    CursorKind::Pointer,
                    Some("Hentikan Live Voice (Esc)".to_string()),
                    input,
                );
                d2d.fill_circle(
                    stop_r.center_x(),
                    stop_r.center_y(),
                    14.0,
                    if sh.hovered {
                        Color::rgba(239, 68, 68, 0.30)
                    } else {
                        Color::white(0.08)
                    },
                );
                d2d.fill_rounded_rect(
                    Rect::new(stop_r.center_x() - 3.8, stop_r.center_y() - 3.8, 7.6, 7.6),
                    1.5,
                    Color::white(if sh.hovered { 0.98 } else { 0.80 }),
                );
                if sh.clicked {
                    svc.voice.cancel_all_voice();
                    svc.ui.send(UiEvent::VoiceLiveActive(false));
                    act.dismiss = true;
                    return act;
                }
            } else {
                let comp_r = Rect::new(notch_rect.x + 18.0, notch_rect.center_y() - 15.0, 30.0, 30.0);
                comp_state.mood = if speaking {
                    CompanionMood::Speaking
                } else if done {
                    CompanionMood::Idle
                } else {
                    CompanionMood::Think
                };
                let v_clicked = comp_renderer.draw(
                    d2d,
                    hits,
                    "voice_comp",
                    comp_r,
                    28.0,
                    &settings.avatar,
                    settings.accent_hex(),
                    custom_photo,
                    comp_state,
                    input,
                );
                CompanionRenderer::cycle_avatar_on_click(
                    v_clicked,
                    settings,
                    custom_photo,
                    &svc.store,
                    &svc.ui,
                );

                let stop_r = Rect::new(notch_rect.right() - 46.0, notch_rect.center_y() - 15.0, 30.0, 30.0);
                let sh = hits.register(
                    "voice_stop_close",
                    stop_r,
                    CursorKind::Pointer,
                    Some((if done || err { "Tutup (Esc)" } else { "Hentikan (Esc)" }).to_string()),
                    input,
                );
                d2d.fill_circle(
                    stop_r.center_x(),
                    stop_r.center_y(),
                    15.0,
                    Color::white(if sh.hovered { 0.16 } else { 0.08 }),
                );
                if done || err {
                    d2d.draw_icon(
                        IconId::Close,
                        Rect::new(stop_r.center_x() - 6.0, stop_r.center_y() - 6.0, 12.0, 12.0),
                        Color::white(0.85),
                        2.0,
                        1.0,
                    );
                } else {
                    d2d.fill_rounded_rect(
                        Rect::new(stop_r.center_x() - 4.0, stop_r.center_y() - 4.0, 8.0, 8.0),
                        1.5,
                        Color::white(0.92),
                    );
                }
                if sh.clicked {
                    svc.voice.cancel_all_voice();
                    svc.ui.send(UiEvent::VoiceLiveActive(false));
                    act.dismiss = true;
                    return act;
                }

                let lx = comp_r.right() + 10.0;
                let lw = (stop_r.x - lx - 12.0).max(120.0);
                let top_quote = if !transcript.trim().is_empty() {
                    format!("“{}”", transcript.trim())
                } else if transcribing {
                    if *live {
                        "Live Voice".to_string()
                    } else {
                        "Voice AI".to_string()
                    }
                } else if !text.trim().is_empty() && !speaking && !done && !err {
                    format!("“{}”", text.trim())
                } else if *live {
                    "LIVE".to_string()
                } else {
                    String::new()
                };

                let active_step = steps
                    .iter()
                    .rfind(|s| matches!(s.state, StepProgress::Active))
                    .or_else(|| steps.iter().rfind(|s| matches!(s.state, StepProgress::Done)));
                let headline = if done || err {
                    if text.trim().is_empty() {
                        status.clone()
                    } else {
                        text.clone()
                    }
                } else if let Some(stp) = active_step {
                    stp.label.clone()
                } else if transcribing {
                    "Transcribing...".to_string()
                } else if speaking {
                    "Speaking...".to_string()
                } else {
                    "Thinking...".to_string()
                };

                if (done || err) && !headline.is_empty() {
                    let body_y = if !top_quote.is_empty() {
                        d2d.draw_text(
                            &top_quote,
                            Rect::new(lx, notch_rect.y + 12.0, lw, 16.0),
                            &TextStyle::default()
                                .size(11.0)
                                .color(Color::white(0.45))
                                .vcenter()
                                .ellipsis(),
                        );
                        notch_rect.y + 30.0
                    } else {
                        notch_rect.y + 14.0
                    };
                    let body_r = Rect::new(
                        lx,
                        body_y,
                        lw,
                        (notch_rect.bottom() - 12.0 - body_y).max(20.0),
                    );
                    let st = TextStyle::default()
                        .size(12.5)
                        .semibold()
                        .color(if err {
                            Color::rgb(255, 110, 90)
                        } else {
                            Color::white(0.94)
                        })
                        .line_height(17.0);
                    let (_, ch) = d2d.measure_text(&headline, body_r.w - 4.0, &st);
                    state.voice_scroll.update_bounds(body_r.h, ch + 2.0, input);
                    state.voice_scroll.handle_wheel(body_r, false, input);
                    d2d.push_clip_rect(body_r);
                    d2d.draw_text(
                        &headline,
                        Rect::new(
                            body_r.x,
                            body_r.y - state.voice_scroll.offset,
                            body_r.w - 4.0,
                            ch + 2.0,
                        ),
                        &st,
                    );
                    d2d.pop_clip_rect();
                    state.voice_scroll.draw_v_bar(d2d, body_r, input);
                } else if !top_quote.is_empty() {
                    let stack_y = (notch_rect.center_y() - 20.0).max(notch_rect.y + 12.0);
                    d2d.draw_text(
                        &top_quote,
                        Rect::new(lx, stack_y, lw, 16.0),
                        &TextStyle::default()
                            .size(11.0)
                            .color(Color::white(0.44))
                            .vcenter()
                            .ellipsis(),
                    );
                    d2d.draw_text(
                        &headline,
                        Rect::new(lx, stack_y + 18.0, lw, 22.0),
                        &TextStyle::default()
                            .size(13.0)
                            .semibold()
                            .color(if err {
                                Color::rgb(255, 110, 90)
                            } else {
                                Color::white(0.95)
                            })
                            .vcenter()
                            .ellipsis(),
                    );
                } else {
                    let show_loader = !done && !err && (transcribing || !speaking);
                    let tx = if show_loader {
                        d2d.draw_spinning_box(
                            lx + 8.0,
                            notch_rect.center_y(),
                            11.5,
                            input.now_ms,
                            accent,
                        );
                        lx + 24.0
                    } else {
                        lx
                    };
                    d2d.draw_text(
                        &headline,
                        Rect::new(tx, notch_rect.y + 8.0, (lx + lw - tx).max(60.0), notch_rect.h - 16.0),
                        &TextStyle::default()
                            .size(13.5)
                            .semibold()
                            .color(if err {
                                Color::rgb(255, 110, 90)
                            } else {
                                Color::white(0.95)
                            })
                            .vcenter()
                            .ellipsis(),
                    );
                }
            }
        }

        PopupState::WhisperSetup { .. } => {
            let pad_x = 14.0;
            let top_y = notch_rect.y + if privacy.mic || privacy.camera { 20.0 } else { 14.0 };
            let comp_r = Rect::new(notch_rect.x + pad_x, top_y, 44.0, 44.0);
            comp_state.mood = CompanionMood::Idle;
            let ws_comp_clicked = comp_renderer.draw(
                d2d,
                hits,
                "ws_comp",
                comp_r,
                44.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                ws_comp_clicked,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            let btn_w = 84.0;
            let btn_h = 26.0;
            let btn_x = notch_rect.right() - pad_x - btn_w;

            // Download button (top right: solid white pill -> Settings -> Voice dual CPU/CUDA picker)
            let dl_r = Rect::new(btn_x, top_y + 2.0, btn_w, btn_h);
            let dh = hits.register(
                "ws_download",
                dl_r,
                CursorKind::Pointer,
                Some("Buka pengaturan unduhan model suara (CPU / CUDA)".to_string()),
                input,
            );
            d2d.fill_rounded_rect(
                dl_r,
                13.0,
                Color::white(if dh.hovered { 0.88 } else { 1.0 }),
            );
            d2d.draw_text(
                "Download",
                dl_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::rgb(0, 0, 0))
                    .center()
                    .vcenter()
                    .nowrap(),
            );
            if dh.clicked {
                act.open_voice_settings = true;
            }

            // Chat button (bottom right: outlined dark pill -> Chat AI tab)
            let chat_r = Rect::new(btn_x, dl_r.bottom() + 6.0, btn_w, btn_h);
            let ch = hits.register(
                "ws_chat",
                chat_r,
                CursorKind::Pointer,
                Some("Buka tab Chat AI".to_string()),
                input,
            );
            if ch.hovered {
                d2d.fill_rounded_rect(chat_r, 13.0, Color::white(0.08));
            }
            d2d.stroke_rounded_rect(
                chat_r,
                13.0,
                1.0,
                Color::white(if ch.hovered { 0.36 } else { 0.20 }),
            );
            d2d.draw_text(
                "Chat",
                chat_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::white(0.90))
                    .center()
                    .vcenter()
                    .nowrap(),
            );
            if ch.clicked {
                act.open_chat = true;
            } else if esc_pressed {
                act.dismiss = true;
            }

            // Middle info column
            let mid_x = comp_r.right() + 10.0;
            let mid_w = (btn_x - mid_x - 10.0).max(120.0);
            d2d.draw_text(
                "MODEL SUARA WHISPER",
                Rect::new(mid_x, top_y, mid_w, 13.0),
                &TextStyle::default()
                    .size(9.0)
                    .bold()
                    .color(Color::rgba(125, 211, 252, 0.88))
                    .vcenter()
                    .nowrap(),
            );
            d2d.draw_text(
                "Model Suara Belum Terpasang",
                Rect::new(mid_x, top_y + 15.0, mid_w, 18.0),
                &TextStyle::default()
                    .size(13.0)
                    .semibold()
                    .color(Color::white(1.0))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                "Pilih Download untuk pilih CPU/CUDA, atau Chat.",
                Rect::new(mid_x, top_y + 35.0, mid_w, 32.0),
                &TextStyle::default()
                    .size(11.0)
                    .line_height(15.0)
                    .color(Color::white(0.58)),
            );
        }

        PopupState::Approval { req, .. } => {
            let pad_x = 14.0;
            let top_y = notch_rect.y + if privacy.mic || privacy.camera { 20.0 } else { 14.0 };
            let comp_r = Rect::new(notch_rect.x + pad_x, top_y, 44.0, 44.0);
            comp_state.mood = CompanionMood::Think;
            let appr_comp_clicked = comp_renderer.draw(
                d2d,
                hits,
                "appr_comp",
                comp_r,
                44.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                appr_comp_clicked,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            let btn_w = 70.0;
            let btn_h = 26.0;
            let btn_x = notch_rect.right() - pad_x - btn_w;

            // Deny pill button (top right: transparent w/ subtle white border)
            let deny_r = Rect::new(btn_x, top_y + 2.0, btn_w, btn_h);
            let dh = hits.register("appr_deny", deny_r, CursorKind::Pointer, None, input);
            if dh.hovered {
                d2d.fill_rounded_rect(deny_r, 13.0, Color::white(0.08));
            }
            d2d.stroke_rounded_rect(
                deny_r,
                13.0,
                1.0,
                Color::white(if dh.hovered { 0.36 } else { 0.20 }),
            );
            d2d.draw_text(
                "Tolak",
                deny_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::white(0.88))
                    .center()
                    .vcenter()
                    .nowrap(),
            );
            if dh.clicked {
                if !svc.approval.respond(&req.id, "deny") {
                    svc.pipe.respond(&req.id, "deny");
                }
                act.dismiss = true;
            }

            // Allow pill button (bottom right: solid white bg, dark text)
            let allow_r = Rect::new(btn_x, deny_r.bottom() + 6.0, btn_w, btn_h);
            let ah = hits.register("appr_allow", allow_r, CursorKind::Pointer, None, input);
            d2d.fill_rounded_rect(
                allow_r,
                13.0,
                Color::white(if ah.hovered { 0.88 } else { 1.0 }),
            );
            d2d.draw_text(
                "Izinkan",
                allow_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::rgb(0, 0, 0))
                    .center()
                    .vcenter()
                    .nowrap(),
            );
            let enter_hit = input
                .keys
                .iter()
                .any(|k| k.vk == crate::ui::widgets::VK_RETURN && !k.ctrl && !k.alt);
            if ah.clicked || enter_hit {
                if !svc.approval.respond(&req.id, "allow") {
                    svc.pipe.respond(&req.id, "allow");
                }
                act.dismiss = true;
            } else if esc_pressed {
                if !svc.approval.respond(&req.id, "deny") {
                    svc.pipe.respond(&req.id, "dismiss");
                }
                act.dismiss = true;
            }

            // Middle column: eyebrow "PERMINTAAN IZIN" + provider badge, bold title, 2-3 line detail
            let mid_x = comp_r.right() + 10.0;
            let mid_w = (btn_x - mid_x - 10.0).max(120.0);
            let is_internal = svc.approval.has_pending(&req.id)
                || req.id.starts_with("dn-local-")
                || req.id.starts_with("sb-local-");
            let (badge_lbl, badge_col) = approval_provider_badge(req, is_internal);
            d2d.draw_text(
                "PERMINTAAN IZIN",
                Rect::new(mid_x, top_y, 86.0, 14.0),
                &TextStyle::default()
                    .size(8.8)
                    .bold()
                    .color(Color::white(0.48))
                    .vcenter()
                    .nowrap(),
            );
            let avail_bw = (mid_w - 88.0).max(36.0);
            let badge_st = TextStyle::default().size(8.5).bold();
            let (btw, _) = d2d.measure_text(&badge_lbl, avail_bw, &badge_st);
            let bw = (btw + 10.0).min(avail_bw).max(36.0);
            let badge_r = Rect::new(mid_x + 88.0, top_y, bw, 14.0);
            d2d.fill_rounded_rect(badge_r, 4.0, badge_col.with_alpha(0.18));
            d2d.stroke_rounded_rect(badge_r, 4.0, 1.0, badge_col.with_alpha(0.48));
            d2d.draw_text(
                &badge_lbl,
                badge_r,
                &TextStyle::default()
                    .size(8.5)
                    .bold()
                    .color(badge_col)
                    .center()
                    .vcenter()
                    .ellipsis(),
            );
            let title_txt = if req.title.is_empty() {
                "Permintaan izin"
            } else {
                req.title.as_str()
            };
            d2d.draw_text(
                title_txt,
                Rect::new(mid_x, top_y + 15.0, mid_w, 18.0),
                &TextStyle::default()
                    .size(13.0)
                    .semibold()
                    .color(Color::white(1.0))
                    .vcenter()
                    .ellipsis(),
            );
            if !req.detail.trim().is_empty() {
                let det_y = top_y + 35.0;
                let det_h = (notch_rect.bottom() - 12.0 - det_y).max(18.0);
                let det_r = Rect::new(mid_x, det_y, mid_w, det_h);
                d2d.draw_text(
                    &req.detail,
                    det_r,
                    &TextStyle::default()
                        .size(11.0)
                        .line_height(15.0)
                        .color(Color::white(0.55))
                        .ellipsis(),
                );
            }
        }

        PopupState::Capture {
            path,
            renaming,
            flash_start_ms,
            status_msg,
            ..
        } => {
            let thumb_r = Rect::new(area.x, area.y + 12.0, 96.0, 96.0);
            d2d.draw_image(path, thumb_r, 14.0, 1.0, true);
            d2d.stroke_rounded_rect(thumb_r, 14.0, 1.0, Color::white(0.16));

            // Initial white camera shutter flash (0..320ms)
            let elapsed = input.now_ms.saturating_sub(*flash_start_ms);
            if elapsed < 320 {
                let a = 1.0 - (elapsed as f32 / 320.0);
                d2d.fill_rounded_rect(thumb_r, 14.0, Color::white(a * 0.85));
            }

            let rx = thumb_r.right() + 14.0;
            let rw = area.right() - rx;

            let fname = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Screenshot");

            if *renaming {
                let ia = state.rename_input.draw(
                    d2d,
                    hits,
                    "cap_rename_inp",
                    Rect::new(rx, area.y + 8.0, rw - 28.0, 26.0),
                    "Nama file baru...",
                    false,
                    None,
                    input,
                );
                if ia.submitted {
                    if let Ok(new_path) = screenshot::rename_screenshot(path, &state.rename_input.text) {
                        *path = new_path;
                        *status_msg = "Nama diubah".to_string();
                    }
                    *renaming = false;
                } else if ia.escaped {
                    *renaming = false;
                }
            } else {
                d2d.draw_text(
                    fname,
                    Rect::new(rx, area.y + 8.0, rw - 54.0, 22.0),
                    &TextStyle::default()
                        .size(12.5)
                        .semibold()
                        .color(Color::white(0.94))
                        .vcenter()
                        .ellipsis(),
                );
                if draw_icon_btn(
                    d2d,
                    hits,
                    "cap_edit_name",
                    Rect::new(area.right() - 52.0, area.y + 8.0, 22.0, 22.0),
                    IconId::Edit,
                    12.0,
                    Color::white(0.55),
                    Color::white(0.10),
                    Some("Ubah nama"),
                    input,
                ) {
                    let stem = std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Screenshot");
                    state.rename_input.set_text(stem);
                    *renaming = true;
                }
            }

            if draw_icon_btn(
                d2d,
                hits,
                "cap_close",
                Rect::new(area.right() - 24.0, area.y + 8.0, 22.0, 22.0),
                IconId::Close,
                12.5,
                Color::white(0.55),
                Color::white(0.10),
                Some("Tutup"),
                input,
            ) {
                act.dismiss = true;
                return act;
            }

            let sub = if status_msg.is_empty() {
                "Screenshot baru tertangkap"
            } else {
                status_msg.as_str()
            };
            d2d.draw_text(
                sub,
                Rect::new(rx, area.y + 34.0, rw, 18.0),
                &TextStyle::default().size(10.5).color(Color::white(0.55)).ellipsis(),
            );

            let by = area.bottom() - 34.0;
            if draw_pill_btn(
                d2d,
                hits,
                "cap_open",
                Rect::new(rx, by, 54.0, 25.0),
                "Buka",
                false,
                None,
                input,
            ) {
                system::open_path_async(path);
                act.dismiss = true;
            }
            if draw_pill_btn(
                d2d,
                hits,
                "cap_reveal",
                Rect::new(rx + 60.0, by, 60.0, 25.0),
                "Folder",
                false,
                None,
                input,
            ) {
                system::reveal_path_async(path);
            }
            if draw_pill_btn(
                d2d,
                hits,
                "cap_ocr",
                Rect::new(rx + 126.0, by, 84.0, 25.0),
                "Salin Teks",
                false,
                None,
                input,
            ) {
                let p_clone = path.clone();
                let ui = svc.ui.clone();
                if let Some(ocr_eng) = svc.ocr.clone() {
                    std::thread::spawn(move || {
                        match ocr_eng.recognize_file(&p_clone) {
                            Ok(text) => {
                                let trimmed = text.trim();
                                if trimmed.is_empty() {
                                    ui.send(UiEvent::CaptureStatus(
                                        "Tidak ada teks terdeteksi".to_string(),
                                    ));
                                } else if clipboard::copy_text(trimmed) {
                                    ui.send(UiEvent::CaptureStatus(
                                        "Teks disalin ke clipboard ✓".to_string(),
                                    ));
                                } else {
                                    ui.send(UiEvent::CaptureStatus(
                                        "Gagal menyalin teks".to_string(),
                                    ));
                                }
                            }
                            Err(_) => {
                                ui.send(UiEvent::CaptureStatus(
                                    "Gagal membaca teks gambar".to_string(),
                                ));
                            }
                        }
                    });
                    *status_msg = "Mengekstrak teks OCR...".to_string();
                } else {
                    let _ = ocr::WinRtOcr::new();
                    *status_msg = "OCR tidak tersedia".to_string();
                }
            }
            if draw_pill_btn(
                d2d,
                hits,
                "cap_ask_ai",
                Rect::new(rx + 216.0, by, 80.0, 25.0),
                "Tanya AI",
                true,
                Some(accent),
                input,
            ) {
                act.ask_ai_image_path = Some(path.clone());
                act.dismiss = true;
            }
            if draw_icon_btn(
                d2d,
                hits,
                "cap_trash",
                Rect::new(area.right() - 26.0, by, 25.0, 25.0),
                IconId::Trash,
                13.0,
                Color::rgb(239, 68, 68),
                Color::rgba(239, 68, 68, 0.22),
                Some("Hapus screenshot"),
                input,
            ) {
                let _ = screenshot::discard_screenshot(path);
                act.dismiss = true;
            }
        }

        PopupState::Reminder {
            message,
            at_ms,
            start_ms,
            ..
        } => {
            let t = input.now_ms.saturating_sub(*start_ms) as f32 / 1000.0;
            let comp_r = Rect::new(area.x + 6.0, area.center_y() - 32.0, 62.0, 64.0);
            for i in 0..3 {
                let ph = (t * 0.62 + i as f32 * 0.33).fract();
                let rad = 22.0 + ph * 17.0;
                d2d.stroke_circle(
                    comp_r.center_x(),
                    comp_r.center_y(),
                    rad,
                    1.1,
                    Color::white((1.0 - ph) * 0.22),
                );
            }
            comp_state.mood = CompanionMood::Hop;
            let rc = comp_renderer.draw(
                d2d,
                hits,
                "rem_comp",
                comp_r,
                54.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                rc,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            // Pastel accent bell badge at top-right of mascot matching Foto 3
            let swing = (t * 7.5).sin() * 1.5;
            let badge_cx = comp_r.right() - 7.0 + swing;
            let badge_cy = comp_r.y + 10.0;
            d2d.fill_circle(badge_cx, badge_cy, 11.5, accent);
            d2d.draw_icon(
                IconId::Bell,
                Rect::new(badge_cx - 6.0, badge_cy - 6.0, 12.0, 12.0),
                Color::rgb(12, 12, 16),
                2.2,
                1.0,
            );

            let tx = comp_r.right() + 14.0;
            let tw = (area.right() - tx).max(120.0);
            let time_label = {
                use chrono::{DateTime, Local};
                if *at_ms > 0 {
                    DateTime::from_timestamp_millis(*at_ms)
                        .map(|dt| dt.with_timezone(&Local).format("%l:%M %p").to_string())
                } else {
                    None
                }
                .unwrap_or_else(|| Local::now().format("%l:%M %p").to_string())
                .trim()
                .to_string()
            };
            d2d.draw_text(
                &format!("REMINDER · {}", time_label),
                Rect::new(tx, area.y + 4.0, tw, 14.0),
                &TextStyle::default()
                    .size(10.0)
                    .bold()
                    .color(accent),
            );
            let msg_r = Rect::new(tx, area.y + 21.0, tw, 40.0);
            d2d.push_clip_rect(msg_r);
            d2d.draw_text(
                if message.trim().is_empty() { "It's time" } else { message },
                msg_r,
                &TextStyle::default()
                    .size(15.0)
                    .semibold()
                    .color(Color::white(0.96))
                    .line_height(18.5)
                    .ellipsis(),
            );
            d2d.pop_clip_rect();

            let by = area.bottom() - 28.0;
            let ok_r = Rect::new(tx, by, 76.0, 26.0);
            let ok_h = hits.register("rem_ok", ok_r, CursorKind::Pointer, None, input);
            d2d.fill_rounded_rect(ok_r, 13.0, accent.with_alpha(if ok_h.hovered { 1.0 } else { 0.94 }));
            d2d.draw_text(
                "Got it",
                ok_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::rgb(12, 12, 16))
                    .center()
                    .vcenter(),
            );
            if ok_h.clicked {
                act.dismiss = true;
            }
            if draw_pill_btn(
                d2d,
                hits,
                "rem_snooze",
                Rect::new(ok_r.right() + 7.0, by, 96.0, 26.0),
                "Snooze 5 min",
                false,
                None,
                input,
            ) {
                let mut rems: Vec<ReminderItem> =
                    serde_json::from_value(svc.store.get("reminders")).unwrap_or_default();
                rems.push(ReminderItem {
                    id: format!("snz{}", now_unix_ms()),
                    at: now_unix_ms() + 300_000,
                    message: message.clone(),
                });
                if let Ok(v) = serde_json::to_value(rems) {
                    svc.store.set(Some(&svc.ui), "reminders", v);
                }
                act.dismiss = true;
            }
        }

        PopupState::AgentCalendarAlert {
            event_id: _,
            title,
            time_label,
            minutes_before,
            note,
            meeting_link,
            start_ms,
            ..
        } => {
            let agent_cyan = Color::rgb(56, 189, 248);
            let t = input.now_ms.saturating_sub(*start_ms) as f32 / 1000.0;
            let comp_r = Rect::new(area.x + 4.0, area.center_y() - 24.0, 48.0, 48.0);
            let glow_a = 0.14 + 0.08 * (t * 2.6).sin().abs();
            d2d.fill_circle(
                comp_r.center_x(),
                comp_r.center_y(),
                26.0,
                agent_cyan.with_alpha(glow_a),
            );
            comp_state.mood = CompanionMood::Speaking;
            let ac = comp_renderer.draw(
                d2d,
                hits,
                "agent_cal_comp",
                comp_r,
                44.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                ac,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            // Sparkle badge at bottom-right of mascot (distinct from Reminder's bell/sonar)
            let bx = comp_r.right() - 6.0;
            let by_b = comp_r.bottom() - 8.0;
            d2d.fill_circle(bx, by_b, 9.5, agent_cyan);
            d2d.draw_icon(
                IconId::Sparkles,
                Rect::new(bx - 5.2, by_b - 5.2, 10.4, 10.4),
                Color::rgb(10, 12, 16),
                2.0,
                1.0,
            );

            let tx = comp_r.right() + 14.0;
            let priv_pad = if privacy.mic || privacy.camera { 24.0 } else { 0.0 };
            let row_right = area.right() - priv_pad;

            let time_chip_txt = if time_label.trim().is_empty() {
                "Segera".to_string()
            } else {
                time_label.trim().to_string()
            };
            let chip_st = TextStyle::default()
                .size(10.0)
                .medium()
                .color(Color::white(0.84))
                .vcenter()
                .nowrap();
            let (ctw, _) = d2d.measure_text(&time_chip_txt, 90.0, &chip_st);
            let chip_w = (ctw + 26.0).clamp(58.0, 102.0);
            let chip_r = Rect::new(row_right - chip_w, area.y + 1.0, chip_w, 18.0);
            d2d.fill_rounded_rect(chip_r, 9.0, Color::white(0.08));
            d2d.stroke_rounded_rect(chip_r, 9.0, 1.0, Color::white(0.12));
            d2d.draw_icon(
                IconId::Calendar,
                Rect::new(chip_r.x + 7.0, chip_r.center_y() - 5.0, 10.0, 10.0),
                agent_cyan,
                1.9,
                1.0,
            );
            d2d.draw_text(
                &time_chip_txt,
                Rect::new(chip_r.x + 19.0, chip_r.y, chip_r.w - 23.0, chip_r.h),
                &chip_st,
            );

            d2d.draw_icon(
                IconId::Sparkles,
                Rect::new(tx, area.y + 4.0, 11.0, 11.0),
                agent_cyan,
                2.0,
                1.0,
            );
            let meta_w = (chip_r.x - (tx + 15.0) - 8.0).max(80.0);
            d2d.draw_text(
                &format!("AGENT · {} MENIT LAGI", minutes_before),
                Rect::new(tx + 15.0, area.y + 2.0, meta_w, 15.0),
                &TextStyle::default()
                    .size(10.0)
                    .bold()
                    .color(agent_cyan)
                    .vcenter()
                    .ellipsis(),
            );

            let tw = (area.right() - tx).max(140.0);
            let title_disp = if title.trim().is_empty() {
                "Agenda Kalender".to_string()
            } else {
                title.trim().to_string()
            };
            d2d.draw_text(
                &title_disp,
                Rect::new(tx, area.y + 21.0, tw, 20.0),
                &TextStyle::default()
                    .size(14.0)
                    .semibold()
                    .color(Color::white(0.96))
                    .vcenter()
                    .ellipsis(),
            );

            let note_clean = if let Some(lk) = meeting_link {
                note.replace(&lk.url, "")
            } else {
                note.clone()
            };
            let note_trim = note_clean.trim();
            let sub_txt = if !note_trim.is_empty() {
                format!("Pukul {} · {}", time_chip_txt, note_trim)
            } else {
                format!(
                    "Agenda kalendermu dimulai {} menit lagi ({}).",
                    minutes_before, time_chip_txt
                )
            };
            d2d.draw_text(
                &sub_txt,
                Rect::new(tx, area.y + 42.0, tw, 17.0),
                &TextStyle::default()
                    .size(11.0)
                    .color(Color::white(0.64))
                    .vcenter()
                    .ellipsis(),
            );

            let by = area.bottom() - 25.0;
            if let Some(lk) = meeting_link.clone() {
                let oke_r = Rect::new(area.right() - 54.0, by, 54.0, 24.0);
                if draw_pill_btn(d2d, hits, "agent_cal_ok", oke_r, "Oke", false, None, input) {
                    act.dismiss = true;
                }
                let cal_r = Rect::new(oke_r.x - 80.0, by, 74.0, 24.0);
                if draw_pill_btn(
                    d2d,
                    hits,
                    "agent_cal_open",
                    cal_r,
                    "Calendar",
                    false,
                    None,
                    input,
                ) {
                    act.open_calendar = true;
                    act.dismiss = true;
                }
                let join_r = Rect::new(cal_r.x - 64.0, by, 58.0, 24.0);
                if draw_pill_btn(
                    d2d,
                    hits,
                    "agent_cal_join",
                    join_r,
                    "Join",
                    true,
                    Some(Color::rgb(37, 99, 235)),
                    input,
                ) {
                    svc.rt.spawn(async move {
                        let _ = system::join_meeting_smart(lk).await;
                    });
                    act.dismiss = true;
                }
            } else {
                let ok_r = Rect::new(area.right() - 82.0, by, 82.0, 24.0);
                let ok_h = hits.register("agent_cal_ok", ok_r, CursorKind::Pointer, None, input);
                d2d.fill_rounded_rect(
                    ok_r,
                    12.0,
                    agent_cyan.with_alpha(if ok_h.hovered { 1.0 } else { 0.92 }),
                );
                d2d.draw_text(
                    "Mengerti",
                    ok_r,
                    &TextStyle::default()
                        .size(11.0)
                        .semibold()
                        .color(Color::rgb(12, 12, 16))
                        .center()
                        .vcenter(),
                );
                if ok_h.clicked {
                    act.dismiss = true;
                }
                let cal_r = Rect::new(ok_r.x - 110.0, by, 102.0, 24.0);
                if draw_pill_btn(
                    d2d,
                    hits,
                    "agent_cal_open",
                    cal_r,
                    "Lihat Calendar",
                    false,
                    None,
                    input,
                ) {
                    act.open_calendar = true;
                    act.dismiss = true;
                }
            }
        }

        PopupState::Notif {
            payload,
            replying,
            ..
        } => {
            let av_r = Rect::new(area.x + 2.0, area.y + 2.0, 36.0, 36.0);
            let logo_path = crate::backend::notif::normalize_logo_path(&payload.logo);
            if !logo_path.is_empty() && std::path::Path::new(&logo_path).exists() {
                d2d.draw_image(&logo_path, av_r, 18.0, 1.0, true);
            } else {
                d2d.fill_circle(av_r.center_x(), av_r.center_y(), 18.0, accent.with_alpha(0.22));
                let ini = if payload.app.to_lowercase().contains("zapfast") {
                    "Z".to_string()
                } else {
                    payload
                        .app
                        .chars()
                        .next()
                        .unwrap_or('N')
                        .to_uppercase()
                        .to_string()
                };
                d2d.draw_text(
                    &ini,
                    av_r,
                    &TextStyle::default()
                        .size(14.5)
                        .bold()
                        .color(accent)
                        .center()
                        .vcenter(),
                );
            }
            if payload.is_call {
                let dx = av_r.right() - 4.0;
                let dy = av_r.bottom() - 4.0;
                d2d.fill_circle(dx, dy, 6.0, Color::rgb(11, 11, 13));
                d2d.fill_circle(dx, dy, 4.5, Color::rgb(34, 197, 94));
            }

            let tx = av_r.right() + 12.0;
            let tw = (area.right() - tx).max(140.0);
            let title_s = if payload.title.is_empty() {
                &payload.app
            } else {
                &payload.title
            };
            d2d.draw_text(
                title_s,
                Rect::new(tx, area.y + 2.0, (tw - 20.0).max(80.0), 19.0),
                &TextStyle::default()
                    .size(12.5)
                    .semibold()
                    .color(Color::white(0.95))
                    .ellipsis(),
            );
            let mut body_fallback = String::new();
            let body_s = notif_effective_body(payload, &mut body_fallback);
            let btn_top = if *replying {
                area.bottom() - 28.0
            } else {
                area.bottom() - 24.0
            };
            let body_avail_h = (btn_top - 8.0 - (area.y + 23.0)).max(15.5);
            let body_r = Rect::new(tx, area.y + 23.0, tw, body_avail_h);
            d2d.push_clip_rect(body_r);
            d2d.draw_text(
                body_s,
                body_r,
                &TextStyle::default()
                    .size(11.0)
                    .color(if payload.is_call {
                        Color::rgb(134, 239, 172)
                    } else {
                        Color::white(0.74)
                    })
                    .line_height(15.5)
                    .ellipsis(),
            );
            d2d.pop_clip_rect();

            let primary_lbl = notif_primary_action_label(
                &payload.app,
                &payload.site,
                &payload.title,
                &payload.body,
                !payload.inputs.is_empty(),
            );

            if *replying {
                let cap_y = area.bottom() - 28.0;
                let inp_r = Rect::new(tx, cap_y, (tw - 68.0).max(100.0), 28.0);
                let ph = payload
                    .inputs
                    .first()
                    .map(|i| i.placeholder.as_str())
                    .filter(|p| !p.is_empty())
                    .unwrap_or("Tulis balasan cepat...");
                let ra = state.notif_reply_input.draw_styled(
                    d2d,
                    hits,
                    "notif_reply_inp",
                    inp_r,
                    ph,
                    false,
                    Some(
                        TextStyle::default()
                            .size(11.5)
                            .color(Color::white(0.92))
                            .vcenter()
                            .nowrap(),
                    ),
                    &TextInputStyle::capsule(28.0),
                    input,
                );
                let send_hit = draw_pill_btn(
                    d2d,
                    hits,
                    "notif_reply_send",
                    Rect::new(inp_r.right() + 6.0, cap_y + 1.0, 62.0, 26.0),
                    "Kirim",
                    true,
                    Some(Color::rgb(52, 211, 153)),
                    input,
                );
                if ra.escaped {
                    *replying = false;
                    state.notif_reply_input.clear();
                } else if (ra.submitted || send_hit) && !state.notif_reply_input.text.trim().is_empty()
                {
                    let reply_txt = state.notif_reply_input.text.trim().to_string();
                    let raw_a = if payload.raw_app.trim().is_empty() {
                        payload.app.clone()
                    } else {
                        payload.raw_app.clone()
                    };
                    let act_args = payload
                        .actions
                        .iter()
                        .find(|a| !a.input_id.is_empty())
                        .map(|a| a.args.clone())
                        .unwrap_or_else(|| payload.launch.clone());
                    let inp_id = payload
                        .inputs
                        .first()
                        .map(|i| i.id.clone())
                        .unwrap_or_default();
                    let u = crate::backend::notif::target_url_of(
                        &payload.launch,
                        &payload.jid,
                        &payload.site,
                    );
                    let p = crate::backend::notif::parse_profile(&payload.launch);
                    let s = payload.title.clone();
                    let self_hwnd = svc.ui.get_hwnd();
                    let ui_tx = svc.ui.clone();
                    let body_txt = payload.body.clone();
                    svc.rt.spawn(async move {
                        if system::notif_send_reply(&raw_a, &act_args, &inp_id, &reply_txt).await {
                            return;
                        }
                        // Zen/Firefox toasts carry no reply input, so native
                        // quick reply cannot work. Open the sender chat and
                        // paste the draft there — never press Enter.
                        let opened = system::notif_open_rich(&raw_a, &act_args, &u, &p, &s).await;
                        let routed = opened.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                        if !routed {
                            return;
                        }
                        let txt = reply_txt.clone();
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
                        let _ = body_txt;
                    });
                    state.notif_reply_input.clear();
                    *replying = false;
                    act.dismiss = true;
                }
            } else if payload.is_call {
                let by = area.bottom() - 24.0;
                let mut rx = area.right();
                rx -= 68.0;
                if draw_pill_btn(
                    d2d,
                    hits,
                    "notif_call_open",
                    Rect::new(rx, by, 68.0, 24.0),
                    "Buka",
                    true,
                    Some(Color::rgb(34, 197, 94)),
                    input,
                ) {
                    let p_clone = payload.clone();
                    svc.rt.spawn(async move {
                        system::focus_call_window_no_uia(p_clone).await;
                    });
                    act.dismiss = true;
                }
                rx -= 66.0;
                if draw_pill_btn(
                    d2d,
                    hits,
                    "notif_call_close",
                    Rect::new(rx, by, 60.0, 24.0),
                    "Tutup",
                    false,
                    None,
                    input,
                ) {
                    act.dismiss = true;
                }
            } else {
                let by = area.bottom() - 24.0;
                let mut rx = area.right();
                rx -= 58.0;
                if draw_pill_btn(
                    d2d,
                    hits,
                    "notif_pop_ok",
                    Rect::new(rx, by, 58.0, 24.0),
                    "OK",
                    true,
                    Some(accent),
                    input,
                ) {
                    act.dismiss = true;
                }
                rx -= 78.0;
                if draw_pill_btn(
                    d2d,
                    hits,
                    "notif_pop_open",
                    Rect::new(rx, by, 72.0, 24.0),
                    primary_lbl,
                    false,
                    None,
                    input,
                ) {
                    if primary_lbl == "Balas" && !payload.inputs.is_empty() {
                        *replying = true;
                        state.notif_reply_input.clear();
                        hits.focused_input = Some("input:notif_reply_inp".to_string());
                        // Remember the chat window NOW, before the notch takes
                        // focus for typing, so a failed native reply can still
                        // paste into the right window later.
                        crate::backend::win::capture_paste_target(svc.ui.get_hwnd());
                    } else {
                        let h = if payload.raw_app.trim().is_empty() {
                            payload.app.clone()
                        } else {
                            payload.raw_app.clone()
                        };
                        let u = crate::backend::notif::target_url_of(
                            &payload.launch,
                            &payload.jid,
                            &payload.site,
                        );
                        let p = crate::backend::notif::parse_profile(&payload.launch);
                        let ln = payload.launch.clone();
                        let s = payload.title.clone();
                        svc.rt.spawn(async move {
                            system::notif_open_rich(&h, &ln, &u, &p, &s).await;
                        });
                        act.dismiss = true;
                    }
                }
                if let Some(otp) = extract_otp_code(&payload.body) {
                    let lbl = format!("Salin {}", otp);
                    let w_otp = (lbl.len() as f32 * 6.5 + 18.0).clamp(74.0, 118.0);
                    rx -= w_otp + 6.0;
                    if draw_pill_btn(
                        d2d,
                        hits,
                        "notif_pop_otp",
                        Rect::new(rx, by, w_otp, 24.0),
                        &lbl,
                        false,
                        Some(Color::rgb(52, 211, 153)),
                        input,
                    ) {
                        clipboard::copy_text(&otp);
                        act.dismiss = true;
                    }
                }
            }
        }

        PopupState::Meeting {
            title,
            subtitle,
            link,
            ..
        } => {
            let m_area = notch_rect.inset(14.0, 12.0);
            let badge = Rect::new(m_area.x, m_area.center_y() - 17.0, 34.0, 34.0);
            d2d.fill_rounded_rect(badge, 10.0, Color::rgb(37, 99, 235));
            d2d.draw_icon(
                IconId::Video,
                Rect::new(badge.center_x() - 8.5, badge.center_y() - 8.5, 17.0, 17.0),
                Color::white(0.98),
                1.8,
                1.0,
            );

            let close_r = Rect::new(m_area.right() - 20.0, m_area.center_y() - 10.0, 20.0, 20.0);
            if draw_icon_btn(
                d2d,
                hits,
                "meet_close",
                close_r,
                IconId::Close,
                11.0,
                Color::white(0.45),
                Color::white(0.10),
                Some("Tutup"),
                input,
            ) {
                act.dismiss = true;
                return act;
            }

            let join_r = Rect::new(close_r.x - 62.0, m_area.center_y() - 14.0, 56.0, 28.0);
            let jh = hits.register(
                "meet_join_btn",
                join_r,
                CursorKind::Pointer,
                Some(format!("Gabung via {}", link.provider.label())),
                input,
            );
            d2d.fill_rounded_rect(
                join_r,
                11.0,
                if jh.hovered {
                    Color::rgb(59, 130, 246)
                } else {
                    Color::rgb(37, 99, 235)
                },
            );
            d2d.draw_text(
                "Join",
                join_r,
                &TextStyle::default()
                    .size(11.5)
                    .bold()
                    .color(Color::white(0.98))
                    .center()
                    .vcenter(),
            );
            if jh.clicked {
                let lk = link.clone();
                svc.rt.spawn(async move {
                    let _ = system::join_meeting_smart(lk).await;
                });
                act.dismiss = true;
            }

            let tx = badge.right() + 10.0;
            let tw = (join_r.x - tx - 8.0).max(80.0);
            d2d.draw_text(
                title,
                Rect::new(tx, m_area.center_y() - 16.0, tw, 17.0),
                &TextStyle::default()
                    .size(12.5)
                    .semibold()
                    .color(Color::white(0.96))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                subtitle,
                Rect::new(tx, m_area.center_y() + 1.0, tw, 15.0),
                &TextStyle::default()
                    .size(10.5)
                    .color(Color::white(0.56))
                    .vcenter()
                    .ellipsis(),
            );
        }

        PopupState::Offline { .. } => {
            let o_area = notch_rect.inset(14.0, 12.0);
            let badge = Rect::new(o_area.x, o_area.center_y() - 17.0, 34.0, 34.0);
            d2d.fill_rounded_rect(badge, 17.0, Color::rgb(127, 29, 29));
            d2d.draw_icon(
                IconId::Wifi,
                Rect::new(badge.center_x() - 8.5, badge.center_y() - 8.5, 17.0, 17.0),
                Color::rgb(248, 113, 113),
                1.8,
                1.0,
            );
            // Diagonal slash over the wifi glyph = offline.
            d2d.draw_line(
                badge.x + 8.0,
                badge.bottom() - 8.0,
                badge.right() - 8.0,
                badge.y + 8.0,
                2.0,
                Color::rgb(248, 113, 113),
            );

            let close_r = Rect::new(o_area.right() - 20.0, o_area.center_y() - 10.0, 20.0, 20.0);
            if draw_icon_btn(
                d2d,
                hits,
                "offline_close",
                close_r,
                IconId::Close,
                11.0,
                Color::white(0.45),
                Color::white(0.10),
                Some("Tutup"),
                input,
            ) {
                act.dismiss = true;
                return act;
            }

            let tx = badge.right() + 10.0;
            let tw = (close_r.x - tx - 8.0).max(80.0);
            d2d.draw_text(
                "Offline",
                Rect::new(tx, o_area.center_y() - 16.0, tw, 17.0),
                &TextStyle::default()
                    .size(12.5)
                    .semibold()
                    .color(Color::white(0.96))
                    .vcenter()
                    .ellipsis(),
            );
            d2d.draw_text(
                "Koneksi internet terputus",
                Rect::new(tx, o_area.center_y() + 1.0, tw, 15.0),
                &TextStyle::default()
                    .size(10.5)
                    .color(Color::white(0.56))
                    .vcenter()
                    .ellipsis(),
            );
        }

        PopupState::Done { duration_sec, .. } => {
            // Foto 3/4: centered mascot + seamless solid white speech bubble tail + pill
            let bub_w = 144.0;
            let top_w = 52.0 + 12.0 + bub_w;
            let top_x = area.center_x() - top_w * 0.5;
            let top_y = area.y + 4.0;
            let comp_r = Rect::new(top_x, top_y, 52.0, 52.0);
            comp_state.mood = CompanionMood::Hop;
            let done_clicked = comp_renderer.draw(
                d2d,
                hits,
                "done_comp",
                comp_r,
                46.0,
                &settings.avatar,
                settings.accent_hex(),
                custom_photo,
                comp_state,
                input,
            );
            CompanionRenderer::cycle_avatar_on_click(
                done_clicked,
                settings,
                custom_photo,
                &svc.store,
                &svc.ui,
            );

            let bub = Rect::new(comp_r.right() + 12.0, top_y + 3.0, bub_w, 46.0);
            let bub_col = Color::white(0.96);
            d2d.fill_triangle(
                (bub.x + 6.0, bub.center_y() - 6.0),
                (bub.x - 6.5, bub.center_y()),
                (bub.x + 6.0, bub.center_y() + 6.0),
                bub_col,
            );
            d2d.fill_rounded_rect(bub, 14.0, bub_col);
            d2d.draw_text(
                "All done!",
                Rect::new(bub.x + 14.0, bub.y + 7.0, bub.w - 28.0, 17.0),
                &TextStyle::default()
                    .size(13.0)
                    .bold()
                    .color(Color::rgb(12, 12, 16))
                    .vcenter(),
            );
            d2d.draw_text(
                "Focus complete",
                Rect::new(bub.x + 14.0, bub.y + 24.0, bub.w - 28.0, 14.0),
                &TextStyle::default()
                    .size(10.0)
                    .medium()
                    .color(Color::rgba(12, 12, 16, 0.52))
                    .vcenter(),
            );

            let dur_lbl = if *duration_sec < 60 {
                format!("{} sec", duration_sec)
            } else {
                format!("{} min", (*duration_sec + 30) / 60)
            };
            let dur_st = TextStyle::default().size(11.0).semibold().color(accent);
            let of_st = TextStyle::default().size(11.0).medium().color(Color::white(0.50));
            let (dur_w, _) = d2d.measure_text(&dur_lbl, 80.0, &dur_st);
            let (of_w, _) = d2d.measure_text(" of focus", 80.0, &of_st);
            let again_w = 68.0;
            let done_w = 68.0;
            let bot_w = dur_w + of_w + 12.0 + again_w + 6.0 + done_w;
            let mut bx = area.center_x() - bot_w * 0.5;
            let by = area.bottom() - 28.0;
            d2d.draw_text(&dur_lbl, Rect::new(bx, by, dur_w + 2.0, 26.0), &dur_st.vcenter());
            bx += dur_w;
            d2d.draw_text(" of focus", Rect::new(bx, by, of_w + 2.0, 26.0), &of_st.vcenter());
            bx += of_w + 12.0;

            let again_r = Rect::new(bx, by, again_w, 26.0);
            let again_h = hits.register("done_again", again_r, CursorKind::Pointer, None, input);
            d2d.fill_rounded_rect(again_r, 13.0, accent.with_alpha(if again_h.hovered { 1.0 } else { 0.94 }));
            d2d.draw_text(
                "Again",
                again_r,
                &TextStyle::default()
                    .size(11.0)
                    .semibold()
                    .color(Color::rgb(12, 12, 16))
                    .center()
                    .vcenter(),
            );
            if again_h.clicked {
                act.restart_focus = true;
                act.dismiss = true;
            }
            if draw_pill_btn(
                d2d,
                hits,
                "done_ok",
                Rect::new(again_r.right() + 6.0, by, done_w, 26.0),
                "Done",
                false,
                None,
                input,
            ) {
                act.dismiss = true;
            }
        }
    }

    if esc_pressed && !act.dismiss {
        match popup {
            PopupState::Approval { req, .. } => {
                if !svc.approval.respond(&req.id, "deny") {
                    svc.pipe.respond(&req.id, "deny");
                }
                act.dismiss = true;
            }
            PopupState::WhisperSetup { .. }
            | PopupState::AgentCalendarAlert { .. }
            | PopupState::Meeting { .. }
            | PopupState::Reminder { .. }
            | PopupState::Done { .. }
            | PopupState::Offline { .. }
            | PopupState::Notif { replying: false, .. } => {
                act.dismiss = true;
            }
            _ => {}
        }
    }

    let _ = (PI, approval_gate::play_done_chime);
    act
}

pub fn approval_provider_badge(req: &ApprovalRequest, is_internal: bool) -> (String, Color) {
    let src_low = req.source.trim().to_lowercase();
    let tool_low = req.tool.trim().to_lowercase();
    let haystack = if !src_low.is_empty() {
        format!("{} {}", src_low, tool_low)
    } else {
        tool_low
    };
    if haystack.contains("opencode") {
        return ("OPENCODE".to_string(), Color::rgb(96, 165, 250));
    }
    if haystack.contains("zcode") {
        return ("ZCODE".to_string(), Color::rgb(192, 132, 252));
    }
    if haystack.contains("claude") {
        return ("CLAUDE CODE".to_string(), Color::rgb(251, 146, 60));
    }
    if haystack.contains("codex") {
        return ("CODEX".to_string(), Color::rgb(52, 211, 153));
    }
    if haystack.contains("cursor") {
        return ("CURSOR".to_string(), Color::rgb(56, 189, 248));
    }
    if is_internal
        || req.id.starts_with("dn-local-")
        || req.id.starts_with("sb-local-")
        || src_low == "selabar ai"
        || src_low == "selabar"
    {
        return ("SELABAR AI".to_string(), Color::rgb(249, 115, 22));
    }
    if !req.source.trim().is_empty() {
        return (req.source.trim().to_uppercase(), Color::rgb(56, 189, 248));
    }
    if !req.tool.trim().is_empty() {
        return (req.tool.trim().to_uppercase(), Color::rgb(56, 189, 248));
    }
    ("EXTERNAL AI".to_string(), Color::rgb(148, 163, 184))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_provider_badge_maps_providers_and_internal() {
        let oc = ApprovalRequest {
            id: "req-1".into(),
            tool: "opencode".into(),
            ..Default::default()
        };
        assert_eq!(approval_provider_badge(&oc, false).0, "OPENCODE");

        let zc_tool = ApprovalRequest {
            id: "req-2".into(),
            tool: "zcode".into(),
            ..Default::default()
        };
        assert_eq!(approval_provider_badge(&zc_tool, false).0, "ZCODE");

        let zc_src = ApprovalRequest {
            id: "req-3".into(),
            tool: "bash".into(),
            source: "ZCode".into(),
            ..Default::default()
        };
        assert_eq!(approval_provider_badge(&zc_src, false).0, "ZCODE");

        let local = ApprovalRequest {
            id: "dn-local-12-1".into(),
            tool: "shell".into(),
            ..Default::default()
        };
        assert_eq!(approval_provider_badge(&local, true).0, "SELABAR AI");

        let fb = ApprovalRequest::default();
        assert_eq!(approval_provider_badge(&fb, false).0, "EXTERNAL AI");

        assert_eq!(
            notif_primary_action_label("WhatsApp", "web.whatsapp.com", "Budi", "Panggilan suara masuk", false),
            "Buka"
        );
        assert_eq!(
            notif_primary_action_label("WhatsApp", "web.whatsapp.com", "Budi", "Halo lagi apa?", false),
            "Balas"
        );
        assert_eq!(
            notif_primary_action_label("ZapFast", "", "Budi", "Sticker", false),
            "Balas"
        );
    }

    #[test]
    fn notif_popup_height_scales_from_compact_to_multiline() {
        let short = notif_popup_height(15.5, false);
        let medium = notif_popup_height(46.5, false);
        let tall = notif_popup_height(140.0, false);
        assert!(short <= 100.0);
        assert!(medium > short);
        assert_eq!(tall, 161.0);
        assert!(notif_popup_height(15.5, true) > short);
        assert_eq!(notif_popup_height(140.0, true), 165.0);
    }
}
