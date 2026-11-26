use crate::backend::clipboard;
use crate::backend::store::StoreManager;
use crate::backend::system;
use crate::events::UiSender;
use crate::state::{ClipboardEntry, FileCardInfo, FileHit};
use crate::ui::d2d::{rasterize_svg_premul_bgra, Color, D2dEngine, Rect, TextStyle};
use crate::ui::icons::{file_badge_style, IconId};
use crate::ui::widgets::{
    draw_icon_btn, draw_pill_btn, CursorKind, HitRegistry, ScrollState, TextInputState,
    TextInputStyle, UiInput,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use std::collections::HashMap;

fn copy_clip_entry(entry: &ClipboardEntry) {
    if !entry.image.is_empty() {
        if let Some((_, b64)) = entry.image.split_once(";base64,") {
            if let Ok(bytes) = B64.decode(b64.trim()) {
                let _ = clipboard::copy_image_bytes_silent(&bytes);
                return;
            }
        }
        let _ = clipboard::copy_image_bytes_silent(entry.image.as_bytes());
    } else {
        let _ = clipboard::copy_text_silent(&entry.text);
    }
}

fn is_link_clip(c: &ClipboardEntry) -> bool {
    if !c.image.is_empty() {
        return false;
    }
    if c.kind == "url" || c.kind == "link" {
        return true;
    }
    let t = c.text.trim();
    t.starts_with("http://") || t.starts_with("https://")
}

fn build_file_icon_svg(badge: &str, hex: &str, is_dir: bool) -> String {
    if is_dir {
        return r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M5 12 C5 9.8 6.8 8 9 8 L19.5 8 L23.5 12.5 L39 12.5 C41.2 12.5 43 14.3 43 16.5 L43 37 C43 39.2 41.2 41 39 41 L9 41 C6.8 41 5 39.2 5 37 Z" fill="#d97706"/>
<rect x="5" y="15" width="38" height="26" rx="4" fill="#f59e0b"/>
<path d="M6 17.5 C6 16.1 7.1 15 8.5 15 L39.5 15 C40.9 15 42 16.1 42 17.5 L42 20 L6 20 Z" fill="#fbbf24" opacity="0.72"/>
</svg>"##.to_string();
    }

    match badge {
        "PDF" => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#f8fafc" stroke="#cbd5e1" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#fca5a5"/>
<path d="M17.5 23.5 C19.8 18 22.2 12.5 22.8 12.5 C23.6 12.5 24 19 27.5 22.5 C30 24.8 31.8 23.2 29.8 21.8 C26.5 19.6 18.5 21.8 15.8 24.5 C14.5 25.8 16.1 26.8 17.5 23.5 Z" fill="none" stroke="#ef4444" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="#ef4444"/>
</svg>"##.to_string(),
        "DOC" => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#f8fafc" stroke="#cbd5e1" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#93c5fd"/>
<rect x="13" y="13.5" width="12" height="2.4" rx="1.2" fill="#3b82f6"/>
<rect x="13" y="18.5" width="19" height="2.2" rx="1.1" fill="#94a3b8"/>
<rect x="13" y="23" width="15" height="2.2" rx="1.1" fill="#94a3b8"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="#2563eb"/>
</svg>"##.to_string(),
        "XLS" => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#f8fafc" stroke="#cbd5e1" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#6ee7b7"/>
<rect x="13" y="13" width="9" height="4.5" rx="1" fill="#10b981" opacity="0.85"/>
<rect x="24" y="13" width="9" height="4.5" rx="1" fill="#34d399" opacity="0.65"/>
<rect x="13" y="19.5" width="9" height="4.5" rx="1" fill="#34d399" opacity="0.65"/>
<rect x="24" y="19.5" width="9" height="4.5" rx="1" fill="#10b981" opacity="0.85"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="#059669"/>
</svg>"##.to_string(),
        "PPT" => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#f8fafc" stroke="#cbd5e1" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#fdba74"/>
<circle cx="19" cy="19" r="5" fill="#f97316" opacity="0.85"/>
<rect x="26" y="15" width="7" height="2.4" rx="1.2" fill="#94a3b8"/>
<rect x="26" y="19.5" width="5.5" height="2.4" rx="1.2" fill="#cbd5e1"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="#ea580c"/>
</svg>"##.to_string(),
        "ZIP" => r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#fef3c7" stroke="#f59e0b" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#fbbf24"/>
<rect x="20" y="7" width="4" height="3" rx="0.8" fill="#b45309"/>
<rect x="22" y="11" width="4" height="3" rx="0.8" fill="#b45309"/>
<rect x="20" y="15" width="4" height="3" rx="0.8" fill="#b45309"/>
<rect x="22" y="19" width="4" height="3" rx="0.8" fill="#b45309"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="#d97706"/>
</svg>"##.to_string(),
        "CODE" | "JSON" | "PY" | "JS" | "TS" | "RS" | "HTML" | "CSS" => format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#1e293b" stroke="#475569" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#334155"/>
<path d="M18 16 L13.5 20.5 L18 25" fill="none" stroke="{hex}" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>
<path d="M28 16 L32.5 20.5 L28 25" fill="none" stroke="{hex}" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="{hex}"/>
</svg>"##
        ),
        _ => format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<path d="M10 4 L29 4 L39 14 L39 42 C39 43.7 37.7 45 36 45 L10 45 C8.3 45 7 43.7 7 42 L7 7 C7 5.3 8.3 4 10 4 Z" fill="#f8fafc" stroke="#cbd5e1" stroke-width="1.2"/>
<path d="M29 4 L39 14 L31.5 14 C30.1 14 29 12.9 29 11.5 Z" fill="#cbd5e1"/>
<rect x="13" y="14" width="13" height="2.4" rx="1.2" fill="{hex}" opacity="0.8"/>
<rect x="13" y="19" width="18" height="2.2" rx="1.1" fill="#94a3b8"/>
<rect x="13" y="23.5" width="14" height="2.2" rx="1.1" fill="#94a3b8"/>
<rect x="9" y="29" width="28" height="12" rx="3" fill="{hex}"/>
</svg>"##
        ),
    }
}

fn draw_file_type_logo(
    d2d: &mut D2dEngine,
    cache: &mut HashMap<String, Vec<u8>>,
    card: Rect,
    ext: &str,
    is_dir: bool,
) {
    let (badge, hex) = file_badge_style(ext, is_dir);
    let key = if is_dir {
        "DIR".to_string()
    } else {
        badge.to_string()
    };
    if !cache.contains_key(&key) {
        if cache.len() >= 32 {
            cache.clear();
        }
        let svg = build_file_icon_svg(badge, hex, is_dir);
        if let Some(bgra) = rasterize_svg_premul_bgra(&svg, 96, 96) {
            cache.insert(key.clone(), bgra);
        }
    }
    let ico_r = Rect::new(card.center_x() - 23.0, card.y + 8.0, 46.0, 46.0);
    if let Some(bgra) = cache.get(&key) {
        let slot = format!("file_ico:{}", key);
        d2d.update_dynamic_bgra(&slot, 96, 96, 1, bgra);
        d2d.draw_dynamic_slot(&slot, ico_r, 1.0);
    }
    if !is_dir {
        let rib_y = ico_r.y + 46.0 * (29.0 / 48.0);
        let rib_h = 46.0 * (12.0 / 48.0);
        let rib_r = Rect::new(ico_r.x + 46.0 * (9.0 / 48.0), rib_y, 46.0 * (28.0 / 48.0), rib_h);
        d2d.draw_text(
            badge,
            rib_r,
            &TextStyle::default()
                .size(if badge.len() >= 4 { 7.5 } else { 8.5 })
                .bold()
                .color(Color::white(0.98))
                .center()
                .vcenter(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesSubTab {
    Clipboard,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipTypeFilter {
    All,
    Text,
    Link,
    Image,
}

pub struct FilesTabState {
    pub sub: FilesSubTab,
    pub filter: ClipTypeFilter,
    pub search_open: bool,
    pub search_anim: f32,
    pub search_input: TextInputState,
    pub last_search_q: String,
    pub search_dirty_since_ms: u64,
    pub scroll: ScrollState,
    pub file_info_cache: HashMap<String, Option<FileCardInfo>>,
    pub icon_rasters: HashMap<String, Vec<u8>>,
    pub copy_flash_idx: Option<usize>,
    pub copy_flash_until_ms: u64,
}

impl Default for FilesTabState {
    fn default() -> Self {
        Self {
            sub: FilesSubTab::Clipboard,
            filter: ClipTypeFilter::All,
            search_open: false,
            search_anim: 0.0,
            search_input: TextInputState::default(),
            last_search_q: String::new(),
            search_dirty_since_ms: 0,
            scroll: ScrollState::default(),
            file_info_cache: HashMap::new(),
            icon_rasters: HashMap::new(),
            copy_flash_idx: None,
            copy_flash_until_ms: 0,
        }
    }
}

fn file_pin_key(path: &str) -> String {
    format!("file:{}", path.to_lowercase())
}

fn clip_pin_key(c: &ClipboardEntry) -> String {
    if !c.image.is_empty() {
        format!("img:{}", c.at)
    } else {
        let head: String = c.text.chars().take(64).collect();
        format!("{}:{}", c.kind, head)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_files(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    notch_rect: Rect,
    clips: &[ClipboardEntry],
    pins: &[String],
    shelf: &[String],
    search_hits: &[FileHit],
    files_drag_over: bool,
    accent: Color,
    state: &mut FilesTabState,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    let hdr = Rect::new(notch_rect.x + 14.0, notch_rect.y + 38.0, notch_rect.w - 28.0, 26.0);

    // Mode segmented pills: Clipboard | Files
    if draw_pill_btn(
        d2d,
        hits,
        "files_tab_clip",
        Rect::new(hdr.x, hdr.y + 1.0, 76.0, 23.0),
        "Clipboard",
        state.sub == FilesSubTab::Clipboard,
        None,
        input,
    ) {
        state.sub = FilesSubTab::Clipboard;
        state.scroll.offset = 0.0;
        state.scroll.target = 0.0;
    }
    if draw_pill_btn(
        d2d,
        hits,
        "files_tab_files",
        Rect::new(hdr.x + 82.0, hdr.y + 1.0, 56.0, 23.0),
        "Files",
        state.sub == FilesSubTab::Files,
        None,
        input,
    ) {
        state.sub = FilesSubTab::Files;
        state.scroll.offset = 0.0;
        state.scroll.target = 0.0;
    }

    if state.sub == FilesSubTab::Clipboard {
        // Filter buttons: All, Text, Link, Image
        let filters = [
            (ClipTypeFilter::All, "all", IconId::Layers, "Semua"),
            (ClipTypeFilter::Text, "txt", IconId::TypeText, "Teks"),
            (ClipTypeFilter::Link, "lnk", IconId::Link, "Link"),
            (ClipTypeFilter::Image, "img", IconId::Image, "Gambar"),
        ];
        let mut fx = hdr.x + 146.0;
        for (f, k, icon, tip) in filters {
            let active = state.filter == f;
            let btn = Rect::new(fx, hdr.y + 1.0, 23.0, 22.0);
            if active {
                d2d.fill_rounded_rect(btn, 7.0, Color::white(0.14));
            }
            if draw_icon_btn(
                d2d,
                hits,
                &format!("clip_f_{}", k),
                btn,
                icon,
                12.5,
                if active { Color::white(0.95) } else { Color::white(0.45) },
                Color::white(0.08),
                Some(tip),
                input,
            ) {
                state.filter = f;
                state.scroll.target = 0.0;
            }
            fx += 25.0;
        }
    }

    // Shared Search + Trash controls on the right for BOTH Clipboard and Files
    let mut sx = hdr.right() - 24.0;
    if draw_icon_btn(
        d2d,
        hits,
        if state.sub == FilesSubTab::Clipboard { "clip_clear" } else { "files_clear" },
        Rect::new(sx, hdr.y + 1.0, 24.0, 22.0),
        IconId::Trash,
        13.0,
        Color::white(0.55),
        Color::rgba(239, 68, 68, 0.22),
        Some(if state.sub == FilesSubTab::Clipboard {
            "Hapus semua kecuali yang dipin"
        } else {
            "Kosongkan shelf file"
        }),
        input,
    ) {
        if state.sub == FilesSubTab::Clipboard {
            let kept: Vec<ClipboardEntry> = clips
                .iter()
                .filter(|c| pins.iter().any(|p| p == &clip_pin_key(c)))
                .cloned()
                .collect();
            if let Ok(v) = serde_json::to_value(kept) {
                store.set(Some(ui), "clipboardHistory", v);
            }
        } else {
            store.set(Some(ui), "shelf", serde_json::json!([]));
            store.set(Some(ui), "dnSearchResults", serde_json::json!([]));
        }
    }
    sx -= 6.0;
    let target_anim = if state.search_open { 1.0 } else { 0.0 };
    let k = (1.0 - (-16.0 * input.dt).exp()).clamp(0.12, 1.0);
    state.search_anim += (target_anim - state.search_anim) * k;
    if (target_anim - state.search_anim).abs() < 0.01 {
        state.search_anim = target_anim;
    }

    if state.search_anim > 0.04 || state.search_open {
        let cur_w = 24.0 + (182.0 - 24.0) * state.search_anim;
        let cap_r = Rect::new(sx - cur_w, hdr.y + 1.0, cur_w, 23.0);
        let focused_search = hits.focused_input.as_deref() == Some("input:files_search_inp");
        let cap_bg = Color::rgba(255, 255, 255, if focused_search { 0.09 } else { 0.06 });
        let cap_bd = if focused_search {
            Color::white(0.24)
        } else {
            Color::white(0.11)
        };
        d2d.fill_rounded_rect(cap_r, 11.5, cap_bg);
        d2d.stroke_rounded_rect(cap_r, 11.5, 1.0, cap_bd);

        d2d.push_clip_rect(cap_r);
        let icon_fg = if focused_search {
            accent
        } else {
            Color::white(0.55)
        };
        d2d.draw_icon(
            IconId::Search,
            Rect::new(cap_r.x + 7.5, cap_r.center_y() - 6.0, 12.0, 12.0),
            icon_fg,
            1.85,
            icon_fg.a,
        );

        if cur_w > 64.0 {
            let mut cap = TextInputStyle::capsule(23.0);
            cap.bg = Color::rgba(0, 0, 0, 0.0);
            cap.hover_bg = Color::rgba(0, 0, 0, 0.0);
            cap.focus_bg = Color::rgba(0, 0, 0, 0.0);
            cap.border = Color::rgba(0, 0, 0, 0.0);
            cap.focus_border = Color::rgba(0, 0, 0, 0.0);
            cap.pad_left = 26.0;
            cap.pad_right = 22.0;

            let act = state.search_input.draw_styled(
                d2d,
                hits,
                "files_search_inp",
                cap_r,
                "Cari nama...",
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
            if act.changed {
                state.search_dirty_since_ms = input.now_ms;
                state.scroll.target = 0.0;
                if state.search_input.text.trim().is_empty() {
                    state.last_search_q.clear();
                    if state.sub == FilesSubTab::Files {
                        store.set(Some(ui), "dnSearchResults", serde_json::json!([]));
                    }
                }
            }
            if state.sub == FilesSubTab::Files {
                let q = state.search_input.text.trim().to_string();
                let debounced = !q.is_empty()
                    && q.len() >= 2
                    && q != state.last_search_q
                    && state.search_dirty_since_ms > 0
                    && input.now_ms.saturating_sub(state.search_dirty_since_ms) >= 320;
                if (act.submitted && !q.is_empty()) || debounced {
                    state.last_search_q = q.clone();
                    state.search_dirty_since_ms = 0;
                    let st = store.clone();
                    let u = ui.clone();
                    rt.spawn(async move {
                        let res = system::search_files(&q, None).await;
                        if res.is_array() {
                            st.set(Some(&u), "dnSearchResults", res);
                        } else if let Some(arr) = res.get("files") {
                            st.set(Some(&u), "dnSearchResults", arr.clone());
                        }
                    });
                }
            }
            if act.escaped {
                state.search_open = false;
                hits.focused_input = None;
            }

            if draw_icon_btn(
                d2d,
                hits,
                "files_search_close",
                Rect::new(cap_r.right() - 21.0, cap_r.y + 2.0, 19.0, 19.0),
                IconId::Close,
                10.5,
                Color::white(0.55),
                Color::white(0.10),
                Some("Tutup pencarian"),
                input,
            ) {
                state.search_input.text.clear();
                state.search_input.cursor = 0;
                state.search_input.sel_anchor = None;
                state.search_input.scroll_x = 0.0;
                state.last_search_q.clear();
                state.search_dirty_since_ms = 0;
                state.search_open = false;
                state.scroll.target = 0.0;
                if state.sub == FilesSubTab::Files {
                    store.set(Some(ui), "dnSearchResults", serde_json::json!([]));
                }
                hits.focused_input = None;
            }
        }
        d2d.pop_clip_rect();
    } else if draw_icon_btn(
        d2d,
        hits,
        "files_search_btn",
        Rect::new(sx - 24.0, hdr.y + 1.0, 24.0, 22.0),
        IconId::Search,
        12.5,
        Color::white(0.55),
        Color::white(0.09),
        Some(if state.sub == FilesSubTab::Clipboard {
            "Cari di clipboard"
        } else {
            "Cari file"
        }),
        input,
    ) {
        state.search_open = true;
        hits.focused_input = Some("input:files_search_inp".to_string());
    }

    // Horizontal cards viewport
    let strip = Rect::new(notch_rect.x + 14.0, notch_rect.y + 72.0, notch_rect.w - 28.0, 98.0);
    if files_drag_over {
        d2d.fill_rounded_rect(strip, 16.0, accent.with_alpha(0.14));
        d2d.stroke_rounded_rect(strip, 16.0, 1.5, accent.with_alpha(0.75));
        d2d.draw_text(
            "Lepas file di sini untuk disimpan ke SelaBar Shelf",
            strip,
            &TextStyle::default()
                .size(12.5)
                .semibold()
                .color(Color::white(0.95))
                .center()
                .vcenter(),
        );
        return;
    }

    if state.sub == FilesSubTab::Clipboard {
        draw_clipboard_cards(d2d, hits, strip, clips, pins, accent, state, store, ui, rt, input);
    } else {
        draw_file_cards(d2d, hits, strip, shelf, pins, search_hits, state, store, ui, input);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_clipboard_cards(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    strip: Rect,
    clips: &[ClipboardEntry],
    pins: &[String],
    _accent: Color,
    state: &mut FilesTabState,
    store: &StoreManager,
    ui: &UiSender,
    rt: &tokio::runtime::Handle,
    input: &UiInput,
) {
    let q = if state.search_open {
        state.search_input.text.trim().to_lowercase()
    } else {
        String::new()
    };
    let mut items: Vec<(usize, &ClipboardEntry, bool)> = clips
        .iter()
        .enumerate()
        .filter(|(_, c)| match state.filter {
            ClipTypeFilter::All => true,
            ClipTypeFilter::Text => c.image.is_empty() && !is_link_clip(c),
            ClipTypeFilter::Link => is_link_clip(c),
            ClipTypeFilter::Image => !c.image.is_empty() || c.kind == "image",
        })
        .filter(|(_, c)| {
            if q.is_empty() {
                true
            } else if !c.image.is_empty() {
                "gambar image screenshot".contains(&q)
            } else {
                c.text.to_lowercase().contains(&q)
            }
        })
        .map(|(i, c)| {
            let p = pins.iter().any(|k| k == &clip_pin_key(c));
            (i, c, p)
        })
        .collect();

    // Pinned cards first cleanly
    items.sort_by_key(|&(_, _, pinned)| !pinned);

    if items.is_empty() {
        d2d.draw_text(
            "Riwayat clipboard kosong.\nSalin teks, link, atau gambar.",
            strip,
            &TextStyle::default()
                .size(11.5)
                .color(Color::white(0.38))
                .center()
                .vcenter(),
        );
        return;
    }

    let cw = 108.0;
    let gap = 8.0;
    let total_w = items.len() as f32 * (cw + gap) - gap;
    state.scroll.update_bounds(strip.w, total_w, input);
    state.scroll.handle_wheel(strip, true, input);

    d2d.push_clip_rect(strip);
    let mut x = strip.x - state.scroll.offset;
    for (orig_idx, entry, is_pinned) in items {
        let card = Rect::new(x, strip.y + 4.0, cw, 84.0);
        if card.right() >= strip.x && card.x <= strip.right() {
            let card_hovered = input.mouse_inside
                && strip.contains(input.mouse_x, input.mouse_y)
                && card.contains(input.mouse_x, input.mouse_y);
            let copied_here = state.copy_flash_idx == Some(orig_idx)
                && input.now_ms < state.copy_flash_until_ms;

            // Register the card body first so the pin/x/join buttons registered
            // below sit on top in the hit list (hover resolves last-registered
            // first). Registering the body last made it swallow every click.
            let ch = hits.register(
                format!("clip_card_{}", orig_idx),
                card,
                CursorKind::Pointer,
                Some("Klik untuk salin ulang".to_string()),
                input,
            );

            d2d.fill_rounded_rect(
                card,
                14.0,
                Color::white(if card_hovered { 0.085 } else { 0.052 }),
            );
            let border = if copied_here {
                Color::rgb(52, 211, 153)
            } else if is_pinned {
                Color::rgba(56, 189, 248, 0.55)
            } else {
                Color::white(if card_hovered { 0.16 } else { 0.085 })
            };
            d2d.stroke_rounded_rect(card, 14.0, 1.0, border);

            if !entry.image.is_empty() {
                d2d.draw_image(&entry.image, card.inset(4.0, 4.0), 10.0, 0.96, true);
            } else if is_link_clip(entry) {
                let meet = system::detect_meeting_link(&entry.text);
                d2d.draw_icon(
                    if meet.is_some() { IconId::Video } else { IconId::Link },
                    Rect::new(card.x + 9.0, card.y + 9.0, 12.0, 12.0),
                    if meet.is_some() { Color::rgb(59, 130, 246) } else { Color::rgb(56, 189, 248) },
                    1.9,
                    1.0,
                );
                let txt_h = if meet.is_some() { 34.0 } else { 52.0 };
                d2d.draw_text(
                    &entry.text,
                    Rect::new(card.x + 8.0, card.y + 24.0, card.w - 16.0, txt_h),
                    &TextStyle::default()
                        .size(10.0)
                        .color(Color::white(0.84))
                        .line_height(13.5),
                );
                if let Some(lk) = meet {
                    let jr = Rect::new(card.right() - 48.0, card.bottom() - 22.0, 40.0, 17.0);
                    let jh = hits.register(
                        format!("clip_join_{}", orig_idx),
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
                        let lc = lk.clone();
                        rt.spawn(async move {
                            let _ = system::join_meeting_smart(lc).await;
                        });
                        continue;
                    }
                }
            } else {
                d2d.draw_text(
                    &entry.text,
                    Rect::new(card.x + 9.0, card.y + 10.0, card.w - 18.0, 64.0),
                    &TextStyle::default()
                        .size(10.0)
                        .color(Color::white(0.84))
                        .line_height(13.5),
                );
            }

            if copied_here {
                d2d.fill_rounded_rect(
                    Rect::new(card.x + 20.0, card.bottom() - 22.0, card.w - 40.0, 16.0),
                    8.0,
                    Color::rgba(52, 211, 153, 0.92),
                );
                d2d.draw_text(
                    "Disalin",
                    Rect::new(card.x + 20.0, card.bottom() - 22.0, card.w - 40.0, 16.0),
                    &TextStyle::default()
                        .size(9.5)
                        .bold()
                        .color(Color::black(0.95))
                        .center()
                        .vcenter(),
                );
            }

            let pin_r = Rect::new(card.x + 4.0, card.y + 4.0, 18.0, 18.0);
            let del_r = Rect::new(card.right() - 22.0, card.y + 4.0, 18.0, 18.0);
            let pin_id = format!("clip_pin_{}", orig_idx);
            let del_id = format!("clip_del_{}", orig_idx);
            // Register the buttons for every visible card, not only hovered ones:
            // hover resolves against the PREVIOUS frame's hit list, so a button
            // that only registers once the mouse is already on it can never be
            // hovered on first entry.
            let pin_hit = hits.register(pin_id.clone(), pin_r, CursorKind::Pointer, None, input);
            let del_hit = hits.register(del_id.clone(), del_r, CursorKind::Pointer, None, input);
            // While a press is held keep the buttons drawn even if hover flickers.
            let pressed_here = hits.pressed_id.as_deref()
                .map(|pid| pid == pin_id || pid == del_id)
                .unwrap_or(false);
            let show_actions = card_hovered || is_pinned || pressed_here;
            if show_actions {
                draw_icon_btn(
                    d2d,
                    hits,
                    &pin_id,
                    pin_r,
                    IconId::Pin,
                    10.5,
                    if is_pinned {
                        Color::rgb(56, 189, 248)
                    } else {
                        Color::white(0.60)
                    },
                    Color::black(0.55),
                    Some(if is_pinned { "Lepas pin" } else { "Pin item" }),
                    input,
                );
                draw_icon_btn(
                    d2d,
                    hits,
                    &del_id,
                    del_r,
                    IconId::Close,
                    10.5,
                    Color::white(0.65),
                    Color::rgba(239, 68, 68, 0.70),
                    Some("Hapus"),
                    input,
                );
            }
            if pin_hit.clicked {
                let key = clip_pin_key(entry);
                let mut next_pins = pins.to_vec();
                if let Some(pos) = next_pins.iter().position(|p| p == &key) {
                    next_pins.remove(pos);
                } else {
                    next_pins.push(key);
                }
                if let Ok(v) = serde_json::to_value(next_pins) {
                    store.set(Some(ui), "pins", v);
                }
            }
            if del_hit.clicked {
                let mut next = clips.to_vec();
                next.remove(orig_idx);
                if let Ok(v) = serde_json::to_value(next) {
                    store.set(Some(ui), "clipboardHistory", v);
                }
            }
            if ch.clicked {
                copy_clip_entry(entry);
                state.copy_flash_idx = Some(orig_idx);
                state.copy_flash_until_ms = input.now_ms + 1100;
            }
        }
        x += cw + gap;
    }
    d2d.pop_clip_rect();
    state.scroll.draw_h_bar(d2d, strip, input);
}

#[allow(clippy::too_many_arguments)]
fn draw_file_cards(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    strip: Rect,
    shelf: &[String],
    pins: &[String],
    search_hits: &[FileHit],
    state: &mut FilesTabState,
    store: &StoreManager,
    ui: &UiSender,
    input: &UiInput,
) {
    let q = if state.search_open {
        state.search_input.text.trim().to_lowercase()
    } else {
        String::new()
    };
    let mut paths: Vec<(String, bool, bool)> = Vec::new();
    for p in shelf {
        if q.is_empty() || p.to_lowercase().contains(&q) {
            let pinned = pins.iter().any(|k| k == &file_pin_key(p));
            paths.push((p.clone(), true, pinned));
        }
    }
    for h in search_hits {
        if !h.path.is_empty()
            && !shelf.contains(&h.path)
            && (q.is_empty() || h.path.to_lowercase().contains(&q))
        {
            paths.push((h.path.clone(), false, false));
        }
    }
    // Pinned shelf files first, mirroring the clipboard cards.
    paths.sort_by_key(|(_, _, pinned)| !*pinned);

    if paths.is_empty() {
        let msg = if !q.is_empty() {
            "Tidak ada file yang cocok dengan pencarian."
        } else {
            "Seret file ke notch untuk disimpan di sini, atau klik ikon Cari."
        };
        d2d.draw_text(
            msg,
            strip,
            &TextStyle::default()
                .size(11.5)
                .color(Color::white(0.38))
                .center()
                .vcenter(),
        );
        return;
    }

    let cw = 108.0;
    let gap = 8.0;
    let total_w = paths.len() as f32 * (cw + gap) - gap;
    state.scroll.update_bounds(strip.w, total_w, input);
    state.scroll.handle_wheel(strip, true, input);

    d2d.push_clip_rect(strip);
    let mut x = strip.x - state.scroll.offset;
    for (idx, (path, is_shelf, is_pinned)) in paths.iter().enumerate() {
        let card = Rect::new(x, strip.y + 4.0, cw, 84.0);
        if card.right() >= strip.x && card.x <= strip.right() {
            if !state.file_info_cache.contains_key(path) {
                if state.file_info_cache.len() >= 96 {
                    state.file_info_cache.clear();
                }
                state
                    .file_info_cache
                    .insert(path.clone(), system::describe_file(path));
            }
            let info = state.file_info_cache.get(path).and_then(|o| o.as_ref());
            let name = info
                .map(|i| i.name.as_str())
                .unwrap_or_else(|| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(path)
                });
            let is_dir = info.map(|i| i.is_dir).unwrap_or(false);
            let thumb = info.and_then(|i| i.thumb.as_deref());

            let card_hovered = input.mouse_inside
                && strip.contains(input.mouse_x, input.mouse_y)
                && card.contains(input.mouse_x, input.mouse_y);

            // Register the card body first so the pin/x buttons registered below
            // sit on top in the hit list (hover resolves last-registered first).
            let ch = hits.register(
                format!("file_card_{}", idx),
                card,
                CursorKind::Pointer,
                Some(format!("{}\nKlik: buka • Klik kanan: tunjukkan di Explorer", path)),
                input,
            );

            d2d.fill_rounded_rect(
                card,
                14.0,
                Color::white(if card_hovered { 0.085 } else { 0.052 }),
            );
            d2d.stroke_rounded_rect(
                card,
                14.0,
                1.0,
                if *is_pinned {
                    Color::rgba(56, 189, 248, 0.55)
                } else {
                    Color::white(if card_hovered { 0.16 } else { 0.085 })
                },
            );

            if let Some(t) = thumb {
                d2d.draw_image(
                    t,
                    Rect::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 48.0),
                    8.0,
                    0.96,
                    true,
                );
            } else {
                let ext = std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                draw_file_type_logo(d2d, &mut state.icon_rasters, card, ext, is_dir);
            }

            d2d.draw_text(
                name,
                Rect::new(card.x + 8.0, card.bottom() - 22.0, card.w - 16.0, 16.0),
                &TextStyle::default()
                    .size(10.0)
                    .medium()
                    .color(Color::white(0.86))
                    .center()
                    .ellipsis(),
            );

            let pin_r = Rect::new(card.x + 4.0, card.y + 4.0, 18.0, 18.0);
            let del_r = Rect::new(card.right() - 22.0, card.y + 4.0, 18.0, 18.0);
            let pin_id = format!("file_pin_{}", idx);
            let del_id = format!("file_del_{}", idx);
            // Buttons exist only for shelf cards; register them whenever the card
            // is visible (hover resolves against the previous frame's hit list,
            // so conditional registration would block the first click).
            let (pin_hit, del_hit) = if *is_shelf {
                (
                    hits.register(pin_id.clone(), pin_r, CursorKind::Pointer, None, input),
                    hits.register(del_id.clone(), del_r, CursorKind::Pointer, None, input),
                )
            } else {
                (
                    crate::ui::widgets::HitResult::default(),
                    crate::ui::widgets::HitResult::default(),
                )
            };
            let pressed_here = hits.pressed_id.as_deref()
                .map(|pid| pid == pin_id || pid == del_id)
                .unwrap_or(false);
            let show_actions = *is_shelf && (card_hovered || *is_pinned || pressed_here);
            if show_actions {
                draw_icon_btn(
                    d2d,
                    hits,
                    &pin_id,
                    pin_r,
                    IconId::Pin,
                    10.5,
                    if *is_pinned {
                        Color::rgb(56, 189, 248)
                    } else {
                        Color::white(0.60)
                    },
                    Color::black(0.55),
                    Some(if *is_pinned { "Lepas pin" } else { "Pin item" }),
                    input,
                );
                draw_icon_btn(
                    d2d,
                    hits,
                    &del_id,
                    del_r,
                    IconId::Close,
                    10.5,
                    Color::white(0.65),
                    Color::rgba(239, 68, 68, 0.70),
                    Some("Keluarkan dari shelf"),
                    input,
                );
            }
            if pin_hit.clicked {
                let key = file_pin_key(path);
                let mut next_pins = pins.to_vec();
                if let Some(pos) = next_pins.iter().position(|p| p == &key) {
                    next_pins.remove(pos);
                } else {
                    next_pins.push(key);
                }
                if let Ok(v) = serde_json::to_value(next_pins) {
                    store.set(Some(ui), "pins", v);
                }
            }
            if del_hit.clicked {
                let next: Vec<String> = shelf.iter().filter(|p| p.as_str() != path.as_str()).cloned().collect();
                if let Ok(v) = serde_json::to_value(next) {
                    store.set(Some(ui), "shelf", v);
                }
            }
            if ch.clicked {
                system::open_path_async(path);
            } else if ch.right_clicked {
                system::reveal_path_async(path);
            }
        }
        x += cw + gap;
    }
    d2d.pop_clip_rect();
    state.scroll.draw_h_bar(d2d, strip, input);
}
