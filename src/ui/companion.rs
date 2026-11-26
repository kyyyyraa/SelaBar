use crate::backend::store::StoreManager;
use crate::events::UiSender;
use crate::state::{hex_to_rgb, next_avatar_species, Settings};
use crate::ui::d2d::{rasterize_svg_premul_bgra, Color, D2dEngine, Rect, TextStyle};
use crate::ui::widgets::{CursorKind, HitRegistry, UiInput};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionMood {
    Idle,
    Hop,
    Focus,
    Sleep,
    Speaking,
    Think,
}

#[derive(Debug, Clone)]
pub struct CompanionState {
    pub mood: CompanionMood,
    pub switch_until_ms: u64,
    pub laugh_until_ms: u64,
    pub blink_next_ms: u64,
    pub blink_end_ms: u64,
    pub gaze_yaw: f32,
    pub gaze_pitch: f32,
}

impl Default for CompanionState {
    fn default() -> Self {
        Self {
            mood: CompanionMood::Idle,
            switch_until_ms: 0,
            laugh_until_ms: 0,
            blink_next_ms: 2800,
            blink_end_ms: 0,
            gaze_yaw: 0.0,
            gaze_pitch: 0.0,
        }
    }
}

impl CompanionState {
    /// Subtle scale pop feedback when switching mascot (no jump or 360 spin).
    pub fn trigger_switch(&mut self, now_ms: u64) {
        self.switch_until_ms = now_ms + 190;
    }

    pub fn step(&mut self, center_x: f32, center_y: f32, input: &UiInput) {
        let now = input.now_ms;
        if now >= self.blink_next_ms {
            self.blink_end_ms = now + 145;
            let jitter = ((now.wrapping_mul(2654435761)) % 2400) + 2900;
            self.blink_next_ms = now + jitter;
        }

        let (target_yaw, target_pitch) = if self.mood == CompanionMood::Sleep {
            (0.0, 0.14)
        } else if input.mouse_inside {
            let dx = ((input.mouse_x - center_x) / 260.0).clamp(-1.0, 1.0);
            let dy = ((input.mouse_y - center_y) / 180.0).clamp(-1.0, 1.0);
            (dx * (22.0 * PI / 180.0), (dy * 11.0 - 3.0) * PI / 180.0)
        } else {
            let sway = (now as f32 * 0.00085).sin() * (8.0 * PI / 180.0);
            let nod = (now as f32 * 0.00062).cos() * (3.0 * PI / 180.0) - (2.0 * PI / 180.0);
            (sway, nod)
        };

        let follow = (1.0 - (-9.5 * input.dt).exp()).clamp(0.05, 1.0);
        self.gaze_yaw += (target_yaw - self.gaze_yaw) * follow;
        self.gaze_pitch += (target_pitch - self.gaze_pitch) * follow;
    }
}

const CAP_N: usize = 144;

#[derive(Hash, PartialEq, Eq, Clone)]
struct CapKey {
    species: String,
    rgb: [u8; 3],
}

struct BakedCompanionCap {
    front_bgra: Vec<u8>,
    side_bgra: Vec<u8>,
}

pub struct CompanionRenderer {
    caps: HashMap<CapKey, BakedCompanionCap>,
}

impl CompanionRenderer {
    pub fn new() -> Self {
        Self {
            caps: HashMap::new(),
        }
    }

    pub fn cycle_avatar_on_click(
        clicked: bool,
        settings: &Settings,
        custom_photo: &str,
        store: &StoreManager,
        ui: &UiSender,
    ) {
        if !clicked {
            return;
        }
        let next = next_avatar_species(&settings.avatar, !custom_photo.is_empty());
        if next != settings.avatar {
            let mut m = Map::new();
            m.insert("avatar".to_string(), Value::String(next.to_string()));
            store.patch_settings(Some(ui), m);
        }
    }

    fn get_or_bake(&mut self, species: &str, rgb: [u8; 3]) -> &BakedCompanionCap {
        let key = CapKey {
            species: species.to_string(),
            rgb,
        };
        if !self.caps.contains_key(&key) {
            let baked = bake_companion_cap(species, rgb);
            if self.caps.len() >= 24 {
                self.caps.clear();
            }
            self.caps.insert(key.clone(), baked);
        }
        self.caps.get(&key).expect("just inserted")
    }

    /// Draws the smooth 3D Companion mascot inside `rect` (`size` logical px).
    /// Returns `true` if clicked.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        d2d: &mut D2dEngine,
        hits: &mut HitRegistry,
        id: &str,
        rect: Rect,
        size: f32,
        species: &str,
        avatar_hex: &str,
        photo_path: &str,
        state: &mut CompanionState,
        input: &UiInput,
    ) -> bool {
        let cx = rect.center_x();
        let cy = rect.center_y();
        state.step(cx, cy, input);

        let hit = hits.register(
            id,
            rect,
            CursorKind::Pointer,
            Some("Klik untuk ganti maskot".to_string()),
            input,
        );
        if hit.clicked {
            state.trigger_switch(input.now_ms);
        }

        let t = input.now_ms as f32 / 1000.0;
        let is_sleep = state.mood == CompanionMood::Sleep;
        let is_hop = state.mood == CompanionMood::Hop;
        let is_speak = state.mood == CompanionMood::Speaking;

        // Smooth breathing / ambient pose matching Tauri (`r8.update`)
        let mut dy = 0.0f32;
        let mut sx = 1.0f32;
        let mut sy = 1.0f32;
        let mut yaw = state.gaze_yaw;
        let mut pitch = state.gaze_pitch;

        let breath = (t * (2.0 * PI / 3.6)).sin();
        sx += breath * 0.008;
        sy += breath * 0.012;

        if input.now_ms < state.switch_until_ms {
            let p = 1.0 - (state.switch_until_ms - input.now_ms) as f32 / 190.0;
            let ease = 0.88 + 0.12 * (1.0 - (1.0 - p).powi(3));
            sx *= ease;
            sy *= ease;
        }

        if is_sleep {
            dy += (t * 1.4).sin() * (size * 0.022);
            pitch = 0.14 + (t * 1.4).sin() * 0.03;
            yaw = 0.0;
        } else if is_hop {
            let phase = (t / 0.68).fract();
            let jump = (phase * PI).sin();
            dy -= jump * (size * 0.10);
            let sq = if !(0.15..=0.85).contains(&phase) {
                -0.04
            } else {
                0.03 * jump
            };
            sy += sq;
            sx -= sq * 0.6;
        } else if is_speak {
            let bounce = (t * 9.5).sin().abs();
            dy -= bounce * (size * 0.04);
            sy += bounce * 0.035;
            sx -= bounce * 0.02;
        } else {
            dy += (t * (2.0 * PI / 3.4)).sin() * (size * 0.018);
        }

        self.draw_body(
            d2d,
            cx,
            cy,
            dy,
            sx,
            sy,
            yaw,
            pitch,
            size,
            species,
            avatar_hex,
            photo_path,
            is_sleep,
            input.now_ms < state.blink_end_ms,
            input.now_ms < state.laugh_until_ms,
            t,
            1.0,
        );

        hit.clicked
    }

    /// Renders a visual-only 3D companion mascot inside `rect` (e.g. Settings preview cards)
    /// without registering a duplicate hit target.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_visual(
        &mut self,
        d2d: &mut D2dEngine,
        rect: Rect,
        size: f32,
        species: &str,
        avatar_hex: &str,
        photo_path: &str,
        now_ms: u64,
    ) {
        self.draw_visual_alpha(d2d, rect, size, species, avatar_hex, photo_path, now_ms, 1.0);
    }

    /// Renders a visual-only 3D companion mascot with explicit opacity (`0.0..=1.0`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_visual_alpha(
        &mut self,
        d2d: &mut D2dEngine,
        rect: Rect,
        size: f32,
        species: &str,
        avatar_hex: &str,
        photo_path: &str,
        now_ms: u64,
        alpha: f32,
    ) {
        let t = now_ms as f32 / 1000.0;
        let breath = (t * (2.0 * PI / 3.6)).sin();
        let sx = 1.0 + breath * 0.008;
        let sy = 1.0 + breath * 0.012;
        let dy = (t * (2.0 * PI / 3.4)).sin() * (size * 0.018);
        let yaw = if alpha >= 0.9 { (t * 0.85).sin() * 0.14 } else { 0.0 };
        self.draw_body(
            d2d,
            rect.center_x(),
            rect.center_y(),
            dy,
            sx,
            sy,
            yaw,
            0.0,
            size,
            species,
            avatar_hex,
            photo_path,
            false,
            false,
            false,
            t,
            alpha.clamp(0.0, 1.0),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_body(
        &mut self,
        d2d: &mut D2dEngine,
        cx: f32,
        cy: f32,
        dy: f32,
        sx: f32,
        sy: f32,
        yaw: f32,
        pitch: f32,
        size: f32,
        species: &str,
        avatar_hex: &str,
        photo_path: &str,
        is_sleep: bool,
        is_blinking: bool,
        is_laughing: bool,
        t: f32,
        alpha: f32,
    ) {
        // Photo custom avatar mode
        if species == "photo" && !photo_path.is_empty() {
            let img_r = Rect::new(
                cx - size * 0.44 * sx,
                cy - size * 0.44 * sy + dy,
                size * 0.88 * sx,
                size * 0.88 * sy,
            );
            d2d.draw_image(photo_path, img_r, size * 0.28, alpha, true);
            d2d.stroke_rounded_rect(img_r, size * 0.28, 1.0, Color::white(0.18 * alpha));
            if is_sleep {
                draw_sleep_zs(d2d, cx + size * 0.34, cy - size * 0.28 + dy, size, t);
            }
            return;
        }

        let rgb = hex_to_rgb(avatar_hex);
        let rgb_arr = [rgb.0, rgb.1, rgb.2];
        let baked = self.get_or_bake(species, rgb_arr);

        let front_slot = format!("comp_v4f_{}_{:02x}{:02x}{:02x}", species, rgb.0, rgb.1, rgb.2);
        let side_slot = format!("comp_v4s_{}_{:02x}{:02x}{:02x}", species, rgb.0, rgb.1, rgb.2);
        d2d.update_dynamic_bgra(&front_slot, CAP_N as u32, CAP_N as u32, 1, &baked.front_bgra);
        d2d.update_dynamic_bgra(&side_slot, CAP_N as u32, CAP_N as u32, 1, &baked.side_bgra);

        // Soft ground ambient shadow below companion
        let shadow_scale = (1.0 - (-dy / (size * 0.4)).clamp(0.0, 0.3)).max(0.65);
        d2d.fill_radial_glow(
            cx,
            cy + size * 0.44,
            size * 0.35 * shadow_scale,
            size * 0.065 * shadow_scale,
            Color::black(0.36 * alpha),
            Color::black(0.0),
        );

        let cos_y = yaw.cos();
        let sin_y = yaw.sin();
        let cos_p = pitch.cos();
        let sin_p = pitch.sin();

        // Subtle curved vinyl 2.5D bevel layers matching Tauri's smooth side shading
        let draw_w = size * sx * (0.34 + 0.66 * cos_y.abs());
        let draw_h = size * sy * (0.90 + 0.10 * cos_p.abs());
        let depth_px = size * 0.085;
        let slices = 10i32;

        for s in (1..=slices).rev() {
            let n = s as f32 / slices as f32;
            let bevel = (0.94 + 0.06 * (1.0 - n * n).max(0.0).sqrt()).clamp(0.92, 1.0);
            let sw = draw_w * bevel;
            let sh = draw_h * bevel;
            let ox = -sin_y * depth_px * n;
            let oy = sin_p * depth_px * 0.45 * n;
            let sr = Rect::new(cx + ox - sw * 0.5, cy + dy + oy - sh * 0.5, sw, sh);
            d2d.draw_dynamic_slot(&side_slot, sr, 0.94 * alpha);
        }

        let fr = Rect::new(cx - draw_w * 0.5, cy + dy - draw_h * 0.5, draw_w, draw_h);
        d2d.draw_dynamic_slot(&front_slot, fr, alpha);

        // Clean Tauri-style dark minimal eyes centered according to species_face
        if cos_y > 0.25 {
            let (fx, fy, fscale) = species_face(species);
            // Account for SVG -5..105 viewBox scale (100/110 = 0.909)
            let svg_fit = 100.0 / 110.0;
            let face_ox = ((fx - 50.0) / 100.0) * size * svg_fit * cos_y;
            let face_oy = ((fy - 50.0) / 100.0) * size * svg_fit;
            let eye_shift_x = sin_y * (size * 0.16);
            let eye_shift_y = sin_p * (size * 0.12);
            let ex_base = cx + face_ox + eye_shift_x;
            let ey_base = cy + dy + face_oy + eye_shift_y;
            let sc = (size / 50.0) * fscale;
            let sep = 6.2 * sc * cos_y.clamp(0.5, 1.0);

            let lum = 0.2126 * rgb.0 as f32 + 0.7152 * rgb.1 as f32 + 0.0722 * rgb.2 as f32;
            let eye_col = if lum > 95.0 {
                Color::rgba(22, 22, 32, 0.92 * alpha)
            } else {
                Color::white(0.96 * alpha)
            };

            for side in [-1.0f32, 1.0f32] {
                let ex = ex_base + side * sep;
                let ey = ey_base;

                if is_sleep {
                    d2d.stroke_quad_bezier(
                        (ex - 2.9 * sc, ey - 0.4 * sc),
                        (ex, ey + 2.4 * sc),
                        (ex + 2.9 * sc, ey - 0.4 * sc),
                        (1.6 * sc).max(0.9),
                        eye_col,
                    );
                } else if is_laughing {
                    d2d.stroke_quad_bezier(
                        (ex - 2.9 * sc, ey + 0.9 * sc),
                        (ex, ey - 2.4 * sc),
                        (ex + 2.9 * sc, ey + 0.9 * sc),
                        (1.7 * sc).max(0.9),
                        eye_col,
                    );
                } else if is_blinking {
                    d2d.draw_line(
                        ex - 2.6 * sc,
                        ey,
                        ex + 2.6 * sc,
                        ey,
                        (1.6 * sc).max(0.9),
                        eye_col,
                    );
                } else {
                    let ew = 3.7 * sc * cos_y.clamp(0.55, 1.0);
                    let eh = 5.4 * sc;
                    d2d.fill_rounded_rect(
                        Rect::new(ex - ew * 0.5, ey - eh * 0.5, ew, eh),
                        ew * 0.5,
                        eye_col,
                    );
                }
            }

            if is_sleep {
                draw_sleep_zs(d2d, cx + size * 0.30, cy - size * 0.25 + dy, size, t);
            }
        }
    }
}

fn draw_sleep_zs(d2d: &mut D2dEngine, ox: f32, oy: f32, size: f32, t: f32) {
    let sc = (size / 48.0).max(0.7);
    for (idx, base_sz) in [(0, 9.0f32), (1, 12.0), (2, 15.0)] {
        let phase = ((t * 0.55) + idx as f32 * 0.33).fract();
        let zx = ox + phase * 10.0 * sc + (phase * PI * 2.0).sin() * 2.5;
        let zy = oy - phase * 16.0 * sc;
        let alpha = if phase < 0.2 {
            phase / 0.2
        } else {
            (1.0 - phase) / 0.8
        };
        let st = TextStyle::default()
            .size((base_sz * sc * 0.72).clamp(8.0, 16.0))
            .bold()
            .color(Color::white((alpha * 0.85).clamp(0.0, 1.0)))
            .center()
            .vcenter();
        d2d.draw_text("z", Rect::new(zx - 8.0, zy - 8.0, 16.0, 16.0), &st);
    }
}

fn species_face(species: &str) -> (f32, f32, f32) {
    match species {
        "cat" => (50.0, 58.0, 1.0),
        "blob" => (49.5, 50.0, 1.0),
        "clover" => (50.0, 50.0, 1.0),
        "droid" => (50.0, 60.0, 0.95),
        "alien" => (50.0, 45.0, 1.05),
        "cloud" => (50.0, 58.0, 0.95),
        _ => (50.0, 48.0, 0.95),
    }
}

fn species_svg(species: &str) -> &'static str {
    match species {
        "clover" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M26.53 22.38A25 25 0 0 1 73.47 22.38A7 7 0 0 0 77.62 26.53A25 25 0 0 1 77.62 73.47A7 7 0 0 0 73.47 77.62A25 25 0 0 1 26.53 77.62A7 7 0 0 0 22.38 73.47A25 25 0 0 1 22.38 26.53A7 7 0 0 0 26.53 22.38Z" fill="white"/></svg>"#
        }
        "blob" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M93.92 50C94.18 51.9 94.24 53.9 93.99 55.79C93.73 57.68 93.18 59.61 92.39 61.36C91.6 63.1 90.48 64.78 89.27 66.27C88.07 67.76 86.58 69.08 85.14 70.29C83.7 71.49 82.1 72.5 80.62 73.5C79.14 74.49 77.64 75.34 76.24 76.24C74.84 77.15 73.51 78 72.2 78.93C70.88 79.86 69.66 80.83 68.36 81.8C67.06 82.76 65.78 83.82 64.39 84.74C63 85.67 61.55 86.63 60.01 87.35C58.47 88.08 56.82 88.73 55.15 89.1C53.48 89.47 51.71 89.64 50 89.57C48.29 89.5 46.55 89.16 44.91 88.68C43.27 88.21 41.67 87.47 40.16 86.72C38.65 85.96 37.24 85.02 35.86 84.13C34.48 83.24 33.19 82.29 31.89 81.37C30.58 80.46 29.32 79.56 28.03 78.63C26.74 77.71 25.44 76.82 24.16 75.84C22.88 74.86 21.57 73.87 20.36 72.74C19.15 71.62 17.95 70.42 16.91 69.1C15.88 67.79 14.92 66.35 14.15 64.85C13.38 63.35 12.76 61.73 12.29 60.1C11.81 58.48 11.52 56.78 11.29 55.1C11.06 53.41 10.98 51.71 10.91 50C10.83 48.29 10.83 46.59 10.83 44.84C10.84 43.1 10.85 41.34 10.93 39.53C11.02 37.72 11.09 35.86 11.34 33.99C11.59 32.12 11.88 30.16 12.43 28.31C12.99 26.46 13.69 24.56 14.66 22.88C15.64 21.21 16.86 19.58 18.27 18.27C19.69 16.96 21.4 15.84 23.17 15.03C24.93 14.22 26.95 13.71 28.89 13.43C30.83 13.15 32.9 13.21 34.82 13.36C36.74 13.5 38.66 13.92 40.43 14.28C42.2 14.65 43.87 15.16 45.46 15.54C47.06 15.93 48.52 16.31 50 16.58C51.48 16.85 52.87 17.02 54.32 17.17C55.77 17.32 57.22 17.36 58.71 17.49C60.2 17.62 61.74 17.7 63.28 17.95C64.81 18.19 66.39 18.5 67.91 18.98C69.43 19.46 70.95 20.08 72.38 20.83C73.81 21.58 75.19 22.5 76.5 23.5C77.81 24.5 79.03 25.63 80.22 26.81C81.41 27.99 82.54 29.25 83.65 30.57C84.76 31.89 85.85 33.26 86.89 34.72C87.93 36.18 88.98 37.69 89.9 39.31C90.82 40.92 91.73 42.64 92.4 44.42C93.07 46.2 93.66 48.1 93.92 50Z" fill="white"/></svg>"#
        }
        "cat" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M50 20A36 36 0 1 1 50 92A36 36 0 1 1 50 20ZM24.72 44.64A4.5 4.5 0 0 1 17.35 40.67L20.21 16.66A4.5 4.5 0 0 1 26.86 13.26L42.3 21.83A4.5 4.5 0 0 1 43.02 29.21L24.72 44.64ZM82.65 40.67A4.5 4.5 0 0 1 75.28 44.64L56.98 29.21A4.5 4.5 0 0 1 57.7 21.83L73.14 13.26A4.5 4.5 0 0 1 79.79 16.66L82.65 40.67Z" fill="white"/></svg>"#
        }
        "droid" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M47.5 14H52.5V32H47.5ZM43 11A7 7 0 1 1 57 11A7 7 0 1 1 43 11ZM16 50C16 38.95 24.95 30 36 30H64C75.05 30 84 38.95 84 50V70C84 81.05 75.05 90 64 90H36C24.95 90 16 81.05 16 70ZM4 62A7 7 0 1 1 18 62A7 7 0 1 1 4 62ZM82 62A7 7 0 1 1 96 62A7 7 0 1 1 82 62Z" fill="white"/></svg>"#
        }
        "alien" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M50 10C70 10 83 27 83 46C83 65 64 92 50 92C36 92 17 65 17 46C17 27 30 10 50 10Z" fill="white"/></svg>"#
        }
        "cloud" => {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M19 44A25 25 0 1 1 69 44A25 25 0 1 1 19 44ZM47 50A21 21 0 1 1 89 50A21 21 0 1 1 47 50ZM7 68A17 17 0 1 1 41 68A17 17 0 1 1 7 68ZM32 73A18 18 0 1 1 68 73A18 18 0 1 1 32 73ZM60 70A16 16 0 1 1 92 70A16 16 0 1 1 60 70Z" fill="white"/></svg>"#
        }
        _ => {
            // Exact Tauri Ghost (`rF.ghost`)
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-5 -5 110 110"><path d="M17 50C17 31.78 31.78 17 50 17C68.22 17 83 31.78 83 50V81.5Q72 93.5 61 81.5Q50 93.5 39 81.5Q28 93.5 17 81.5Z" fill="white"/></svg>"#
        }
    }
}

/// 5-tap separable binomial blur matching Tauri's `os()` to eliminate jagged discrete normal steps.
fn smooth_field(grid: &mut [f32], n: usize) {
    let mut tmp = vec![0.0f32; n * n];
    for y in 0..n {
        let row = y * n;
        for x in 0..n {
            let xm2 = x.saturating_sub(2);
            let xm1 = x.saturating_sub(1);
            let xp1 = (x + 1).min(n - 1);
            let xp2 = (x + 2).min(n - 1);
            tmp[row + x] = (grid[row + xm2]
                + 4.0 * grid[row + xm1]
                + 6.0 * grid[row + x]
                + 4.0 * grid[row + xp1]
                + grid[row + xp2])
                * 0.0625;
        }
    }
    for x in 0..n {
        for y in 0..n {
            let ym2 = y.saturating_sub(2);
            let ym1 = y.saturating_sub(1);
            let yp1 = (y + 1).min(n - 1);
            let yp2 = (y + 2).min(n - 1);
            grid[y * n + x] = (tmp[ym2 * n + x]
                + 4.0 * tmp[ym1 * n + x]
                + 6.0 * tmp[y * n + x]
                + 4.0 * tmp[yp1 * n + x]
                + tmp[yp2 * n + x])
                * 0.0625;
        }
    }
}

/// Smooth signed-distance + Poisson dome height field + 64x64 soft vinyl MatCap.
fn bake_companion_cap(species: &str, rgb: [u8; 3]) -> BakedCompanionCap {
    let n = CAP_N;
    let svg = species_svg(species);
    let mask_bgra = rasterize_svg_premul_bgra(svg, n as u32, n as u32)
        .unwrap_or_else(|| vec![255u8; n * n * 4]);

    let mut alpha = vec![0.0f32; n * n];
    let mut inside = vec![false; n * n];
    let mut outside = vec![false; n * n];
    for i in 0..n * n {
        let a = mask_bgra[i * 4 + 3] as f32 / 255.0;
        alpha[i] = a;
        let in_b = a >= 0.5;
        inside[i] = in_b;
        outside[i] = !in_b;
    }

    let dist_in = edt_2d_sq(&inside, n, n);
    let dist_out = edt_2d_sq(&outside, n, n);

    let mut sdf = vec![0.0f32; n * n];
    for i in 0..n * n {
        let a = alpha[i];
        sdf[i] = if a > 0.01 && a < 0.99 {
            a - 0.5
        } else if inside[i] {
            dist_in[i].sqrt() - 0.5
        } else {
            0.5 - dist_out[i].sqrt()
        };
    }
    smooth_field(&mut sdf, n);
    smooth_field(&mut sdf, n);

    // SOR Poisson dome inside smoothed silhouette
    let mut phi = vec![0.0f32; n * n];
    let omega = 1.66f32;
    for _iter in 0..48 {
        for y in 1..(n - 1) {
            let row = y * n;
            for x in 1..(n - 1) {
                let idx = row + x;
                if sdf[idx] <= 0.0 {
                    continue;
                }
                let gs = 0.25 * (phi[idx - 1] + phi[idx + 1] + phi[idx - n] + phi[idx + n] + 1.0);
                phi[idx] = (1.0 - omega) * phi[idx] + omega * gs;
            }
        }
    }

    let max_phi = phi.iter().copied().fold(0.001f32, f32::max);
    let scale = 110.0 / n as f32;
    let mut height = vec![0.0f32; n * n];
    for i in 0..n * n {
        let s = (sdf[i] * scale).max(0.0);
        let p_norm = (phi[i] / max_phi).max(0.0).sqrt();
        // Smooth curved rim + pillow dome height without any sharp cliff
        let edge_dome = (s / 8.0).clamp(0.0, 1.0);
        let edge_curve = (1.0 - (1.0 - edge_dome).powi(2)).max(0.0).sqrt();
        height[i] = p_norm * 15.8 + edge_curve * 5.0;
    }
    smooth_field(&mut height, n);
    smooth_field(&mut height, n);

    let mat = build_plastic_matcap(rgb);
    let mut front_bgra = vec![0u8; n * n * 4];
    let mut side_bgra = vec![0u8; n * n * 4];

    let br = rgb[0] as f32 / 255.0;
    let bg = rgb[1] as f32 / 255.0;
    let bb = rgb[2] as f32 / 255.0;

    for y in 0..n {
        let yf = y as f32 / n as f32;
        // Soft vertical rim gradient for 3D side slices (warm bright rim matching MatCap, never muddy dark gray)
        let rim_shade = 0.88 - 0.14 * yf;
        for x in 0..n {
            let idx = y * n + x;
            let a = alpha[idx];
            if a <= 0.004 {
                continue;
            }

            let h_l = if x > 0 { height[idx - 1] } else { 0.0 };
            let h_r = if x + 1 < n { height[idx + 1] } else { 0.0 };
            let h_u = if y > 0 { height[idx - n] } else { 0.0 };
            let h_d = if y + 1 < n { height[idx + n] } else { 0.0 };

            let dx = (h_r - h_l) * 0.5;
            let dy = (h_d - h_u) * 0.5;
            let inv_len = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
            let nx = (-dx * inv_len).clamp(-0.96, 0.96);
            let ny = (-dy * inv_len).clamp(-0.96, 0.96);

            let mut occ = 0.0f32;
            let hc = height[idx];
            for r in 0..8 {
                let ang = (r as f32) * (PI * 0.25);
                let rx = ang.cos();
                let ry = ang.sin();
                let mut max_s = 0.0f32;
                for step in [2.5f32, 5.5, 9.0] {
                    let sx = (x as f32 + rx * step).round() as isize;
                    let sy = (y as f32 + ry * step).round() as isize;
                    if sx >= 0 && sx < n as isize && sy >= 0 && sy < n as isize {
                        let hs = height[sy as usize * n + sx as usize];
                        let s = ((hs - hc) / step).max(0.0);
                        if s > max_s {
                            max_s = s;
                        }
                    }
                }
                occ += (max_s * 0.28).clamp(0.0, 1.0);
            }
            let ao = (1.0 - (occ / 8.0) * 0.24).clamp(0.76, 1.0);

            let [mr, mg, mb] = sample_matcap(&mat, nx, ny);
            let r = (mr * ao).clamp(0.0, 1.0);
            let g = (mg * ao).clamp(0.0, 1.0);
            let b = (mb * ao).clamp(0.0, 1.0);

            let out = &mut front_bgra[idx * 4..idx * 4 + 4];
            out[0] = (b * a * 255.0).round() as u8;
            out[1] = (g * a * 255.0).round() as u8;
            out[2] = (r * a * 255.0).round() as u8;
            out[3] = (a * 255.0).round() as u8;

            let sr = (br * rim_shade).clamp(0.0, 1.0);
            let sg = (bg * rim_shade).clamp(0.0, 1.0);
            let sb = (bb * (rim_shade + 0.03)).clamp(0.0, 1.0);
            let sout = &mut side_bgra[idx * 4..idx * 4 + 4];
            sout[0] = (sb * a * 255.0).round() as u8;
            sout[1] = (sg * a * 255.0).round() as u8;
            sout[2] = (sr * a * 255.0).round() as u8;
            sout[3] = (a * 255.0).round() as u8;
        }
    }

    BakedCompanionCap {
        front_bgra,
        side_bgra,
    }
}

fn srgb_to_lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn tone_lin_to_srgb(v: f32) -> f32 {
    let clamped = v.clamp(0.0, 2.5);
    let i = if clamped <= 0.75 {
        clamped
    } else {
        0.75 + 0.25 * (1.0 - (-(clamped - 0.75) / 0.25).exp())
    };
    let srgb = if i <= 0.0031308 {
        12.92 * i
    } else {
        1.055 * i.powf(1.0 / 2.4) - 0.055
    };
    srgb.clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn soft_box(center: f32, half_w: f32, x: f32) -> f32 {
    1.0 - smoothstep(center - half_w, center + half_w, x)
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn build_plastic_matcap(rgb: [u8; 3]) -> Vec<[f32; 3]> {
    let m = 64usize;
    let mut tex = vec![[0.0f32; 3]; m * m];
    let sr = rgb[0] as f32 / 255.0;
    let sg = rgb[1] as f32 / 255.0;
    let sb = rgb[2] as f32 / 255.0;
    let lum = 0.2126 * sr + 0.7152 * sg + 0.0722 * sb;
    let br = (lum + (sr - lum) * 1.18).clamp(0.0, 1.0);
    let bg = (lum + (sg - lum) * 1.18).clamp(0.0, 1.0);
    let bb = (lum + (sb - lum) * 1.18).clamp(0.0, 1.0);

    let t_lin = [srgb_to_lin(br), srgb_to_lin(bg), srgb_to_lin(bb)];
    let d_max = t_lin[0].max(t_lin[1]).max(t_lin[2]).max(0.05);
    let u = [t_lin[0] / d_max, t_lin[1] / d_max, t_lin[2] / d_max];
    let shadow_k = (0.3 - 0.15 * 0.35_f32).max(0.03); // 0.2475 colored ambient
    let k = [shadow_k * u[0], shadow_k * u[1], shadow_k * u[2]];

    let ang = 265.0 * PI / 180.0;
    let lx = ang.cos();
    let ly = ang.sin();
    let r9 = 48.0 * PI / 180.0;
    let (oe, ot) = (r9.cos(), r9.sin());
    let l_dir = norm3([oe * lx, oe * ly, ot]);
    let v_dir = [0.0f32, 0.0, 1.0];
    let h_dir = norm3([l_dir[0] + v_dir[0], l_dir[1] + v_dir[1], l_dir[2] + v_dir[2]]);
    let u_dir = norm3([lx, ly, 0.0]);

    let gl = 80.0 * PI / 180.0;
    let (cg, sg_a) = (gl.cos(), gl.sin());
    let w_dir = norm3([
        0.55 * (cg * lx - sg_a * ly),
        0.55 * (sg_a * lx + cg * ly),
        0.83,
    ]);
    let a_dir = norm3([w_dir[1], -w_dir[0], 0.0]);
    let b_dir = [
        w_dir[1] * a_dir[2] - w_dir[2] * a_dir[1],
        w_dir[2] * a_dir[0] - w_dir[0] * a_dir[2],
        w_dir[0] * a_dir[1] - w_dir[1] * a_dir[0],
    ];

    let spread = 1.55_f32;
    let wrap_m = 0.15 + 0.14 * spread;
    let highlight = 1.3_f32;
    let spec_v = 0.45 * highlight;
    let spec_w = 0.10 * highlight;
    let box_b = 0.11 * highlight;
    let rim_m = 0.3 * 0.5_f32;

    for y in 0..m {
        let ny = (y as f32 / (m - 1) as f32) * 2.0 - 1.0;
        for x in 0..m {
            let nx = (x as f32 / (m - 1) as f32) * 2.0 - 1.0;
            let r2 = nx * nx + ny * ny;
            let (nnx, nny, nnz) = if r2 > 1.0 {
                let l = r2.sqrt().max(1e-5);
                (nx / l, ny / l, 0.0)
            } else {
                (nx, ny, (1.0 - r2).sqrt())
            };

            let ndotl = nnx * l_dir[0] + nny * l_dir[1] + nnz * l_dir[2];
            let ndotv = (nnx * v_dir[0] + nny * v_dir[1] + nnz * v_dir[2]).max(0.0);
            let ndoth = (nnx * h_dir[0] + nny * h_dir[1] + nnz * h_dir[2]).max(0.0);

            let wrap_s = ((ndotl + wrap_m) / (1.0 + wrap_m)).clamp(0.0, 1.0);
            let one_m_nv = 1.0 - ndotv;
            let n2 = one_m_nv * one_m_nv;
            let n3 = n2 * one_m_nv;
            let n5 = n3 * n2;
            let spec = (spec_v * ndoth.powi(62) + spec_w * ndoth.powi(5)) * (1.0 + 3.0 * n5);

            let rx = 2.0 * ndotv * nnx - v_dir[0];
            let ry = 2.0 * ndotv * nny - v_dir[1];
            let rz = 2.0 * ndotv * nnz - v_dir[2];
            let rim_f = 0.45 + 0.55 * smoothstep(-0.4, 0.6, rx * u_dir[0] + ry * u_dir[1] + rz * u_dir[2]);
            let rv = rx * w_dir[0] + ry * w_dir[1] + rz * w_dir[2];
            let mut softbox = 0.0_f32;
            if rv > 0.5 {
                let ea = (rx * a_dir[0] + ry * a_dir[1] + rz * a_dir[2]) / rv;
                let eb = (rx * b_dir[0] + ry * b_dir[1] + rz * b_dir[2]) / rv;
                softbox = soft_box(0.34, 0.12, ea.abs()) * soft_box(0.12, 0.06, eb.abs());
            }
            let rim_i = rim_m * n3 * rim_f + box_b * softbox;

            let r_lin = t_lin[0] * (k[0] + 0.85 * wrap_s) + spec + 0.92 * rim_i;
            let g_lin = t_lin[1] * (k[1] + 0.85 * wrap_s) + 0.98 * spec + 0.96 * rim_i;
            let b_lin = t_lin[2] * (k[2] + 0.85 * wrap_s) + 0.95 * spec + rim_i;
            tex[y * m + x] = [
                tone_lin_to_srgb(r_lin),
                tone_lin_to_srgb(g_lin),
                tone_lin_to_srgb(b_lin),
            ];
        }
    }
    tex
}

fn sample_matcap(tex: &[[f32; 3]], nx: f32, ny: f32) -> [f32; 3] {
    let fx = ((nx + 1.0) * 0.5 * 63.0).clamp(0.0, 62.999);
    let fy = ((ny + 1.0) * 0.5 * 63.0).clamp(0.0, 62.999);
    let x0 = fx as usize;
    let y0 = fy as usize;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let c00 = tex[y0 * 64 + x0];
    let c10 = tex[y0 * 64 + x0 + 1];
    let c01 = tex[(y0 + 1) * 64 + x0];
    let c11 = tex[(y0 + 1) * 64 + x0 + 1];
    let mut out = [0.0f32; 3];
    for ch in 0..3 {
        let a = c00[ch] + (c10[ch] - c00[ch]) * tx;
        let b = c01[ch] + (c11[ch] - c01[ch]) * tx;
        out[ch] = a + (b - a) * ty;
    }
    out
}

fn edt_1d(f: &[f32], n: usize, out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let mut s = ((f[q] + (q * q) as f32) - (f[v[k]] + (v[k] * v[k]) as f32))
            / (2 * q as isize - 2 * v[k] as isize) as f32;
        while s <= z[k] {
            k -= 1;
            s = ((f[q] + (q * q) as f32) - (f[v[k]] + (v[k] * v[k]) as f32))
                / (2 * q as isize - 2 * v[k] as isize) as f32;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    let mut kk = 0usize;
    for q in 0..n {
        while z[kk + 1] < q as f32 {
            kk += 1;
        }
        let d = q as f32 - v[kk] as f32;
        out[q] = d * d + f[v[kk]];
    }
}

fn edt_2d_sq(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let inf = 1e12f32;
    let mut grid: Vec<f32> = inside.iter().map(|&b| if b { inf } else { 0.0 }).collect();
    let max_dim = w.max(h);
    let mut f = vec![0.0f32; max_dim];
    let mut d = vec![0.0f32; max_dim];
    let mut v = vec![0usize; max_dim];
    let mut z = vec![0.0f32; max_dim + 1];

    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], h, &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..y * w + w]);
        edt_1d(&f[..w], w, &mut d[..w], &mut v, &mut z);
        for x in 0..w {
            grid[y * w + x] = d[x].max(0.0);
        }
    }
    grid
}
