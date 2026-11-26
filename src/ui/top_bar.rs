use crate::events::{ActiveCallInfo, PrivacySnapshot};
use crate::ui::d2d::{Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::IconId;
use crate::ui::widgets::{draw_icon_btn, CursorKind, HitRegistry, UiInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Glance,
    Desk,
    Files,
    Music,
    Calendar,
    Conversation,
    Notifs,
    Settings,
}

impl ActiveTab {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActiveTab::Glance => "glance",
            ActiveTab::Desk => "desk",
            ActiveTab::Files => "files",
            ActiveTab::Music => "music",
            ActiveTab::Calendar => "cal",
            ActiveTab::Conversation => "conversation",
            ActiveTab::Notifs => "notifTab",
            ActiveTab::Settings => "settings",
        }
    }

    pub fn from_enabled_key(k: &str) -> Option<Self> {
        match k {
            "glance" | "home" => Some(ActiveTab::Glance),
            "desk" | "focus" | "tasks" => Some(ActiveTab::Desk),
            "files" | "clipboard" => Some(ActiveTab::Files),
            "music" | "media" => Some(ActiveTab::Music),
            "cal" | "calendar" => Some(ActiveTab::Calendar),
            "conversation" | "chat" | "ai" => Some(ActiveTab::Conversation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TopBarAction {
    pub select_tab: Option<ActiveTab>,
    pub toggle_pin: bool,
    pub toggle_dnd: bool,
    pub end_call: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_top_bar(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    active: ActiveTab,
    indicator_x: &mut f32,
    hidden_views: &[String],
    pinned: bool,
    dnd: bool,
    unread_notifs: usize,
    focus_running: bool,
    media_playing: bool,
    has_update: bool,
    privacy: &PrivacySnapshot,
    active_call: Option<&ActiveCallInfo>,
    accent: Color,
    input: &UiInput,
) -> TopBarAction {
    let mut act = TopBarAction::default();
    let bar = Rect::new(notch_rect.x + 14.0, notch_rect.y + 6.0, notch_rect.w - 28.0, 26.0);

    let all_tabs: [(ActiveTab, &str, IconId, &str); 6] = [
        (ActiveTab::Glance, "glance", IconId::Home, "Home"),
        (ActiveTab::Desk, "desk", IconId::Clock, "Focus & Tasks"),
        (ActiveTab::Files, "files", IconId::Folder, "Clipboard & Files"),
        (ActiveTab::Music, "music", IconId::Music, "Music & Lyrics"),
        (ActiveTab::Calendar, "cal", IconId::Calendar, "Calendar"),
        (ActiveTab::Conversation, "conversation", IconId::Chat, "Chat AI"),
    ];

    let mut has_active_left_tab = false;
    let mut x = bar.x;
    for (tab, key, icon, tip) in all_tabs {
        let hidden = key != "glance"
            && hidden_views
                .iter()
                .any(|v| ActiveTab::from_enabled_key(v) == Some(tab));
        if hidden {
            continue;
        }
        let btn = Rect::new(x, bar.y, 26.0, 24.0);
        let is_act = active == tab;
        let fg = if is_act {
            Color::white(0.98)
        } else {
            Color::white(0.45)
        };
        if draw_icon_btn(
            d2d,
            hits,
            &format!("tb_{}", key),
            btn,
            icon,
            13.5,
            fg,
            Color::white(0.08),
            Some(tip),
            input,
        ) {
            act.select_tab = Some(tab);
        }
        if is_act {
            has_active_left_tab = true;
            let target_x = btn.center_x() - 7.0;
            if *indicator_x <= 0.0 {
                *indicator_x = target_x;
            } else {
                *indicator_x += (target_x - *indicator_x) * 0.36;
            }
        }

        // Subtle live activity indicator dots on Desk / Music tab buttons
        if tab == ActiveTab::Desk && focus_running {
            d2d.fill_circle(btn.right() - 4.5, btn.y + 5.0, 2.0, Color::rgb(52, 211, 153));
        } else if tab == ActiveTab::Music && media_playing {
            d2d.fill_circle(btn.right() - 4.5, btn.y + 5.0, 2.0, accent);
        }

        x += 28.0;
    }

    if has_active_left_tab && *indicator_x > 0.0 {
        d2d.fill_rounded_rect(
            Rect::new(*indicator_x, bar.bottom() - 1.5, 14.0, 2.0),
            1.0,
            Color::white(0.95),
        );
    }

    // Right utility rail: Settings, Pin, Notifications/DND, plus live privacy dots
    let mut rx = bar.right();

    // Settings
    rx -= 26.0;
    let set_btn = Rect::new(rx, bar.y, 26.0, 24.0);
    let set_act = active == ActiveTab::Settings;
    if set_act {
        d2d.fill_rounded_rect(set_btn, 8.0, Color::white(0.14));
    }
    if draw_icon_btn(
        d2d,
        hits,
        "tb_settings",
        set_btn,
        IconId::Settings,
        13.5,
        if set_act { Color::white(0.95) } else { Color::white(0.48) },
        Color::white(0.09),
        Some(if has_update {
            "Settings (Update SelaBar tersedia!)"
        } else {
            "Settings"
        }),
        input,
    ) {
        act.select_tab = Some(ActiveTab::Settings);
    }
    if has_update {
        d2d.fill_circle(
            set_btn.right() - 4.5,
            set_btn.y + 5.0,
            2.4,
            Color::rgb(52, 211, 153),
        );
    }

    // Pin
    rx -= 28.0;
    let pin_btn = Rect::new(rx, bar.y, 26.0, 24.0);
    if pinned {
        d2d.fill_rounded_rect(pin_btn, 8.0, Color::white(0.16));
    }
    if draw_icon_btn(
        d2d,
        hits,
        "tb_pin",
        pin_btn,
        IconId::Pin,
        13.0,
        if pinned { Color::white(0.96) } else { Color::white(0.48) },
        Color::white(0.09),
        Some(if pinned { "Lepas pin" } else { "Pin notch terbuka" }),
        input,
    ) {
        act.toggle_pin = true;
    }

    // Bell / Notifications + right-click DND
    rx -= 28.0;
    let bell_btn = Rect::new(rx, bar.y, 26.0, 24.0);
    let bell_act = active == ActiveTab::Notifs;
    if bell_act {
        d2d.fill_rounded_rect(bell_btn, 8.0, Color::white(0.14));
    } else if dnd {
        d2d.fill_rounded_rect(bell_btn, 8.0, Color::rgba(239, 68, 68, 0.14));
    }
    let bell_hit = hits.register(
        "tb_bell",
        bell_btn,
        CursorKind::Pointer,
        Some(if dnd {
            "Notifikasi (DND aktif — klik kanan matikan)".to_string()
        } else {
            "Notifikasi (klik kanan untuk DND)".to_string()
        }),
        input,
    );
    if bell_hit.hovered && !bell_act {
        d2d.fill_rounded_rect(bell_btn, 8.0, Color::white(0.08));
    }
    let bell_col = if dnd {
        Color::rgb(239, 68, 68)
    } else if bell_act {
        Color::white(0.95)
    } else {
        Color::white(0.48)
    };
    d2d.draw_icon(
        if dnd { IconId::BellOff } else { IconId::Bell },
        Rect::new(bell_btn.center_x() - 6.75, bell_btn.center_y() - 6.75, 13.5, 13.5),
        bell_col,
        1.85,
        1.0,
    );
    if !dnd && unread_notifs > 0 {
        d2d.fill_circle(bell_btn.right() - 4.5, bell_btn.y + 5.0, 2.4, Color::rgb(56, 189, 248));
    }
    if bell_hit.clicked {
        act.select_tab = Some(ActiveTab::Notifs);
    }
    if bell_hit.right_clicked {
        act.toggle_dnd = true;
    }

    // Privacy LEDs + optional active call indicator pill to the left of Bell
    rx -= 8.0;
    let mut util_left = crate::ui::popups::draw_privacy_dots(
        d2d,
        hits,
        "tb_priv",
        rx + 4.0,
        bar.center_y(),
        privacy,
        input,
    );
    if let Some(call) = active_call {
        let secs = input.now_ms.saturating_sub(call.started_at_ms) / 1000;
        let dur_txt = format!("{:02}:{:02}", secs / 60, secs % 60);
        let pill_w = 58.0;
        util_left -= pill_w + 6.0;
        if util_left >= x + 6.0 {
            let pr = Rect::new(util_left, bar.y + 2.0, pill_w, 20.0);
            let ch = hits.register(
                "tb_active_call_end",
                pr,
                CursorKind::Pointer,
                Some("Panggilan aktif — klik untuk hentikan mode telepon".to_string()),
                input,
            );
            let fill = if ch.hovered {
                Color::rgba(239, 68, 68, 0.24)
            } else {
                Color::rgba(34, 197, 94, 0.20)
            };
            let stroke = if ch.hovered {
                Color::rgba(239, 68, 68, 0.55)
            } else {
                Color::rgba(34, 197, 94, 0.45)
            };
            let fg = if ch.hovered {
                Color::rgb(252, 165, 165)
            } else {
                Color::rgb(74, 222, 128)
            };
            d2d.fill_rounded_rect(pr, 10.0, fill);
            d2d.stroke_rounded_rect(pr, 10.0, 1.0, stroke);
            d2d.draw_icon(
                IconId::Phone,
                Rect::new(pr.x + 6.0, pr.center_y() - 5.0, 10.0, 10.0),
                fg,
                2.0,
                1.0,
            );
            d2d.draw_text(
                &dur_txt,
                Rect::new(pr.x + 17.0, pr.y, pr.w - 21.0, pr.h),
                &TextStyle::default()
                    .size(10.0)
                    .semibold()
                    .color(fg)
                    .center()
                    .vcenter()
                    .nowrap(),
            );
            if ch.clicked {
                act.end_call = true;
            }
        }
    }

    act
}
