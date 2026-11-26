use crate::backend::clipboard;
use crate::ui::d2d::{Color, D2dEngine, Rect, TextAlign, TextStyle};
use crate::ui::icons::IconId;
use std::collections::HashMap;

pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_DELETE: u32 = 0x2E;

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub vk: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Arrow,
    Pointer,
    Text,
}

#[derive(Debug, Clone)]
pub struct UiInput {
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub mouse_inside: bool,
    pub left_down: bool,
    pub left_pressed: bool,
    pub left_released: bool,
    pub right_pressed: bool,
    pub wheel_y: f32,
    pub wheel_x: f32,
    pub keys: Vec<KeyEvent>,
    pub chars: Vec<char>,
    pub now_ms: u64,
    pub dt: f32,
}

impl Default for UiInput {
    fn default() -> Self {
        Self {
            mouse_x: -1000.0,
            mouse_y: -1000.0,
            mouse_inside: false,
            left_down: false,
            left_pressed: false,
            left_released: false,
            right_pressed: false,
            wheel_y: 0.0,
            wheel_x: 0.0,
            keys: Vec::new(),
            chars: Vec::new(),
            now_ms: 0,
            dt: 0.016,
        }
    }
}

#[derive(Debug, Clone)]
struct HitEntry {
    id: String,
    rect: Rect,
    cursor: CursorKind,
    tooltip: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HitResult {
    pub hovered: bool,
    pub clicked: bool,
    pub right_clicked: bool,
    pub pressed: bool,
    pub down: bool,
}

pub struct HitRegistry {
    prev_hits: Vec<HitEntry>,
    curr_hits: Vec<HitEntry>,
    shell_rects: Vec<Rect>,
    clip_stack: Vec<Rect>,
    switch_anim: std::collections::HashMap<String, f32>,
    pub hovered_id: Option<String>,
    pub pressed_id: Option<String>,
    pub focused_input: Option<String>,
    pub prev_focused_input: Option<String>,
    hover_start_ms: u64,
    last_hovered_id: Option<String>,
}

impl HitRegistry {
    pub fn new() -> Self {
        Self {
            prev_hits: Vec::new(),
            curr_hits: Vec::new(),
            shell_rects: Vec::new(),
            clip_stack: Vec::new(),
            switch_anim: std::collections::HashMap::new(),
            hovered_id: None,
            pressed_id: None,
            focused_input: None,
            prev_focused_input: None,
            hover_start_ms: 0,
            last_hovered_id: None,
        }
    }

    pub fn begin(&mut self, input: &UiInput) {
        self.curr_hits.clear();
        self.shell_rects.clear();
        self.clip_stack.clear();
        self.prev_focused_input = self.focused_input.clone();

        self.hovered_id = if input.left_down && self.pressed_id.is_some() {
            // Keep hover frozen on the pressed control while the button is
            // held: conditional buttons (card pin/x) must not lose their
            // hover mid-click when a re-render shifts layout under them.
            self.pressed_id.clone()
        } else if input.mouse_inside {
            self.prev_hits
                .iter()
                .rev()
                .find(|h| h.rect.contains(input.mouse_x, input.mouse_y))
                .map(|h| h.id.clone())
        } else {
            None
        };

        if self.hovered_id != self.last_hovered_id {
            self.last_hovered_id = self.hovered_id.clone();
            self.hover_start_ms = input.now_ms;
        }

        if input.left_pressed {
            self.pressed_id = self.hovered_id.clone();
            // Clear focused text input when clicking outside any text input; text input will re-claim focus if clicked.
            let clicked_is_input = self
                .hovered_id
                .as_deref()
                .map(|id| id.starts_with("input:"))
                .unwrap_or(false);
            if !clicked_is_input {
                self.focused_input = None;
            }
        }
    }

    pub fn end(&mut self, input: &UiInput) {
        if input.left_released {
            self.pressed_id = None;
        }
        std::mem::swap(&mut self.prev_hits, &mut self.curr_hits);
    }

    pub fn focus_changed(&self) -> (bool, bool) {
        let was = self.prev_focused_input.is_some();
        let now = self.focused_input.is_some();
        (was, now)
    }

    pub fn add_shell_rect(&mut self, r: Rect) {
        if r.w > 0.0 && r.h > 0.0 {
            self.shell_rects.push(r);
        }
    }

    pub fn shell_rects(&self) -> &[Rect] {
        &self.shell_rects
    }

    pub fn push_clip(&mut self, r: Rect) {
        let clipped = if let Some(parent) = self.clip_stack.last() {
            parent.intersect(r)
        } else {
            r
        };
        self.clip_stack.push(clipped);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    pub fn get_switch_pos(&mut self, id: &str, checked: bool) -> f32 {
        let target = if checked { 1.0f32 } else { 0.0f32 };
        let entry = self.switch_anim.entry(id.to_string()).or_insert(target);
        *entry += (target - *entry) * 0.32;
        if (*entry - target).abs() < 0.01 {
            *entry = target;
        }
        *entry
    }

    pub fn any_switch_animating(&self) -> bool {
        self.switch_anim.values().any(|&p| p > 0.01 && p < 0.99)
    }

    pub fn register(
        &mut self,
        id: impl Into<String>,
        rect: Rect,
        cursor: CursorKind,
        tooltip: Option<String>,
        input: &UiInput,
    ) -> HitResult {
        let id = id.into();
        let eff_rect = if let Some(c) = self.clip_stack.last() {
            rect.intersect(*c)
        } else {
            rect
        };
        if eff_rect.w <= 0.5 || eff_rect.h <= 0.5 {
            return HitResult::default();
        }
        let hovered = self.hovered_id.as_deref() == Some(id.as_str())
            || (self.hovered_id.is_none()
                && input.mouse_inside
                && eff_rect.contains(input.mouse_x, input.mouse_y));
        let down = self.pressed_id.as_deref() == Some(id.as_str()) && input.left_down;
        let pressed = hovered && input.left_pressed;
        let clicked = hovered && input.left_released && self.pressed_id.as_deref() == Some(id.as_str());
        let right_clicked = hovered && input.right_pressed;

        self.curr_hits.push(HitEntry {
            id,
            rect: eff_rect,
            cursor,
            tooltip,
        });

        HitResult {
            hovered,
            clicked,
            right_clicked,
            pressed,
            down,
        }
    }

    pub fn active_cursor(&self) -> CursorKind {
        if let Some(id) = self.hovered_id.as_deref() {
            if let Some(h) = self.prev_hits.iter().rev().find(|h| h.id == id) {
                return h.cursor;
            }
        }
        CursorKind::Arrow
    }

    pub fn active_tooltip(&self, now_ms: u64) -> Option<(String, Rect)> {
        if now_ms.saturating_sub(self.hover_start_ms) < 480 {
            return None;
        }
        let id = self.hovered_id.as_deref()?;
        let entry = self.prev_hits.iter().rev().find(|h| h.id == id)?;
        entry.tooltip.clone().map(|t| (t, entry.rect))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextInputState {
    pub text: String,
    pub cursor: usize,
    pub sel_anchor: Option<usize>,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub multiline: bool,
    pub rich_paste: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TextInputAction {
    pub changed: bool,
    pub submitted: bool,
    pub escaped: bool,
    pub pasted_attachments: Vec<clipboard::PastedAttachment>,
}

#[derive(Debug, Clone, Copy)]
pub struct TextInputStyle {
    pub bg: Color,
    pub hover_bg: Color,
    pub focus_bg: Color,
    pub border: Color,
    pub focus_border: Color,
    pub radius: f32,
    pub pad_left: f32,
    pub pad_right: f32,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            bg: Color::white(0.048),
            hover_bg: Color::white(0.065),
            focus_bg: Color::white(0.085),
            border: Color::white(0.10),
            focus_border: Color::white(0.24),
            radius: 9.0,
            pad_left: 10.0,
            pad_right: 10.0,
        }
    }
}

impl TextInputStyle {
    pub fn capsule(height: f32) -> Self {
        Self {
            bg: Color::white(0.045),
            hover_bg: Color::white(0.068),
            focus_bg: Color::white(0.082),
            border: Color::white(0.10),
            focus_border: Color::white(0.24),
            radius: height * 0.5,
            pad_left: 14.0,
            pad_right: 14.0,
        }
    }

    pub fn borderless() -> Self {
        Self {
            bg: Color::white(0.0),
            hover_bg: Color::white(0.0),
            focus_bg: Color::white(0.0),
            border: Color::white(0.0),
            focus_border: Color::white(0.0),
            radius: 0.0,
            pad_left: 0.0,
            pad_right: 4.0,
        }
    }
}

impl TextInputState {
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.sel_anchor = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.sel_anchor = None;
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
    }

    pub fn measure_composer_height(
        &self,
        d2d: &mut D2dEngine,
        total_w: f32,
        pad_left: f32,
        pad_right: f32,
        style: &TextStyle,
        min_h: f32,
        max_h: f32,
    ) -> f32 {
        if !self.multiline || self.text.is_empty() {
            return min_h;
        }
        let inner_w = (total_w - pad_left - pad_right).max(40.0);
        let mut wrap_style = style.clone();
        wrap_style.nowrap = false;
        wrap_style.valign = crate::ui::d2d::TextVAlign::Top;
        if wrap_style.line_height.is_none() {
            wrap_style.line_height = Some(16.5);
        }
        let measured_str = if self.text.ends_with('\n') {
            format!("{} ", self.text)
        } else {
            self.text.clone()
        };
        let (_, th) = d2d.measure_text(&measured_str, inner_w, &wrap_style);
        (th + 14.0).clamp(min_h, max_h)
    }

    fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.sel_anchor?;
        let c = self.cursor.min(self.text.len());
        let a = a.min(self.text.len());
        if a == c {
            None
        } else if a < c {
            Some((a, c))
        } else {
            Some((c, a))
        }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.selected_range() {
            self.text.replace_range(start..end, "");
            self.cursor = start;
            self.sel_anchor = None;
            true
        } else {
            false
        }
    }

    fn prev_char_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        if p == 0 {
            return 0;
        }
        p -= 1;
        while p > 0 && !self.text.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    fn next_char_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        if p >= self.text.len() {
            return self.text.len();
        }
        p += 1;
        while p < self.text.len() && !self.text.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    pub fn draw(
        &mut self,
        d2d: &mut D2dEngine,
        hits: &mut HitRegistry,
        id: &str,
        rect: Rect,
        placeholder: &str,
        masked: bool,
        style_opt: Option<TextStyle>,
        input: &UiInput,
    ) -> TextInputAction {
        let mut box_style = TextInputStyle::default();
        box_style.radius = (rect.h * 0.34).min(12.0);
        self.draw_styled(d2d, hits, id, rect, placeholder, masked, style_opt, &box_style, input)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_styled(
        &mut self,
        d2d: &mut D2dEngine,
        hits: &mut HitRegistry,
        id: &str,
        rect: Rect,
        placeholder: &str,
        masked: bool,
        style_opt: Option<TextStyle>,
        box_style: &TextInputStyle,
        input: &UiInput,
    ) -> TextInputAction {
        let full_id = if id.starts_with("input:") {
            id.to_string()
        } else {
            format!("input:{}", id)
        };
        let hit = hits.register(full_id.clone(), rect, CursorKind::Text, None, input);

        if hit.pressed {
            hits.focused_input = Some(full_id.clone());
        }
        self.focused = hits.focused_input.as_deref() == Some(full_id.as_str());

        let base_style = style_opt.unwrap_or_else(|| {
            TextStyle::default()
                .size(11.5)
                .color(Color::white(0.92))
                .vcenter()
                .nowrap()
        });
        let is_centered = base_style.align == TextAlign::Center;
        let is_multiline = self.multiline && !masked && !is_centered;

        let inner = if is_multiline {
            Rect::new(
                rect.x + box_style.pad_left,
                rect.y + 6.0,
                (rect.w - box_style.pad_left - box_style.pad_right).max(8.0),
                (rect.h - 12.0).max(16.0),
            )
        } else {
            Rect::new(
                rect.x + box_style.pad_left,
                rect.y,
                (rect.w - box_style.pad_left - box_style.pad_right).max(4.0),
                rect.h,
            )
        };
        let layout_w = if is_centered || is_multiline { inner.w } else { 4096.0 };

        let mut wrap_style = base_style.clone();
        if is_multiline {
            wrap_style.nowrap = false;
            wrap_style.valign = crate::ui::d2d::TextVAlign::Top;
            if wrap_style.line_height.is_none() {
                wrap_style.line_height = Some(16.5);
            }
        }

        let display_str: String = if masked && !self.text.is_empty() {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        };

        let pre_top_pad = if is_multiline {
            let probe = if display_str.ends_with('\n') {
                format!("{} ", display_str)
            } else {
                display_str.clone()
            };
            let (_, ch) = if probe.is_empty() {
                (0.0, 0.0)
            } else {
                d2d.measure_text(&probe, inner.w, &wrap_style)
            };
            if !display_str.contains('\n') && ch > 0.0 && ch <= inner.h - 2.0 {
                ((inner.h - ch) * 0.5).max(0.0)
            } else {
                0.0
            }
        } else {
            0.0
        };

        if hit.pressed && !self.text.is_empty() && !masked {
            let idx = if is_multiline {
                let local_x = (input.mouse_x - inner.x).max(0.0);
                let local_y = (input.mouse_y - inner.y + self.scroll_y - pre_top_pad).max(0.0);
                d2d.hit_test_point(&display_str, inner.w, &wrap_style, local_x, local_y)
            } else {
                let local_x = (input.mouse_x - inner.x + self.scroll_x).max(0.0);
                d2d.hit_test_point(&display_str, layout_w, &base_style, local_x, rect.h * 0.5)
            };
            if input.keys.iter().any(|k| k.shift) {
                if self.sel_anchor.is_none() {
                    self.sel_anchor = Some(self.cursor);
                }
            } else {
                self.sel_anchor = None;
            }
            self.cursor = idx.min(self.text.len());
        } else if hit.pressed && masked {
            self.cursor = self.text.len();
            self.sel_anchor = None;
        }

        let mut act = TextInputAction::default();

        if self.focused {
            for k in &input.keys {
                match k.vk {
                    VK_RETURN => {
                        if is_multiline && k.shift {
                            self.delete_selection();
                            let c = self.cursor.min(self.text.len());
                            self.text.insert(c, '\n');
                            self.cursor = c + 1;
                            act.changed = true;
                        } else {
                            act.submitted = true;
                        }
                    }
                    VK_ESCAPE => {
                        act.escaped = true;
                        hits.focused_input = None;
                        self.focused = false;
                    }
                    VK_BACK => {
                        if !self.delete_selection() && self.cursor > 0 {
                            let prev = self.prev_char_boundary(self.cursor);
                            self.text.replace_range(prev..self.cursor, "");
                            self.cursor = prev;
                        }
                        act.changed = true;
                    }
                    VK_DELETE => {
                        if !self.delete_selection() && self.cursor < self.text.len() {
                            let next = self.next_char_boundary(self.cursor);
                            self.text.replace_range(self.cursor..next, "");
                        }
                        act.changed = true;
                    }
                    VK_LEFT => {
                        let next_c = if k.ctrl {
                            let trimmed = self.text[..self.cursor].trim_end();
                            trimmed
                                .rfind(|c: char| c.is_whitespace())
                                .map(|i| i + 1)
                                .unwrap_or(0)
                        } else {
                            self.prev_char_boundary(self.cursor)
                        };
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        self.cursor = next_c;
                    }
                    VK_RIGHT => {
                        let next_c = if k.ctrl {
                            let rem = &self.text[self.cursor..];
                            let skip_ws = rem.len() - rem.trim_start().len();
                            let after_ws = &rem[skip_ws..];
                            let word_len = after_ws
                                .find(|c: char| c.is_whitespace())
                                .unwrap_or(after_ws.len());
                            self.cursor + skip_ws + word_len
                        } else {
                            self.next_char_boundary(self.cursor)
                        };
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        self.cursor = next_c;
                    }
                    VK_UP if is_multiline => {
                        let (cx, cy, lh) = d2d.caret_metrics(&self.text, inner.w, &wrap_style, self.cursor);
                        let next_c = if cy <= lh * 0.5 {
                            0
                        } else {
                            d2d.hit_test_point(
                                &self.text,
                                inner.w,
                                &wrap_style,
                                cx,
                                (cy - lh * 0.55).max(0.0),
                            )
                        };
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        self.cursor = next_c.min(self.text.len());
                    }
                    VK_DOWN if is_multiline => {
                        let (cx, cy, lh) = d2d.caret_metrics(&self.text, inner.w, &wrap_style, self.cursor);
                        let next_c = d2d.hit_test_point(
                            &self.text,
                            inner.w,
                            &wrap_style,
                            cx,
                            cy + lh * 1.45,
                        );
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        self.cursor = next_c.min(self.text.len());
                    }
                    VK_HOME => {
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        if is_multiline && !k.ctrl {
                            let c = self.cursor.min(self.text.len());
                            self.cursor = self.text[..c].rfind('\n').map(|i| i + 1).unwrap_or(0);
                        } else {
                            self.cursor = 0;
                        }
                    }
                    VK_END => {
                        if k.shift {
                            if self.sel_anchor.is_none() {
                                self.sel_anchor = Some(self.cursor);
                            }
                        } else {
                            self.sel_anchor = None;
                        }
                        if is_multiline && !k.ctrl {
                            let c = self.cursor.min(self.text.len());
                            self.cursor = self.text[c..]
                                .find('\n')
                                .map(|i| c + i)
                                .unwrap_or(self.text.len());
                        } else {
                            self.cursor = self.text.len();
                        }
                    }
                    0x41 if k.ctrl => {
                        // Ctrl+A
                        self.sel_anchor = Some(0);
                        self.cursor = self.text.len();
                    }
                    0x43 if k.ctrl => {
                        // Ctrl+C
                        if let Some((a, b)) = self.selected_range() {
                            if !masked {
                                let _ = clipboard::copy_text(&self.text[a..b]);
                            }
                        }
                    }
                    0x58 if k.ctrl => {
                        // Ctrl+X
                        if let Some((a, b)) = self.selected_range() {
                            if !masked {
                                let _ = clipboard::copy_text(&self.text[a..b]);
                            }
                            self.delete_selection();
                            act.changed = true;
                        }
                    }
                    0x56 if k.ctrl => {
                        // Ctrl+V
                        if self.rich_paste {
                            if let Some(atts) = clipboard::read_clipboard_attachments() {
                                if !atts.is_empty() {
                                    act.pasted_attachments = atts;
                                    continue;
                                }
                            }
                        }
                        if let Some(clip) = clipboard::read_clipboard_text() {
                            let inserted = if is_multiline {
                                clip.replace("\r\n", "\n").replace('\r', "\n")
                            } else {
                                clip.replace(['\r', '\n'], " ")
                            };
                            if !inserted.is_empty() {
                                self.delete_selection();
                                let c = self.cursor.min(self.text.len());
                                self.text.insert_str(c, &inserted);
                                self.cursor = c + inserted.len();
                                act.changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }

            for &ch in &input.chars {
                if !ch.is_control() {
                    self.delete_selection();
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    let c = self.cursor.min(self.text.len());
                    self.text.insert_str(c, s);
                    self.cursor = c + s.len();
                    act.changed = true;
                }
            }
        }

        // Visual shell: subtle recessed dark pill or borderless input
        let bg = if self.focused {
            box_style.focus_bg
        } else if hit.hovered {
            box_style.hover_bg
        } else {
            box_style.bg
        };
        let border = if self.focused {
            box_style.focus_border
        } else {
            box_style.border
        };
        if bg.a > 0.001 {
            d2d.fill_rounded_rect(rect, box_style.radius, bg);
        }
        if border.a > 0.001 {
            d2d.stroke_rounded_rect(rect, box_style.radius, 1.0, border);
        }

        let updated_display: String = if masked && !self.text.is_empty() {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        };

        if is_multiline {
            self.scroll_x = 0.0;
            let probe = if updated_display.ends_with('\n') {
                format!("{} ", updated_display)
            } else {
                updated_display.clone()
            };
            let (_, content_h) = if probe.is_empty() {
                (0.0, 0.0)
            } else {
                d2d.measure_text(&probe, inner.w, &wrap_style)
            };
            let top_pad = if !updated_display.contains('\n') && content_h > 0.0 && content_h <= inner.h - 2.0 {
                ((inner.h - content_h) * 0.5).max(0.0)
            } else {
                0.0
            };
            let max_scroll_y = (content_h - inner.h).max(0.0);
            if rect.contains(input.mouse_x, input.mouse_y) && input.wheel_y.abs() > 0.01 {
                self.scroll_y = (self.scroll_y - input.wheel_y * 24.0).clamp(0.0, max_scroll_y);
            }
            let c_byte = self.cursor.min(updated_display.len());
            let (cx_rel, cy_rel, lh) =
                d2d.caret_metrics(&updated_display, inner.w, &wrap_style, c_byte);
            if act.changed || !input.keys.is_empty() {
                if cy_rel + lh - self.scroll_y > inner.h {
                    self.scroll_y = cy_rel + lh - inner.h;
                } else if cy_rel - self.scroll_y < 0.0 {
                    self.scroll_y = cy_rel.max(0.0);
                }
            }
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);

            d2d.push_clip_rect(inner);
            if updated_display.is_empty() {
                let mut ph_style = wrap_style.clone().color(Color::white(0.32));
                ph_style.valign = crate::ui::d2d::TextVAlign::Center;
                ph_style.nowrap = true;
                ph_style.ellipsis = true;
                d2d.draw_text(placeholder, inner, &ph_style);
            } else {
                if let Some((sa, sb)) = self.selected_range() {
                    let (x0, y0, lh0) = d2d.caret_metrics(&updated_display, inner.w, &wrap_style, sa);
                    let (x1, y1, _) = d2d.caret_metrics(&updated_display, inner.w, &wrap_style, sb);
                    let sel_col = Color::rgba(56, 189, 248, 0.30);
                    if (y1 - y0).abs() < 2.0 {
                        let sx = inner.x + x0.min(x1);
                        let sw = (x1 - x0).abs().max(2.0);
                        d2d.fill_rounded_rect(
                            Rect::new(sx, inner.y + top_pad + y0 - self.scroll_y, sw, lh0),
                            3.0,
                            sel_col,
                        );
                    } else {
                        let (sx_top, y_top, sx_bot, y_bot) = if y0 <= y1 {
                            (x0, y0, x1, y1)
                        } else {
                            (x1, y1, x0, y0)
                        };
                        d2d.fill_rounded_rect(
                            Rect::new(
                                inner.x + sx_top,
                                inner.y + top_pad + y_top - self.scroll_y,
                                (inner.w - sx_top).max(4.0),
                                lh0,
                            ),
                            3.0,
                            sel_col,
                        );
                        let mid_h = (y_bot - (y_top + lh0)).max(0.0);
                        if mid_h > 1.0 {
                            d2d.fill_rect(
                                Rect::new(
                                    inner.x,
                                    inner.y + top_pad + y_top + lh0 - self.scroll_y,
                                    inner.w,
                                    mid_h,
                                ),
                                sel_col,
                            );
                        }
                        d2d.fill_rounded_rect(
                            Rect::new(
                                inner.x,
                                inner.y + top_pad + y_bot - self.scroll_y,
                                sx_bot.max(4.0),
                                lh0,
                            ),
                            3.0,
                            sel_col,
                        );
                    }
                }
                let draw_rect = Rect::new(
                    inner.x,
                    inner.y + top_pad - self.scroll_y,
                    inner.w,
                    (content_h + 24.0).max(inner.h),
                );
                d2d.draw_text(&updated_display, draw_rect, &wrap_style);
            }

            if self.focused && (input.now_ms / 520) % 2 == 0 {
                let caret_x = (inner.x + cx_rel).min(inner.right() - 1.5);
                let (caret_y, caret_h) = if updated_display.is_empty() {
                    let h = (base_style.size * 1.3).min(inner.h - 2.0).max(12.0);
                    (inner.center_y() - h * 0.5, h)
                } else {
                    let h = lh.min(inner.h).max(12.0);
                    (inner.y + top_pad + cy_rel - self.scroll_y + 1.0, (h - 2.0).max(10.0))
                };
                d2d.draw_line(
                    caret_x,
                    caret_y,
                    caret_x,
                    caret_y + caret_h,
                    1.3,
                    Color::white(0.92),
                );
            }
            d2d.pop_clip_rect();
            return act;
        }

        let visual_cursor_byte = if masked {
            let char_idx = self.text[..self.cursor.min(self.text.len())].chars().count();
            char_idx * "•".len()
        } else {
            self.cursor.min(updated_display.len())
        };
        let cx_rel = if updated_display.is_empty() {
            if is_centered {
                inner.w * 0.5
            } else {
                0.0
            }
        } else {
            d2d.caret_x(&updated_display, layout_w, &base_style, visual_cursor_byte)
        };
        if is_centered {
            self.scroll_x = 0.0;
        } else {
            if cx_rel - self.scroll_x > inner.w - 4.0 {
                self.scroll_x = cx_rel - (inner.w - 4.0);
            } else if cx_rel - self.scroll_x < 0.0 {
                self.scroll_x = cx_rel.max(0.0);
            }
            let total_w = if updated_display.is_empty() {
                0.0
            } else {
                d2d.caret_x(&updated_display, layout_w, &base_style, updated_display.len())
            };
            let max_scroll = (total_w - (inner.w - 4.0)).max(0.0);
            self.scroll_x = self.scroll_x.clamp(0.0, max_scroll);
        }

        d2d.push_clip_rect(inner);
        if updated_display.is_empty() {
            let ph_style = base_style.clone().color(Color::white(0.32));
            d2d.draw_text(placeholder, inner, &ph_style);
        } else {
            if let Some((sa, sb)) = self.selected_range() {
                if !masked {
                    let x0 = d2d.caret_x(&updated_display, layout_w, &base_style, sa) - self.scroll_x;
                    let x1 = d2d.caret_x(&updated_display, layout_w, &base_style, sb) - self.scroll_x;
                    let sx = inner.x + x0.min(x1);
                    let sw = (x1 - x0).abs().max(2.0);
                    d2d.fill_rounded_rect(
                        Rect::new(sx, inner.y + 4.0, sw, inner.h - 8.0),
                        3.0,
                        Color::rgba(56, 189, 248, 0.30),
                    );
                }
            }
            let draw_rect = if is_centered {
                inner
            } else {
                Rect::new(
                    inner.x - self.scroll_x,
                    inner.y,
                    (inner.w + self.scroll_x + 400.0).max(400.0),
                    inner.h,
                )
            };
            d2d.draw_text(&updated_display, draw_rect, &base_style);
        }

        if self.focused && (input.now_ms / 520) % 2 == 0 {
            let caret_x = inner.x + cx_rel - self.scroll_x;
            let caret_h = (base_style.size * 1.3).min(inner.h - 6.0);
            let caret_y = inner.center_y() - caret_h * 0.5;
            d2d.draw_line(
                caret_x,
                caret_y,
                caret_x,
                caret_y + caret_h,
                1.3,
                Color::white(0.92),
            );
        }
        d2d.pop_clip_rect();

        act
    }
}

pub fn append_unique_attachments(
    dst: &mut Vec<clipboard::PastedAttachment>,
    incoming: Vec<clipboard::PastedAttachment>,
    max_count: usize,
) {
    for item in incoming {
        if dst.len() >= max_count {
            break;
        }
        let dup = dst
            .iter()
            .any(|existing| existing.path.eq_ignore_ascii_case(&item.path));
        if !dup {
            dst.push(item);
        }
    }
}

fn format_bytes_short(b: u64) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    }
}

fn attachment_chip_theme(att: &clipboard::PastedAttachment) -> (Color, String) {
    if att.kind == clipboard::AttachmentKind::Image {
        return (Color::rgb(167, 139, 250), "IMG".to_string());
    }
    let ext = std::path::Path::new(&att.name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => (Color::rgb(251, 146, 60), "PDF".to_string()),
        "doc" | "docx" => (Color::rgb(56, 189, 248), "DOC".to_string()),
        "xls" | "xlsx" | "csv" => (
            Color::rgb(52, 211, 153),
            if ext == "csv" { "CSV" } else { "XLS" }.to_string(),
        ),
        "ppt" | "pptx" => (Color::rgb(244, 114, 182), "PPT".to_string()),
        "" => (Color::rgb(56, 189, 248), "FILE".to_string()),
        other => {
            let tag: String = other.chars().take(4).collect::<String>().to_ascii_uppercase();
            (Color::rgb(56, 189, 248), tag)
        }
    }
}

fn truncate_filename_mid(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        return name.to_string();
    }
    let keep_head = max_chars.saturating_sub(6).max(6);
    let head: String = chars[..keep_head].iter().collect();
    let tail: String = chars[chars.len() - 5..].iter().collect();
    format!("{}…{}", head, tail)
}

/// Draws harness-TUI-style colored filename attachment pills without showing image thumbnails.
pub fn draw_attachment_chips(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id_prefix: &str,
    rect: Rect,
    attachments: &mut Vec<clipboard::PastedAttachment>,
    input: &UiInput,
) {
    if attachments.is_empty() {
        return;
    }
    let mut cx = rect.x;
    let mut remove_idx: Option<usize> = None;
    let mut shown = 0usize;

    let badge_st = TextStyle::default().size(8.5).bold().center().vcenter();
    let name_st = TextStyle::default()
        .size(10.5)
        .medium()
        .color(Color::white(0.92))
        .vcenter()
        .nowrap();

    for (idx, att) in attachments.iter().enumerate() {
        let (accent, tag) = attachment_chip_theme(att);
        let short_name = truncate_filename_mid(&att.name, 20);
        let (tag_w, _) = d2d.measure_text(&tag, 48.0, &badge_st);
        let badge_w = (tag_w + 8.0).max(24.0);
        let (name_w, _) = d2d.measure_text(&short_name, 140.0, &name_st);
        let chip_w = (8.0 + badge_w + 5.0 + name_w.min(130.0) + 18.0).min(rect.w);

        if idx > 0 && cx + chip_w + 30.0 > rect.right() {
            let rem = attachments.len() - shown;
            let rem_lbl = format!("+{}", rem);
            let rem_r = Rect::new(cx, rect.y + 1.0, 26.0, rect.h - 2.0);
            d2d.fill_rounded_rect(rem_r, (rem_r.h * 0.5).min(9.0), Color::white(0.08));
            d2d.draw_text(
                &rem_lbl,
                rem_r,
                &TextStyle::default()
                    .size(9.5)
                    .semibold()
                    .color(Color::white(0.68))
                    .center()
                    .vcenter(),
            );
            break;
        }

        shown += 1;
        let chip_r = Rect::new(cx, rect.y, chip_w, rect.h);
        let tip = format!("{} ({})", att.path, format_bytes_short(att.size_bytes));
        let ch = hits.register(
            format!("{}_chip_{}", id_prefix, idx),
            chip_r,
            CursorKind::Arrow,
            Some(tip),
            input,
        );

        let bg = accent.with_alpha(if ch.hovered { 0.22 } else { 0.14 });
        let border = accent.with_alpha(if ch.hovered { 0.65 } else { 0.45 });
        let radius = (chip_r.h * 0.5).min(10.0);
        d2d.fill_rounded_rect(chip_r, radius, bg);
        d2d.stroke_rounded_rect(chip_r, radius, 1.0, border);

        let badge_r = Rect::new(chip_r.x + 5.0, chip_r.y + 3.0, badge_w, chip_r.h - 6.0);
        d2d.fill_rounded_rect(badge_r, (badge_r.h * 0.5).min(6.0), accent.with_alpha(0.28));
        d2d.draw_text(&tag, badge_r, &badge_st.clone().color(accent));

        let name_r = Rect::new(
            badge_r.right() + 5.0,
            chip_r.y,
            (chip_r.right() - 18.0 - (badge_r.right() + 5.0)).max(20.0),
            chip_r.h,
        );
        d2d.draw_text(&short_name, name_r, &name_st);

        let close_r = Rect::new(chip_r.right() - 17.0, chip_r.center_y() - 7.0, 14.0, 14.0);
        let xh = hits.register(
            format!("{}_rm_{}", id_prefix, idx),
            close_r,
            CursorKind::Pointer,
            Some("Hapus lampiran".to_string()),
            input,
        );
        if xh.hovered {
            d2d.fill_circle(close_r.center_x(), close_r.center_y(), 6.0, Color::white(0.16));
        }
        d2d.draw_icon(
            IconId::Close,
            Rect::new(close_r.center_x() - 3.5, close_r.center_y() - 3.5, 7.0, 7.0),
            Color::white(if xh.hovered { 0.96 } else { 0.65 }),
            1.6,
            1.0,
        );
        if xh.clicked {
            remove_idx = Some(idx);
        }

        cx += chip_w + 6.0;
    }

    if let Some(i) = remove_idx {
        if i < attachments.len() {
            attachments.remove(i);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    pub offset: f32,
    pub target: f32,
    pub content: f32,
    pub viewport: f32,
    pub last_scroll_ms: u64,
}

impl ScrollState {
    pub fn update_bounds(&mut self, viewport: f32, content: f32, input: &UiInput) {
        self.viewport = viewport.max(1.0);
        self.content = content.max(0.0);
        let max_off = (self.content - self.viewport).max(0.0);
        self.target = self.target.clamp(0.0, max_off);
        self.offset += (self.target - self.offset) * (1.0 - (-18.0 * input.dt).exp()).clamp(0.15, 1.0);
        self.offset = self.offset.clamp(0.0, max_off);
    }

    pub fn handle_wheel(&mut self, area: Rect, horizontal: bool, input: &UiInput) -> bool {
        if !input.mouse_inside || !area.contains(input.mouse_x, input.mouse_y) {
            return false;
        }
        let delta = if horizontal {
            if input.wheel_x.abs() > 0.01 {
                -input.wheel_x
            } else {
                input.wheel_y
            }
        } else {
            input.wheel_y
        };
        if delta.abs() < 0.01 {
            return false;
        }
        let max_off = (self.content - self.viewport).max(0.0);
        if max_off <= 0.5 {
            return false;
        }
        self.target = (self.target - delta * 34.0).clamp(0.0, max_off);
        self.last_scroll_ms = input.now_ms;
        true
    }

    pub fn scroll_to_end(&mut self) {
        let max_off = (self.content - self.viewport).max(0.0);
        self.target = max_off;
        self.offset = max_off;
    }

    pub fn draw_v_bar(&self, d2d: &mut D2dEngine, area: Rect, input: &UiInput) {
        if self.content <= self.viewport + 2.0 {
            return;
        }
        let active = area.contains(input.mouse_x, input.mouse_y)
            || input.now_ms.saturating_sub(self.last_scroll_ms) < 900;
        let alpha = if active { 0.24 } else { 0.09 };
        let max_off = (self.content - self.viewport).max(1.0);
        let ratio = (self.viewport / self.content).clamp(0.12, 1.0);
        let thumb_h = (area.h - 8.0) * ratio;
        let frac = (self.offset / max_off).clamp(0.0, 1.0);
        let thumb_y = area.y + 4.0 + (area.h - 8.0 - thumb_h) * frac;
        d2d.fill_rounded_rect(
            Rect::new(area.right() - 4.0, thumb_y, 2.5, thumb_h),
            1.5,
            Color::white(alpha),
        );
    }

    pub fn draw_h_bar(&self, d2d: &mut D2dEngine, area: Rect, input: &UiInput) {
        if self.content <= self.viewport + 2.0 {
            return;
        }
        let active = area.contains(input.mouse_x, input.mouse_y)
            || input.now_ms.saturating_sub(self.last_scroll_ms) < 900;
        let alpha = if active { 0.24 } else { 0.09 };
        let max_off = (self.content - self.viewport).max(1.0);
        let ratio = (self.viewport / self.content).clamp(0.12, 1.0);
        let thumb_w = (area.w - 8.0) * ratio;
        let frac = (self.offset / max_off).clamp(0.0, 1.0);
        let thumb_x = area.x + 4.0 + (area.w - 8.0 - thumb_w) * frac;
        d2d.fill_rounded_rect(
            Rect::new(thumb_x, area.bottom() - 4.0, thumb_w, 2.5),
            1.5,
            Color::white(alpha),
        );
    }
}

/// Draws DeskNotch's translucent frost card (`rounded-[24px]`).
pub fn draw_frost_card(d2d: &mut D2dEngine, rect: Rect, hovered: bool, border_tint: Option<Color>) {
    let fill = if hovered {
        Color::white(0.068)
    } else {
        Color::white(0.048)
    };
    let border = border_tint.unwrap_or_else(|| {
        if hovered {
            Color::white(0.13)
        } else {
            Color::white(0.085)
        }
    });
    d2d.fill_rounded_rect(rect, 24.0, fill);
    d2d.stroke_rounded_rect(rect, 24.0, 1.0, border);
}

/// Draws Tauri's crisp monochrome switch visual (`30x18px`) without registering a hit box.
pub fn draw_switch_visual(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    rect: Rect,
    checked: bool,
    hovered: bool,
    disabled: bool,
) {
    let pos = hits.get_switch_pos(id, checked);
    let alpha_mul = if disabled { 0.42 } else { 1.0 };
    let off_col = Color::white(if hovered && !disabled { 0.22 } else { 0.14 }).with_alpha(
        if hovered && !disabled { 0.22 } else { 0.14 } * alpha_mul,
    );
    let on_col = Color::white((if hovered && !disabled { 1.0 } else { 0.94 }) * alpha_mul);
    let track = off_col.lerp(on_col, pos);
    let r = rect.h * 0.5;
    d2d.fill_rounded_rect(rect, r, track);

    let knob_d = (rect.h - 4.0).max(8.0);
    let travel = (rect.w - knob_d - 4.0).max(0.0);
    let knob_x = rect.x + 2.0 + pos * travel;
    let knob_off = Color::white(0.72 * alpha_mul);
    let knob_on = Color::rgba(12, 12, 14, 0.96 * alpha_mul);
    let knob_col = knob_off.lerp(knob_on, pos);
    d2d.fill_circle(
        knob_x + knob_d * 0.5,
        rect.center_y(),
        knob_d * 0.5,
        knob_col,
    );
}

/// Draws an interactive monochrome pill switch (`30x18px`).
pub fn draw_switch(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    rect: Rect,
    checked: bool,
    _accent: Color,
    input: &UiInput,
) -> bool {
    let h = hits.register(id, rect, CursorKind::Pointer, None, input);
    draw_switch_visual(d2d, hits, id, rect, checked, h.hovered, false);
    h.clicked
}

/// Draws a Tauri-style rounded-9 segmented pill group and returns the clicked option index.
pub fn draw_segmented(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    prefix: &str,
    rect: Rect,
    options: &[(&str, &str)],
    active_id: &str,
    disabled_id: Option<&str>,
    input: &UiInput,
) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    d2d.fill_rounded_rect(rect, 9.0, Color::white(0.07));
    let pad = 2.0;
    let inner_w = (rect.w - pad * 2.0).max(20.0);
    let cell_w = inner_w / options.len() as f32;
    let cell_h = (rect.h - pad * 2.0).max(16.0);
    let mut chosen = None;
    for (i, (id, lbl)) in options.iter().enumerate() {
        let cr = Rect::new(rect.x + pad + i as f32 * cell_w, rect.y + pad, cell_w, cell_h);
        let is_act = *id == active_id;
        let is_dis = disabled_id == Some(*id);
        let h = hits.register(
            format!("{}_{}", prefix, id),
            cr,
            if is_dis { CursorKind::Arrow } else { CursorKind::Pointer },
            None,
            input,
        );
        if is_act {
            d2d.fill_rounded_rect(cr, 7.0, Color::white(0.16));
        } else if h.hovered && !is_dis {
            d2d.fill_rounded_rect(cr, 7.0, Color::white(0.06));
        }
        d2d.draw_text(
            lbl,
            cr.inset(3.0, 0.0),
            &TextStyle::default()
                .size(10.5)
                .medium()
                .color(if is_dis {
                    Color::white(0.28)
                } else if is_act {
                    Color::white(0.96)
                } else if h.hovered {
                    Color::white(0.84)
                } else {
                    Color::white(0.58)
                })
                .center()
                .vcenter()
                .nowrap()
                .ellipsis(),
        );
        if h.clicked && !is_dis && !is_act {
            chosen = Some((*id).to_string());
        }
    }
    chosen
}

/// Draws a pill button with optional background & border.
pub fn draw_pill_btn(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    rect: Rect,
    label: &str,
    active: bool,
    accent: Option<Color>,
    input: &UiInput,
) -> bool {
    let h = hits.register(id, rect, CursorKind::Pointer, None, input);
    let bg = if active {
        accent.unwrap_or(Color::white(0.16)).with_alpha(if h.hovered { 0.26 } else { 0.18 })
    } else if h.hovered {
        Color::white(0.10)
    } else {
        Color::white(0.05)
    };
    let border = if active {
        accent.unwrap_or(Color::white(0.28)).with_alpha(0.38)
    } else {
        Color::white(0.08)
    };
    let fg = if active {
        accent.unwrap_or(Color::white(0.95))
    } else if h.hovered {
        Color::white(0.90)
    } else {
        Color::white(0.68)
    };
    let r = (rect.h * 0.5).min(12.0);
    d2d.fill_rounded_rect(rect, r, bg);
    d2d.stroke_rounded_rect(rect, r, 1.0, border);
    d2d.draw_text(
        label,
        rect.inset(6.0, 0.0),
        &TextStyle::default()
            .size(11.0)
            .medium()
            .color(fg)
            .center()
            .vcenter()
            .nowrap()
            .ellipsis(),
    );
    h.clicked
}

/// Draws an icon button with hover background pill.
pub fn draw_icon_btn(
    d2d: &mut D2dEngine,
    hits: &mut HitRegistry,
    id: &str,
    rect: Rect,
    icon: IconId,
    icon_size: f32,
    fg: Color,
    hover_bg: Color,
    tooltip: Option<&str>,
    input: &UiInput,
) -> bool {
    let h = hits.register(
        id,
        rect,
        CursorKind::Pointer,
        tooltip.map(|s| s.to_string()),
        input,
    );
    if h.hovered || h.down {
        d2d.fill_rounded_rect(rect, (rect.h * 0.5).min(9.0), hover_bg);
    }
    let ir = Rect::new(
        rect.center_x() - icon_size * 0.5,
        rect.center_y() - icon_size * 0.5,
        icon_size,
        icon_size,
    );
    d2d.draw_icon(icon, ir, fg, 1.85, fg.a);
    h.clicked
}

pub fn draw_tooltip(d2d: &mut D2dEngine, hits: &HitRegistry, now_ms: u64, max_w: f32) {
    let Some((text, anchor)) = hits.active_tooltip(now_ms) else {
        return;
    };
    let style = TextStyle::default()
        .size(10.0)
        .medium()
        .color(Color::white(0.92))
        .center()
        .vcenter()
        .nowrap();
    let (tw, _) = d2d.measure_text(&text, 240.0, &style);
    let pw = (tw + 14.0).max(36.0);
    let ph = 20.0;
    let tx = (anchor.center_x() - pw * 0.5).clamp(8.0, (max_w - pw - 8.0).max(8.0));
    let ty = anchor.bottom() + 4.0;
    let r = Rect::new(tx, ty, pw, ph);
    d2d.fill_rounded_rect(r, 6.0, Color::rgb(22, 22, 26));
    d2d.stroke_rounded_rect(r, 6.0, 1.0, Color::white(0.14));
    d2d.draw_text(&text, r, &style);
}

#[derive(Default)]
pub struct MultiInputMap {
    pub map: HashMap<String, TextInputState>,
}

impl MultiInputMap {
    pub fn get_mut(&mut self, key: &str) -> &mut TextInputState {
        self.map.entry(key.to_string()).or_default()
    }
}
