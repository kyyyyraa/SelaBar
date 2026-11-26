use crate::ui::icons::IconId;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use image::imageops::FilterType;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::hash::{Hash, Hasher};
use std::mem::ManuallyDrop;
use tiny_skia::Pixmap;
use windows::core::{Interface, HRESULT, PCWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory1, ID2D1Geometry,
    ID2D1Layer, ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_GAMMA_2_2, D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES1, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory5, IDWriteFontCollection, IDWriteInMemoryFontFileLoader,
    IDWriteTextFormat, IDWriteTextLayout, DWRITE_CONTAINER_TYPE_WOFF,
    DWRITE_CONTAINER_TYPE_WOFF2, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_HIT_TEST_METRICS, DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_FAR, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_TRIMMING_GRANULARITY_NONE,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;
use windows_numerics::{Matrix3x2, Vector2};

static GEIST_WOFF2: &[u8] = include_bytes!("../../assets/GeistVariable.woff2");
const D2DERR_RECREATE_TARGET_HR: i32 = 0x8899_000C_u32 as i32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    pub const fn white(a: f32) -> Self {
        Self::new(1.0, 1.0, 1.0, a)
    }

    pub const fn black(a: f32) -> Self {
        Self::new(0.0, 0.0, 0.0, a)
    }

    pub fn from_hex(hex: &str) -> Self {
        let h = hex.trim().trim_start_matches('#');
        if !h.is_ascii() {
            return Self::white(1.0);
        }
        if h.len() == 6 {
            let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(255);
            let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(255);
            let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(255);
            return Self::rgb(r, g, b);
        }
        if h.len() == 8 {
            let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(255);
            let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(255);
            let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(255);
            let a = u8::from_str_radix(&h[6..8], 16).unwrap_or(255) as f32 / 255.0;
            return Self::rgba(r, g, b, a);
        }
        Self::white(1.0)
    }

    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    pub fn to_d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a,
        }
    }

    pub fn to_hex(self) -> String {
        let r = (self.r.clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (self.g.clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (self.b.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }

    fn key_u32(self) -> u32 {
        let r = (self.r.clamp(0.0, 1.0) * 255.0).round() as u32;
        let g = (self.g.clamp(0.0, 1.0) * 255.0).round() as u32;
        let b = (self.b.clamp(0.0, 1.0) * 255.0).round() as u32;
        let a = (self.a.clamp(0.0, 1.0) * 255.0).round() as u32;
        (a << 24) | (r << 16) | (g << 8) | b
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w * 0.5
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h * 0.5
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - dx * 2.0).max(0.0),
            h: (self.h - dy * 2.0).max(0.0),
        }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    pub fn intersect(&self, other: Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn to_d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextVAlign {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone)]
pub struct TextStyle {
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
    pub mono: bool,
    pub color: Color,
    pub align: TextAlign,
    pub valign: TextVAlign,
    pub nowrap: bool,
    pub ellipsis: bool,
    pub line_height: Option<f32>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 12.0,
            weight: 400,
            italic: false,
            mono: false,
            color: Color::white(0.92),
            align: TextAlign::Left,
            valign: TextVAlign::Top,
            nowrap: false,
            ellipsis: false,
            line_height: None,
        }
    }
}

impl TextStyle {
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn weight(mut self, w: u16) -> Self {
        self.weight = w;
        self
    }
    pub fn medium(mut self) -> Self {
        self.weight = 500;
        self
    }
    pub fn semibold(mut self) -> Self {
        self.weight = 600;
        self
    }
    pub fn bold(mut self) -> Self {
        self.weight = 700;
        self
    }
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
    pub fn center(mut self) -> Self {
        self.align = TextAlign::Center;
        self
    }
    pub fn right(mut self) -> Self {
        self.align = TextAlign::Right;
        self
    }
    pub fn vcenter(mut self) -> Self {
        self.valign = TextVAlign::Center;
        self
    }
    pub fn nowrap(mut self) -> Self {
        self.nowrap = true;
        self
    }
    pub fn ellipsis(mut self) -> Self {
        self.ellipsis = true;
        self.nowrap = true;
        self
    }
    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = Some(lh);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FormatKey {
    size_q10: u32,
    weight: u16,
    italic: bool,
    mono: bool,
    align: TextAlign,
    valign: TextVAlign,
    nowrap: bool,
    ellipsis: bool,
    lh_q10: u32,
}

impl From<&TextStyle> for FormatKey {
    fn from(s: &TextStyle) -> Self {
        Self {
            size_q10: (s.size * 10.0).round() as u32,
            weight: s.weight,
            italic: s.italic,
            mono: s.mono,
            align: s.align,
            valign: s.valign,
            nowrap: s.nowrap,
            ellipsis: s.ellipsis,
            lh_q10: s.line_height.map(|v| (v * 10.0).round() as u32).unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text_hash: u64,
    text_len: usize,
    max_w_q4: u32,
    max_h_q4: u32,
    fmt: FormatKey,
}

struct DynamicSlot {
    w: u32,
    h: u32,
    generation: u64,
    bitmap: Option<ID2D1Bitmap>,
}

#[derive(Clone)]
enum CachedImage {
    Ready(ID2D1Bitmap),
    Missing,
}

pub struct D2dEngine {
    d2d_factory: ID2D1Factory1,
    dw_factory: IDWriteFactory5,
    in_mem_loader: Option<IDWriteInMemoryFontFileLoader>,
    _unpacked_font_bytes: Option<Vec<u8>>,
    font_collection: Option<IDWriteFontCollection>,
    primary_family: String,
    target: Option<ID2D1DCRenderTarget>,
    current_scale: f32,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    formats: HashMap<FormatKey, IDWriteTextFormat>,
    layouts: HashMap<LayoutKey, IDWriteTextLayout>,
    layout_order: VecDeque<LayoutKey>,
    svgs: HashMap<String, ID2D1Bitmap>,
    images: HashMap<String, CachedImage>,
    image_order: VecDeque<String>,
    dynamic_slots: HashMap<String, DynamicSlot>,
    layer_stack: Vec<ID2D1Layer>,
}

impl Drop for D2dEngine {
    fn drop(&mut self) {
        if let Some(loader) = &self.in_mem_loader {
            unsafe {
                let _ = self.dw_factory.UnregisterFontFileLoader(loader);
            }
        }
    }
}

impl D2dEngine {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                    .map_err(|e| format!("D2D1CreateFactory failed: {e}"))?;
            let dw_factory: IDWriteFactory5 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                    .map_err(|e| format!("DWriteCreateFactory failed: {e}"))?;

            let (in_mem_loader, unpacked_bytes, font_collection, primary_family) =
                load_geist_font(&dw_factory);

            Ok(Self {
                d2d_factory,
                dw_factory,
                in_mem_loader,
                _unpacked_font_bytes: unpacked_bytes,
                font_collection,
                primary_family,
                target: None,
                current_scale: 1.0,
                brushes: HashMap::new(),
                formats: HashMap::new(),
                layouts: HashMap::new(),
                layout_order: VecDeque::new(),
                svgs: HashMap::new(),
                images: HashMap::new(),
                image_order: VecDeque::new(),
                dynamic_slots: HashMap::new(),
                layer_stack: Vec::new(),
            })
        }
    }

    pub fn font_family(&self) -> &str {
        &self.primary_family
    }

    fn ensure_target(&mut self) -> Option<&ID2D1DCRenderTarget> {
        if self.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0 * self.current_scale,
                dpiY: 96.0 * self.current_scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            unsafe {
                if let Ok(rt) = self.d2d_factory.CreateDCRenderTarget(&props) {
                    rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                    self.target = Some(rt);
                }
            }
        }
        self.target.as_ref()
    }

    fn invalidate_device_resources(&mut self) {
        self.target = None;
        self.brushes.clear();
        self.svgs.clear();
        self.images.clear();
        self.image_order.clear();
        for s in self.dynamic_slots.values_mut() {
            s.bitmap = None;
        }
        self.layer_stack.clear();
    }

    pub fn begin_frame(&mut self, hdc: HDC, phys_w: u32, phys_h: u32, scale: f32) -> bool {
        let scale = scale.max(0.5);
        self.current_scale = scale;
        if self.ensure_target().is_none() {
            return false;
        }
        let rt = self.target.as_ref().unwrap().clone();
        unsafe {
            rt.SetDpi(96.0 * scale, 96.0 * scale);
            let sub = RECT {
                left: 0,
                top: 0,
                right: phys_w as i32,
                bottom: phys_h as i32,
            };
            if rt.BindDC(hdc, &sub).is_err() {
                self.invalidate_device_resources();
                return false;
            }
            rt.BeginDraw();
            let ident = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            };
            rt.SetTransform(&ident);
            rt.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }
        true
    }

    pub fn end_frame(&mut self) -> bool {
        let Some(rt) = self.target.as_ref() else {
            return false;
        };
        unsafe {
            match rt.EndDraw(None, None) {
                Ok(()) => true,
                Err(e) => {
                    if e.code() == HRESULT(D2DERR_RECREATE_TARGET_HR) {
                        self.invalidate_device_resources();
                    }
                    false
                }
            }
        }
    }

    fn brush(&mut self, color: Color) -> Option<ID2D1SolidColorBrush> {
        let k = color.key_u32();
        if let Some(b) = self.brushes.get(&k) {
            return Some(b.clone());
        }
        let rt = self.target.as_ref()?;
        unsafe {
            let b = rt.CreateSolidColorBrush(&color.to_d2d(), None).ok()?;
            if self.brushes.len() > 256 {
                self.brushes.clear();
            }
            self.brushes.insert(k, b.clone());
            Some(b)
        }
    }

    fn format(&mut self, style: &TextStyle) -> Option<IDWriteTextFormat> {
        let key = FormatKey::from(style);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let family_name = if style.mono {
            "Consolas"
        } else {
            self.primary_family.as_str()
        };
        let mut wide_family: Vec<u16> = family_name.encode_utf16().collect();
        wide_family.push(0);
        let wide_locale: [u16; 6] = [b'e' as u16, b'n' as u16, b'-' as u16, b'U' as u16, b'S' as u16, 0];
        let col = if style.mono {
            None
        } else {
            self.font_collection.as_ref()
        };

        unsafe {
            let fmt = self
                .dw_factory
                .CreateTextFormat(
                    PCWSTR(wide_family.as_ptr()),
                    col,
                    DWRITE_FONT_WEIGHT(style.weight as i32),
                    if style.italic {
                        DWRITE_FONT_STYLE_ITALIC
                    } else {
                        DWRITE_FONT_STYLE_NORMAL
                    },
                    DWRITE_FONT_STRETCH_NORMAL,
                    style.size.max(6.0),
                    PCWSTR(wide_locale.as_ptr()),
                )
                .or_else(|_| {
                    let segoe: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
                    self.dw_factory.CreateTextFormat(
                        PCWSTR(segoe.as_ptr()),
                        None,
                        DWRITE_FONT_WEIGHT(style.weight as i32),
                        if style.italic {
                            DWRITE_FONT_STYLE_ITALIC
                        } else {
                            DWRITE_FONT_STYLE_NORMAL
                        },
                        DWRITE_FONT_STRETCH_NORMAL,
                        style.size.max(6.0),
                        PCWSTR(wide_locale.as_ptr()),
                    )
                })
                .ok()?;

            let align = match style.align {
                TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                TextAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            };
            let valign = match style.valign {
                TextVAlign::Top => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                TextVAlign::Center => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                TextVAlign::Bottom => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
            };
            let _ = fmt.SetTextAlignment(align);
            let _ = fmt.SetParagraphAlignment(valign);
            let _ = fmt.SetWordWrapping(if style.nowrap {
                DWRITE_WORD_WRAPPING_NO_WRAP
            } else {
                DWRITE_WORD_WRAPPING_WRAP
            });

            if style.ellipsis {
                if let Ok(sign) = self.dw_factory.CreateEllipsisTrimmingSign(&fmt) {
                    let trim = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = fmt.SetTrimming(&trim, &sign);
                }
            } else {
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_NONE,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = fmt.SetTrimming(&trim, None);
            }

            if let Some(lh) = style.line_height {
                let _ = fmt.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    lh,
                    lh * 0.8,
                );
            }

            if self.formats.len() > 128 {
                self.formats.clear();
            }
            self.formats.insert(key, fmt.clone());
            Some(fmt)
        }
    }

    fn text_layout(
        &mut self,
        text: &str,
        max_w: f32,
        max_h: f32,
        style: &TextStyle,
    ) -> Option<IDWriteTextLayout> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let key = LayoutKey {
            text_hash: h.finish(),
            text_len: text.len(),
            max_w_q4: (max_w.max(1.0) * 4.0).round() as u32,
            max_h_q4: (max_h.max(1.0) * 4.0).round() as u32,
            fmt: FormatKey::from(style),
        };
        if let Some(l) = self.layouts.get(&key) {
            return Some(l.clone());
        }
        let fmt = self.format(style)?;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self
                .dw_factory
                .CreateTextLayout(&utf16, &fmt, max_w.max(1.0), max_h.max(1.0))
                .ok()?;
            if self.layout_order.len() >= 192 {
                if let Some(old) = self.layout_order.pop_front() {
                    self.layouts.remove(&old);
                }
            }
            self.layouts.insert(key.clone(), layout.clone());
            self.layout_order.push_back(key);
            Some(layout)
        }
    }

    pub fn measure_text(&mut self, text: &str, max_w: f32, style: &TextStyle) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, style.line_height.unwrap_or(style.size * 1.35));
        }
        let Some(layout) = self.text_layout(text, max_w, 4096.0, style) else {
            return (0.0, style.size * 1.35);
        };
        unsafe {
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                (m.widthIncludingTrailingWhitespace, m.height)
            } else {
                (0.0, style.size * 1.35)
            }
        }
    }

    pub fn hit_test_point(
        &mut self,
        text: &str,
        max_w: f32,
        style: &TextStyle,
        local_x: f32,
        local_y: f32,
    ) -> usize {
        let Some(layout) = self.text_layout(text, max_w, 2048.0, style) else {
            return 0;
        };
        unsafe {
            let mut trailing = windows::core::BOOL(0);
            let mut inside = windows::core::BOOL(0);
            let mut metrics = DWRITE_HIT_TEST_METRICS::default();
            if layout
                .HitTestPoint(local_x, local_y, &mut trailing, &mut inside, &mut metrics)
                .is_ok()
            {
                let utf16_pos = metrics.textPosition as usize
                    + if trailing.as_bool() {
                        metrics.length as usize
                    } else {
                        0
                    };
                utf16_offset_to_utf8(text, utf16_pos)
            } else {
                text.len()
            }
        }
    }

    pub fn caret_metrics(
        &mut self,
        text: &str,
        max_w: f32,
        style: &TextStyle,
        utf8_idx: usize,
    ) -> (f32, f32, f32) {
        let fallback_lh = style.line_height.unwrap_or(style.size * 1.35);
        if text.is_empty() {
            return (0.0, 0.0, fallback_lh);
        }
        let clamped_idx = utf8_idx.min(text.len());
        let utf16_pos = utf8_offset_to_utf16(text, clamped_idx);
        let probe = if text.ends_with('\n') && clamped_idx == text.len() {
            Some(format!("{} ", text))
        } else {
            None
        };
        let layout_text = probe.as_deref().unwrap_or(text);
        let Some(layout) = self.text_layout(layout_text, max_w, 2048.0, style) else {
            return (0.0, 0.0, fallback_lh);
        };
        unsafe {
            let mut x = 0.0f32;
            let mut y = 0.0f32;
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            if layout
                .HitTestTextPosition(utf16_pos as u32, false, &mut x, &mut y, &mut m)
                .is_ok()
            {
                (x, y, if m.height > 1.0 { m.height } else { fallback_lh })
            } else {
                (0.0, 0.0, fallback_lh)
            }
        }
    }

    pub fn caret_x(&mut self, text: &str, max_w: f32, style: &TextStyle, utf8_idx: usize) -> f32 {
        self.caret_metrics(text, max_w, style, utf8_idx).0
    }

    pub fn draw_text(&mut self, text: &str, rect: Rect, style: &TextStyle) {
        if text.is_empty() || rect.w <= 0.5 || rect.h <= 0.5 {
            return;
        }
        let Some(layout) = self.text_layout(text, rect.w, rect.h, style) else {
            return;
        };
        let Some(brush) = self.brush(style.color) else {
            return;
        };
        let Some(rt) = self.target.as_ref() else {
            return;
        };
        unsafe {
            rt.DrawTextLayout(
                Vector2 {
                    X: rect.x,
                    Y: rect.y,
                },
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        if rect.w <= 0.0 || rect.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.FillRectangle(&rect.to_d2d(), &b);
        }
    }

    pub fn stroke_rect(&mut self, rect: Rect, stroke_w: f32, color: Color) {
        if rect.w <= 0.0 || rect.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.DrawRectangle(&rect.to_d2d(), &b, stroke_w, None);
        }
    }

    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if rect.w <= 0.0 || rect.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        let rr = D2D1_ROUNDED_RECT {
            rect: rect.to_d2d(),
            radiusX: r,
            radiusY: r,
        };
        unsafe {
            rt.FillRoundedRectangle(&rr, &b);
        }
    }

    pub fn stroke_rounded_rect(&mut self, rect: Rect, radius: f32, stroke_w: f32, color: Color) {
        if rect.w <= 0.0 || rect.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        let rr = D2D1_ROUNDED_RECT {
            rect: rect.to_d2d(),
            radiusX: r,
            radiusY: r,
        };
        unsafe {
            rt.DrawRoundedRectangle(&rr, &b, stroke_w, None);
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        if radius <= 0.0 || color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        let el = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            rt.FillEllipse(&el, &b);
        }
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, stroke_w: f32, color: Color) {
        if radius <= 0.0 || color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        let el = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            rt.DrawEllipse(&el, &b, stroke_w, None);
        }
    }

    pub fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, stroke_w: f32, color: Color) {
        if color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.DrawLine(
                Vector2 { X: x1, Y: y1 },
                Vector2 { X: x2, Y: y2 },
                &b,
                stroke_w,
                None,
            );
        }
    }

    /// Antigravity-style 2x2 clockwise chasing-dots loading indicator (Foto 6).
    pub fn draw_spinning_box(&mut self, cx: f32, cy: f32, size: f32, now_ms: u64, _color: Color) {
        let phase = (now_ms as f32 / 1000.0) * 4.4;
        let head = phase % 4.0;
        let off = size * 0.27;
        let dot_r = (size * 0.19).max(1.3);
        let offsets = [
            (-off, -off), // top-left
            (off, -off),  // top-right
            (off, off),   // bottom-right
            (-off, off),  // bottom-left
        ];
        for (idx, (dx, dy)) in offsets.iter().enumerate() {
            let k = idx as f32;
            let dist = (head - k + 4.0) % 4.0;
            let norm = (dist / 2.3).clamp(0.0, 1.0);
            let intensity = (1.0 - norm).powf(1.35);
            let alpha = 0.20 + 0.74 * intensity;
            let px = cx + dx;
            let py = cy + dy;
            if intensity > 0.35 {
                self.fill_circle(px, py, dot_r * 1.45, Color::rgba(232, 236, 245, alpha * 0.22));
            }
            self.fill_circle(px, py, dot_r, Color::rgba(232, 236, 245, alpha));
        }
    }

    pub fn draw_ring_arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        progress_0_to_1: f32,
        stroke_w: f32,
        track_color: Color,
        fill_color: Color,
    ) {
        self.stroke_circle(cx, cy, radius, stroke_w, track_color);
        let p = progress_0_to_1.clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        if p >= 0.999 {
            self.stroke_circle(cx, cy, radius, stroke_w, fill_color);
            return;
        }
        let Some(b) = self.brush(fill_color) else { return };
        let Some(rt) = self.target.clone() else { return };
        let start_angle = -PI * 0.5;
        let end_angle = start_angle + p * 2.0 * PI;
        let sx = cx + radius * start_angle.cos();
        let sy = cy + radius * start_angle.sin();
        let ex = cx + radius * end_angle.cos();
        let ey = cy + radius * end_angle.sin();
        unsafe {
            if let Ok(geom) = self.d2d_factory.CreatePathGeometry() {
                if let Ok(sink) = geom.Open() {
                    sink.BeginFigure(Vector2 { X: sx, Y: sy }, D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: Vector2 { X: ex, Y: ey },
                        size: D2D_SIZE_F {
                            width: radius,
                            height: radius,
                        },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: if p > 0.5 {
                            D2D1_ARC_SIZE_LARGE
                        } else {
                            D2D1_ARC_SIZE_SMALL
                        },
                    });
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    let _ = sink.Close();
                    let style = self.round_stroke_style();
                    rt.DrawGeometry(&geom, &b, stroke_w, style.as_ref());
                }
            }
        }
    }

    pub fn fill_linear_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        start: (f32, f32),
        end: (f32, f32),
        stops: &[(f32, Color)],
    ) {
        if rect.w <= 0.0 || rect.h <= 0.0 || stops.is_empty() {
            return;
        }
        let Some(rt) = self.target.clone() else { return };
        let d2_stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|(pos, col)| D2D1_GRADIENT_STOP {
                position: *pos,
                color: col.to_d2d(),
            })
            .collect();
        unsafe {
            let Ok(coll) = rt.CreateGradientStopCollection(
                &d2_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 {
                    X: start.0,
                    Y: start.1,
                },
                endPoint: Vector2 { X: end.0, Y: end.1 },
            };
            if let Ok(brush) = rt.CreateLinearGradientBrush(&props, None, &coll) {
                let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
                let rr = D2D1_ROUNDED_RECT {
                    rect: rect.to_d2d(),
                    radiusX: r,
                    radiusY: r,
                };
                rt.FillRoundedRectangle(&rr, &brush);
            }
        }
    }

    pub fn fill_radial_glow(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        inner: Color,
        outer: Color,
    ) {
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let Some(rt) = self.target.clone() else { return };
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: inner.to_d2d(),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: outer.to_d2d(),
            },
        ];
        unsafe {
            let Ok(coll) =
                rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return;
            };
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center: Vector2 { X: cx, Y: cy },
                gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
                radiusX: rx,
                radiusY: ry,
            };
            if let Ok(brush) = rt.CreateRadialGradientBrush(&props, None, &coll) {
                let el = D2D1_ELLIPSE {
                    point: Vector2 { X: cx, Y: cy },
                    radiusX: rx,
                    radiusY: ry,
                };
                rt.FillEllipse(&el, &brush);
            }
        }
    }

    /// Builds DeskNotch's exact top-flush notch path:
    /// horizontal top edge flush with `y = 0`, optional concave top ears (`8x8px` when `!is_open && !tucked`),
    /// straight vertical sides, and rounded bottom corners (`14px` closed, `30px` expanded).
    pub fn build_notch_path(
        &self,
        rect: Rect,
        bottom_radius: f32,
        ear_radius: f32,
    ) -> Option<ID2D1PathGeometry> {
        let ear = ear_radius.max(0.0).min(rect.h * 0.48);
        let br = bottom_radius
            .min(rect.w * 0.5)
            .min((rect.h - ear).max(0.0))
            .max(0.0);
        let x0 = rect.x;
        let x1 = rect.x + rect.w;
        let y0 = rect.y;
        let y1 = rect.y + rect.h;

        unsafe {
            let geom = self.d2d_factory.CreatePathGeometry().ok()?;
            let sink = geom.Open().ok()?;

            if ear > 0.5 {
                sink.BeginFigure(Vector2 { X: x0 - ear, Y: y0 }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(Vector2 { X: x1 + ear, Y: y0 });
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x1, Y: y0 + ear },
                    size: D2D_SIZE_F {
                        width: ear,
                        height: ear,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.BeginFigure(Vector2 { X: x0, Y: y0 }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(Vector2 { X: x1, Y: y0 });
            }

            sink.AddLine(Vector2 {
                X: x1,
                Y: y1 - br,
            });
            if br > 0.5 {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x1 - br, Y: y1 },
                    size: D2D_SIZE_F {
                        width: br,
                        height: br,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            }
            sink.AddLine(Vector2 {
                X: x0 + br,
                Y: y1,
            });
            if br > 0.5 {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x0, Y: y1 - br },
                    size: D2D_SIZE_F {
                        width: br,
                        height: br,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            }
            if ear > 0.5 {
                sink.AddLine(Vector2 { X: x0, Y: y0 + ear });
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x0 - ear, Y: y0 },
                    size: D2D_SIZE_F {
                        width: ear,
                        height: ear,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.AddLine(Vector2 { X: x0, Y: y0 });
            }

            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            let _ = sink.Close();
            Some(geom.into())
        }
    }

    /// Renders the DeskNotch shell silhouette (`#0b0b0d` Obsidian Black or translucent Glass frost).
    pub fn draw_notch_shell(
        &mut self,
        rect: Rect,
        bottom_radius: f32,
        ear_radius: f32,
        is_glass: bool,
        is_open: bool,
        accent: Color,
        edge_glow: f32,
    ) {
        let Some(geom) = self.build_notch_path(rect, bottom_radius, ear_radius) else {
            return;
        };

        let fill_color = if is_glass {
            Color::rgba(18, 19, 24, 0.86)
        } else {
            Color::rgb(11, 11, 13) // #0b0b0d
        };

        let Some(fill_brush) = self.brush(fill_color) else {
            return;
        };
        let Some(rt) = self.target.clone() else {
            return;
        };

        unsafe {
            rt.FillGeometry(&geom, &fill_brush, None);
        }

        if is_glass {
            self.push_geometry_clip(&geom, rect);
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: Color::white(0.11).to_d2d(),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.45,
                    color: Color::white(0.02).to_d2d(),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: Color::white(0.05).to_d2d(),
                },
            ];
            unsafe {
                if let Ok(coll) =
                    rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                {
                    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: Vector2 {
                            X: rect.x,
                            Y: rect.y,
                        },
                        endPoint: Vector2 {
                            X: rect.x,
                            Y: rect.bottom(),
                        },
                    };
                    if let Ok(lb) = rt.CreateLinearGradientBrush(&props, None, &coll) {
                        rt.FillGeometry(&geom, &lb, None);
                    }
                }
            }
            self.pop_layer();
        }

        if edge_glow > 0.01 {
            self.push_geometry_clip(&geom, rect);
            self.fill_radial_glow(
                rect.center_x(),
                rect.bottom(),
                rect.w * 0.42,
                rect.h * 0.55,
                accent.with_alpha(0.14 * edge_glow),
                accent.with_alpha(0.0),
            );
            self.pop_layer();
        }

        if is_glass {
            let border_color = Color::white(if is_open { 0.18 } else { 0.14 });
            if let Some(bb) = self.brush(border_color) {
                unsafe {
                    rt.DrawGeometry(&geom, &bb, 1.0, None);
                }
            }
        }
    }

    /// Draws DeskNotch's soft cyan/indigo/violet glowing bottom aurora rim for Ask (`Ctrl+Alt+C`), Voice AI, and subtle Chat AI edge tint.
    pub fn draw_ai_aurora_rim(
        &mut self,
        rect: Rect,
        bottom_radius: f32,
        ear_radius: f32,
        phase: f32,
        subtle_thin: bool,
    ) {
        let Some(geom) = self.build_notch_path(rect, bottom_radius, ear_radius) else {
            return;
        };
        let bounds = Rect::new(
            rect.x - ear_radius,
            rect.y,
            rect.w + ear_radius * 2.0,
            rect.h,
        );
        self.push_geometry_clip(&geom, bounds);

        let shift = phase.sin() * (rect.w * 0.08);
        if subtle_thin {
            // Whisper-thin cyan/indigo/violet edge glow along the bottom edge for Chat AI tab
            self.fill_radial_glow(
                rect.center_x() - rect.w * 0.16 + shift,
                rect.bottom() + 2.0,
                rect.w * 0.42,
                22.0,
                Color::rgba(56, 189, 248, 0.20),
                Color::rgba(56, 189, 248, 0.0),
            );
            self.fill_radial_glow(
                rect.center_x() - shift * 0.6,
                rect.bottom() + 2.0,
                rect.w * 0.36,
                20.0,
                Color::rgba(99, 102, 241, 0.18),
                Color::rgba(99, 102, 241, 0.0),
            );
            self.fill_radial_glow(
                rect.center_x() + rect.w * 0.18 + shift * 0.5,
                rect.bottom() + 2.0,
                rect.w * 0.34,
                18.0,
                Color::rgba(192, 132, 252, 0.16),
                Color::rgba(192, 132, 252, 0.0),
            );
            self.pop_layer();
            return;
        }

        // 1. Soft vertical aurora wash over the lower 80% of the AI popup notch (borderless, like Music Ambient Glow)
        let wash_r = Rect::new(rect.x, rect.y + rect.h * 0.20, rect.w, rect.h * 0.80);
        self.fill_linear_rounded_rect(
            wash_r,
            0.0,
            (wash_r.x, wash_r.bottom()),
            (wash_r.x, wash_r.y),
            &[
                (0.0, Color::rgba(99, 102, 241, 0.22)),
                (0.42, Color::rgba(56, 189, 248, 0.10)),
                (0.78, Color::rgba(192, 132, 252, 0.025)),
                (1.0, Color::rgba(99, 102, 241, 0.0)),
            ],
        );

        // 2. Three soft animated radial plumes rising gently from the bottom edge
        let plume_ry = (rect.h * 0.60).max(36.0);
        self.fill_radial_glow(
            rect.center_x() - rect.w * 0.22 + shift,
            rect.bottom() - rect.h * 0.06,
            rect.w * 0.48,
            plume_ry,
            Color::rgba(56, 189, 248, 0.34),
            Color::rgba(56, 189, 248, 0.0),
        );
        self.fill_radial_glow(
            rect.center_x() - shift * 0.6,
            rect.bottom() - rect.h * 0.08,
            rect.w * 0.46,
            plume_ry,
            Color::rgba(99, 102, 241, 0.32),
            Color::rgba(99, 102, 241, 0.0),
        );
        self.fill_radial_glow(
            rect.center_x() + rect.w * 0.22 + shift * 0.5,
            rect.bottom() - rect.h * 0.06,
            rect.w * 0.46,
            plume_ry,
            Color::rgba(192, 132, 252, 0.30),
            Color::rgba(192, 132, 252, 0.0),
        );

        // 3. Soft bottom edge bloom
        self.fill_radial_glow(
            rect.center_x() - rect.w * 0.14 + shift * 0.7,
            rect.bottom() + 2.0,
            rect.w * 0.42,
            24.0,
            Color::rgba(56, 189, 248, 0.34),
            Color::rgba(56, 189, 248, 0.0),
        );
        self.fill_radial_glow(
            rect.center_x() + rect.w * 0.14 - shift * 0.7,
            rect.bottom() + 2.0,
            rect.w * 0.42,
            24.0,
            Color::rgba(192, 132, 252, 0.32),
            Color::rgba(192, 132, 252, 0.0),
        );

        self.pop_layer();
    }

    fn round_stroke_style(&self) -> Option<ID2D1StrokeStyle> {
        let props = D2D1_STROKE_STYLE_PROPERTIES1 {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        unsafe { self.d2d_factory.CreateStrokeStyle(&props, None).ok().map(Into::into) }
    }

    pub fn push_clip_rect(&mut self, rect: Rect) {
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.PushAxisAlignedClip(&rect.to_d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
    }

    pub fn pop_clip_rect(&mut self) {
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.PopAxisAlignedClip();
        }
    }

    pub fn push_rounded_clip(&mut self, rect: Rect, radius: f32) {
        let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        let rr = D2D1_ROUNDED_RECT {
            rect: rect.to_d2d(),
            radiusX: r,
            radiusY: r,
        };
        unsafe {
            if let Ok(rg) = self.d2d_factory.CreateRoundedRectangleGeometry(&rr) {
                let g: ID2D1Geometry = rg.into();
                self.push_geometry_clip(&g, rect);
            }
        }
    }

    pub fn push_notch_clip(&mut self, rect: Rect, bottom_radius: f32, ear_radius: f32) {
        if let Some(pg) = self.build_notch_path(rect, bottom_radius, ear_radius) {
            let g: ID2D1Geometry = pg.into();
            self.push_geometry_clip(&g, Rect::new(rect.x - ear_radius, rect.y, rect.w + ear_radius * 2.0, rect.h));
        }
    }

    fn push_geometry_clip(&mut self, geom: &ID2D1Geometry, bounds: Rect) {
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            if let Ok(layer) = rt.CreateLayer(None) {
                let params = D2D1_LAYER_PARAMETERS {
                    contentBounds: bounds.to_d2d(),
                    geometricMask: ManuallyDrop::new(Some(geom.clone())),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2 {
                        M11: 1.0,
                        M12: 0.0,
                        M21: 0.0,
                        M22: 1.0,
                        M31: 0.0,
                        M32: 0.0,
                    },
                    opacity: 1.0,
                    opacityBrush: ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS_NONE,
                };
                rt.PushLayer(&params, &layer);
                ManuallyDrop::into_inner(params.geometricMask);
                self.layer_stack.push(layer);
            }
        }
    }

    pub fn push_opacity_layer(&mut self, bounds: Rect, alpha: f32) -> bool {
        let a = alpha.clamp(0.0, 1.0);
        if a >= 0.995 {
            return false;
        }
        let Some(rt) = self.target.clone() else {
            return false;
        };
        unsafe {
            if let Ok(layer) = rt.CreateLayer(None) {
                let params = D2D1_LAYER_PARAMETERS {
                    contentBounds: bounds.to_d2d(),
                    geometricMask: ManuallyDrop::new(None),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2 {
                        M11: 1.0,
                        M12: 0.0,
                        M21: 0.0,
                        M22: 1.0,
                        M31: 0.0,
                        M32: 0.0,
                    },
                    opacity: a,
                    opacityBrush: ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS_NONE,
                };
                rt.PushLayer(&params, &layer);
                self.layer_stack.push(layer);
                return true;
            }
        }
        false
    }

    pub fn pop_layer(&mut self) {
        if self.layer_stack.pop().is_some() {
            if let Some(rt) = self.target.as_ref() {
                unsafe {
                    rt.PopLayer();
                }
            }
        }
    }

    pub fn fill_triangle(
        &mut self,
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        color: Color,
    ) {
        if color.a <= 0.001 {
            return;
        }
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            if let Ok(geom) = self.d2d_factory.CreatePathGeometry() {
                if let Ok(sink) = geom.Open() {
                    sink.BeginFigure(Vector2 { X: p0.0, Y: p0.1 }, D2D1_FIGURE_BEGIN_FILLED);
                    sink.AddLine(Vector2 { X: p1.0, Y: p1.1 });
                    sink.AddLine(Vector2 { X: p2.0, Y: p2.1 });
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    let _ = sink.Close();
                    rt.FillGeometry(&geom, &b, None);
                }
            }
        }
    }

    /// Tauri `ambientVideo` style ambient glow wash inside the lower 90% of the expanded notch while music plays.
    pub fn draw_music_ambient_glow(
        &mut self,
        rect: Rect,
        bottom_radius: f32,
        ear_radius: f32,
        accent: Color,
        phase: f32,
        opacity: f32,
    ) {
        let op = opacity.clamp(0.0, 1.0);
        if op <= 0.004 || rect.w <= 20.0 || rect.h <= 20.0 {
            return;
        }
        let Some(geom) = self.build_notch_path(rect, bottom_radius, ear_radius) else {
            return;
        };
        let bounds = Rect::new(
            rect.x - ear_radius,
            rect.y,
            rect.w + ear_radius * 2.0,
            rect.h,
        );
        self.push_geometry_clip(&geom, bounds);

        let max_c = accent.r.max(accent.g).max(accent.b);
        let boosted = if max_c > 0.01 && max_c < 0.72 {
            let scale = (0.72 / max_c).min(2.2);
            Color::new(
                (accent.r * scale).min(1.0),
                (accent.g * scale).min(1.0),
                (accent.b * scale).min(1.0),
                1.0,
            )
        } else {
            accent
        };

        let wash_r = Rect::new(rect.x, rect.y + rect.h * 0.04, rect.w, rect.h * 0.96);
        self.fill_linear_rounded_rect(
            wash_r,
            0.0,
            (wash_r.x, wash_r.bottom()),
            (wash_r.x, wash_r.y),
            &[
                (0.0, boosted.with_alpha(0.28 * op)),
                (0.42, boosted.with_alpha(0.14 * op)),
                (0.78, boosted.with_alpha(0.035 * op)),
                (1.0, boosted.with_alpha(0.0)),
            ],
        );

        let warm_shift = Color::new(
            (boosted.r * 0.85 + 0.15).min(1.0),
            (boosted.g * 0.90 + 0.06).min(1.0),
            (boosted.b * 0.85).min(1.0),
            1.0,
        );
        let cool_shift = Color::new(
            (boosted.r * 0.80).min(1.0),
            (boosted.g * 0.90 + 0.08).min(1.0),
            (boosted.b * 0.85 + 0.15).min(1.0),
            1.0,
        );

        let dx1 = (phase * 0.42).sin() * (rect.w * 0.14);
        let dy1 = (phase * 0.31).cos() * (rect.h * 0.06);
        let dx2 = (phase * 0.33 + 2.1).cos() * (rect.w * 0.16);
        let dy2 = (phase * 0.27 + 1.2).sin() * (rect.h * 0.05);
        let dx3 = (phase * 0.24 + 4.0).sin() * (rect.w * 0.10);

        self.fill_radial_glow(
            rect.x + rect.w * 0.28 + dx1,
            rect.bottom() - rect.h * 0.14 + dy1,
            rect.w * 0.52,
            rect.h * 0.72,
            boosted.with_alpha(0.42 * op),
            boosted.with_alpha(0.0),
        );
        self.fill_radial_glow(
            rect.x + rect.w * 0.72 + dx2,
            rect.bottom() - rect.h * 0.18 + dy2,
            rect.w * 0.50,
            rect.h * 0.68,
            warm_shift.with_alpha(0.34 * op),
            warm_shift.with_alpha(0.0),
        );
        self.fill_radial_glow(
            rect.center_x() + dx3,
            rect.bottom() - rect.h * 0.06,
            rect.w * 0.46,
            rect.h * 0.56,
            cool_shift.with_alpha(0.28 * op),
            cool_shift.with_alpha(0.0),
        );
        self.fill_radial_glow(
            rect.center_x() + dx3 * 0.7,
            rect.bottom() + 2.0,
            rect.w * 0.48,
            28.0,
            boosted.with_alpha(0.36 * op),
            boosted.with_alpha(0.0),
        );

        self.pop_layer();
    }

    /// Quadratic Bezier curve stroke helper for smooth UI lines & Companion eyes.
    pub fn stroke_quad_bezier(
        &mut self,
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        stroke_w: f32,
        color: Color,
    ) {
        let Some(b) = self.brush(color) else { return };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            if let Ok(geom) = self.d2d_factory.CreatePathGeometry() {
                if let Ok(sink) = geom.Open() {
                    sink.BeginFigure(Vector2 { X: p0.0, Y: p0.1 }, D2D1_FIGURE_BEGIN_HOLLOW);
                    // Convert quadratic P0,P1,P2 -> cubic C1 = P0 + 2/3*(P1-P0), C2 = P2 + 2/3*(P1-P2)
                    let c1 = Vector2 {
                        X: p0.0 + (p1.0 - p0.0) * (2.0 / 3.0),
                        Y: p0.1 + (p1.1 - p0.1) * (2.0 / 3.0),
                    };
                    let c2 = Vector2 {
                        X: p2.0 + (p1.0 - p2.0) * (2.0 / 3.0),
                        Y: p2.1 + (p1.1 - p2.1) * (2.0 / 3.0),
                    };
                    sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                        point1: c1,
                        point2: c2,
                        point3: Vector2 { X: p2.0, Y: p2.1 },
                    });
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    let _ = sink.Close();
                    let style = self.round_stroke_style();
                    rt.DrawGeometry(&geom, &b, stroke_w, style.as_ref());
                }
            }
        }
    }

    pub fn draw_icon(
        &mut self,
        icon: IconId,
        rect: Rect,
        color: Color,
        stroke_width: f32,
        opacity: f32,
    ) {
        let color_hex = color.to_hex();
        let eff_alpha = (color.a * opacity).clamp(0.0, 1.0);
        if eff_alpha <= 0.005 {
            return;
        }
        let pw = (rect.w * self.current_scale).ceil().max(4.0) as u32;
        let ph = (rect.h * self.current_scale).ceil().max(4.0) as u32;
        let sw_q = (stroke_width * 10.0).round() as u32;
        let key = format!("{}:{}:{}:{}:{}", icon.key(), color_hex, sw_q, pw, ph);
        let bmp = if let Some(b) = self.svgs.get(&key) {
            Some(b.clone())
        } else {
            let svg = icon.svg_markup(&color_hex, stroke_width);
            if let Some(bgra) = rasterize_svg_premul_bgra(&svg, pw, ph) {
                if let Some(b) = self.upload_bgra_bitmap(pw, ph, &bgra) {
                    if self.svgs.len() > 200 {
                        self.svgs.clear();
                    }
                    self.svgs.insert(key, b.clone());
                    Some(b)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(b) = bmp {
            let Some(rt) = self.target.clone() else { return };
            unsafe {
                rt.DrawBitmap(
                    &b,
                    Some(&rect.to_d2d()),
                    eff_alpha,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
        }
    }

    pub fn draw_image(&mut self, source: &str, rect: Rect, radius: f32, opacity: f32, cover: bool) {
        if source.is_empty() || rect.w <= 0.5 || rect.h <= 0.5 || opacity <= 0.005 {
            return;
        }
        if !self.images.contains_key(source) {
            let loaded = load_image_bitmap_data(source, 360)
                .and_then(|(w, h, bgra)| self.upload_bgra_bitmap(w, h, &bgra));
            self.cache_image(source.to_string(), loaded);
        }
        self.draw_cached_image(source, rect, radius, opacity, cover);
    }

    pub fn draw_image_bytes(
        &mut self,
        cache_key: &str,
        raw_bytes: &[u8],
        rect: Rect,
        radius: f32,
        opacity: f32,
        cover: bool,
    ) {
        if raw_bytes.is_empty() || rect.w <= 0.5 || rect.h <= 0.5 || opacity <= 0.005 {
            return;
        }
        if !self.images.contains_key(cache_key) {
            let loaded = decode_image_bitmap_bytes(raw_bytes, 360)
                .and_then(|(w, h, bgra)| self.upload_bgra_bitmap(w, h, &bgra));
            self.cache_image(cache_key.to_string(), loaded);
        }
        self.draw_cached_image(cache_key, rect, radius, opacity, cover);
    }

    fn cache_image(&mut self, key: String, bmp: Option<ID2D1Bitmap>) {
        if self.image_order.len() >= 28 {
            if let Some(old) = self.image_order.pop_front() {
                self.images.remove(&old);
            }
        }
        self.image_order.push_back(key.clone());
        self.images.insert(
            key,
            match bmp {
                Some(b) => CachedImage::Ready(b),
                None => CachedImage::Missing,
            },
        );
    }

    fn draw_cached_image(&mut self, key: &str, rect: Rect, radius: f32, opacity: f32, cover: bool) {
        let Some(CachedImage::Ready(bmp)) = self.images.get(key).cloned() else {
            return;
        };
        let Some(rt) = self.target.clone() else { return };

        if radius > 0.5 {
            self.push_rounded_clip(rect, radius);
        }
        unsafe {
            let size = bmp.GetSize();
            let src_rect = if cover && size.width > 0.0 && size.height > 0.0 {
                let bmp_aspect = size.width / size.height;
                let dst_aspect = rect.w / rect.h;
                if bmp_aspect > dst_aspect {
                    let crop_w = size.height * dst_aspect;
                    let left = (size.width - crop_w) * 0.5;
                    Some(D2D_RECT_F {
                        left,
                        top: 0.0,
                        right: left + crop_w,
                        bottom: size.height,
                    })
                } else {
                    let crop_h = size.width / dst_aspect;
                    let top = (size.height - crop_h) * 0.5;
                    Some(D2D_RECT_F {
                        left: 0.0,
                        top,
                        right: size.width,
                        bottom: top + crop_h,
                    })
                }
            } else {
                None
            };
            rt.DrawBitmap(
                &bmp,
                Some(&rect.to_d2d()),
                opacity,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                src_rect.as_ref().map(|r| r as *const _),
            );
        }
        if radius > 0.5 {
            self.pop_layer();
        }
    }

    pub fn update_dynamic_bgra(
        &mut self,
        slot: &str,
        w: u32,
        h: u32,
        generation: u64,
        premul_bgra: &[u8],
    ) {
        if w == 0 || h == 0 || premul_bgra.len() < (w * h * 4) as usize {
            return;
        }
        let needs_upload = match self.dynamic_slots.get(slot) {
            Some(s) => s.w != w || s.h != h || s.generation != generation || s.bitmap.is_none(),
            None => true,
        };
        if !needs_upload {
            return;
        }
        if let Some(bmp) = self.upload_bgra_bitmap(w, h, premul_bgra) {
            if !self.dynamic_slots.contains_key(slot) && self.dynamic_slots.len() >= 64 {
                self.dynamic_slots.clear();
            }
            self.dynamic_slots.insert(
                slot.to_string(),
                DynamicSlot {
                    w,
                    h,
                    generation,
                    bitmap: Some(bmp),
                },
            );
        }
    }

    pub fn draw_dynamic_slot(&mut self, slot: &str, rect: Rect, opacity: f32) {
        let Some(entry) = self.dynamic_slots.get(slot) else {
            return;
        };
        let Some(bmp) = entry.bitmap.as_ref() else {
            return;
        };
        let Some(rt) = self.target.clone() else { return };
        unsafe {
            rt.DrawBitmap(
                bmp,
                Some(&rect.to_d2d()),
                opacity,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    /// Trims disposable D2D caches when the notch has been idle (keeps font loader & render target intact).
    pub fn trim_idle_caches(&mut self) {
        self.layouts.clear();
        self.layout_order.clear();
        self.brushes.clear();
        self.dynamic_slots.clear();
        if self.svgs.len() > 48 {
            self.svgs.clear();
        }
        while self.images.len() > 4 {
            let Some(oldest) = self.image_order.pop_front() else {
                break;
            };
            self.images.remove(&oldest);
        }
    }

    fn upload_bgra_bitmap(&self, w: u32, h: u32, premul_bgra: &[u8]) -> Option<ID2D1Bitmap> {
        let rt = self.target.as_ref()?;
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe {
            rt.CreateBitmap(
                D2D_SIZE_U {
                    width: w,
                    height: h,
                },
                Some(premul_bgra.as_ptr().cast()),
                w * 4,
                &props,
            )
            .ok()
        }
    }
}

unsafe fn load_geist_font(
    dw: &IDWriteFactory5,
) -> (
    Option<IDWriteInMemoryFontFileLoader>,
    Option<Vec<u8>>,
    Option<IDWriteFontCollection>,
    String,
) {
    let container = dw.AnalyzeContainerType(GEIST_WOFF2.as_ptr().cast(), GEIST_WOFF2.len() as u32);
    let bytes: Vec<u8> = if container == DWRITE_CONTAINER_TYPE_WOFF2 || container == DWRITE_CONTAINER_TYPE_WOFF {
        if let Ok(stream) = dw.UnpackFontFile(container, GEIST_WOFF2.as_ptr().cast(), GEIST_WOFF2.len() as u32) {
            if let Ok(sz) = stream.GetFileSize() {
                let sz = sz as usize;
                let mut buf = vec![0u8; sz];
                let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                let mut ctx: *mut std::ffi::c_void = std::ptr::null_mut();
                if stream.ReadFileFragment(&mut ptr, 0, sz as u64, &mut ctx).is_ok() && !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(ptr.cast::<u8>(), buf.as_mut_ptr(), sz);
                    stream.ReleaseFileFragment(ctx);
                    buf
                } else {
                    return (None, None, None, "Segoe UI".to_string());
                }
            } else {
                return (None, None, None, "Segoe UI".to_string());
            }
        } else {
            return (None, None, None, "Segoe UI".to_string());
        }
    } else {
        GEIST_WOFF2.to_vec()
    };

    let Ok(loader) = dw.CreateInMemoryFontFileLoader() else {
        return (None, None, None, "Segoe UI".to_string());
    };
    if dw.RegisterFontFileLoader(&loader).is_err() {
        return (None, None, None, "Segoe UI".to_string());
    }
    let Ok(font_file) = loader.CreateInMemoryFontFileReference(
        dw,
        bytes.as_ptr().cast(),
        bytes.len() as u32,
        None,
    ) else {
        let _ = dw.UnregisterFontFileLoader(&loader);
        return (None, None, None, "Segoe UI".to_string());
    };
    let Ok(builder) = dw.CreateFontSetBuilder() else {
        let _ = dw.UnregisterFontFileLoader(&loader);
        return (None, None, None, "Segoe UI".to_string());
    };
    if builder.AddFontFile(&font_file).is_err() {
        let _ = dw.UnregisterFontFileLoader(&loader);
        return (None, None, None, "Segoe UI".to_string());
    }
    let Ok(font_set) = builder.CreateFontSet() else {
        let _ = dw.UnregisterFontFileLoader(&loader);
        return (None, None, None, "Segoe UI".to_string());
    };
    let Ok(col1) = dw.CreateFontCollectionFromFontSet(&font_set) else {
        let _ = dw.UnregisterFontFileLoader(&loader);
        return (None, None, None, "Segoe UI".to_string());
    };

    let mut family_name = "Geist".to_string();
    if let Ok(fam) = col1.GetFontFamily(0) {
        if let Ok(names) = fam.GetFamilyNames() {
            if let Ok(len) = names.GetStringLength(0) {
                let mut u16buf = vec![0u16; len as usize + 1];
                if names.GetString(0, &mut u16buf).is_ok() {
                    family_name = String::from_utf16_lossy(&u16buf[..len as usize]);
                }
            }
        }
    }
    let col: Option<IDWriteFontCollection> = col1.cast().ok();
    (Some(loader), Some(bytes), col, family_name)
}

pub fn rasterize_svg_premul_bgra(svg_str: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg_str, &opt).ok()?;
    let mut pix = Pixmap::new(w, h)?;
    let size = tree.size();
    let sx = w as f32 / size.width().max(1.0);
    let sy = h as f32 / size.height().max(1.0);
    let tr = tiny_skia::Transform::from_scale(sx, sy);
    resvg::render(&tree, tr, &mut pix.as_mut());
    // tiny-skia outputs premultiplied RGBA; swap R <-> B to get premultiplied BGRA for Direct2D.
    let mut data = pix.take();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(data)
}

fn decode_image_bitmap_bytes(raw_bytes: &[u8], max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(raw_bytes).ok()?;
    let (iw, ih) = (img.width(), img.height());
    let img = if iw > max_side || ih > max_side {
        img.resize(max_side, max_side, FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in rgba.as_raw().chunks_exact(4) {
        let a = px[3] as u16;
        let r = ((px[0] as u16 * a + 127) / 255) as u8;
        let g = ((px[1] as u16 * a + 127) / 255) as u8;
        let b = ((px[2] as u16 * a + 127) / 255) as u8;
        out.extend_from_slice(&[b, g, r, a as u8]);
    }
    Some((w, h, out))
}

fn load_image_bitmap_data(source: &str, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let raw_bytes: Vec<u8> = if let Some(rest) = source.strip_prefix("data:image/") {
        let (_, b64) = rest.split_once(";base64,")?;
        B64.decode(b64.trim()).ok()?
    } else {
        std::fs::read(source).ok()?
    };
    decode_image_bitmap_bytes(&raw_bytes, max_side)
}

fn utf8_offset_to_utf16(s: &str, byte_idx: usize) -> usize {
    let clamped = byte_idx.min(s.len());
    let mut idx = clamped;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    s[..idx].encode_utf16().count()
}

fn utf16_offset_to_utf8(s: &str, utf16_pos: usize) -> usize {
    let mut count = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if count >= utf16_pos {
            return byte_idx;
        }
        count += ch.len_utf16();
    }
    s.len()
}
