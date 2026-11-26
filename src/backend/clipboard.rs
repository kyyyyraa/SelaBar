use crate::events::{UiEvent, UiSender};
use base64::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};

const MAX_HISTORY: usize = 24;
const POLL_INTERVAL_MS: u64 = 800;
const MAX_TEXT_UNITS: usize = 2_000_000;
const MAX_DIB_BYTES: usize = 8 * 1024 * 1024;

/// Stores `GetClipboardSequenceNumber() as u64 + 1` for self-initiated copies
/// that should be ignored by `ClipboardWatcher`.
static SELF_COPY_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct ClipboardWatcher {
    running: Arc<AtomicBool>,
    history: Arc<Mutex<Vec<String>>>,
}

impl ClipboardWatcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            history: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn history(&self) -> Vec<String> {
        self.history.lock().map(|h| h.clone()).unwrap_or_default()
    }

    pub fn start(&self, ui: UiSender) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let running = Arc::clone(&self.running);
        let history = Arc::clone(&self.history);

        std::thread::spawn(move || {
            let mut last_text = String::new();
            let mut last_image_sig: Option<u64> = None;
            let mut last_seq: u32 = unsafe { GetClipboardSequenceNumber() };

            while running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));

                let seq = unsafe { GetClipboardSequenceNumber() };
                if seq == last_seq {
                    continue;
                }
                last_seq = seq;

                if SELF_COPY_SEQ.load(Ordering::SeqCst) == (seq as u64 + 1) {
                    let text = read_clipboard_text().unwrap_or_default();
                    let image = read_clipboard_image();
                    if !text.is_empty() {
                        last_text = text;
                    }
                    if let Some((sig, _)) = image {
                        last_image_sig = Some(sig);
                    }
                    continue;
                }

                let text = read_clipboard_text().unwrap_or_default();
                let image = read_clipboard_image();

                if text.is_empty() && image.is_none() {
                    continue;
                }

                let text_changed = !text.is_empty() && text != last_text;
                let image_changed = match (&image, last_image_sig) {
                    (Some((sig, _)), Some(prev)) => *sig != prev,
                    (Some(_), None) => true,
                    _ => false,
                };

                if !text_changed && !image_changed {
                    continue;
                }

                if text_changed {
                    last_text = text.clone();
                }
                if let Some((sig, _)) = &image {
                    last_image_sig = Some(*sig);
                }

                if text_changed {
                    if let Ok(mut h) = history.lock() {
                        h.insert(0, text.clone());
                        h.truncate(MAX_HISTORY);
                    }
                }

                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let trimmed = text.trim();
                let kind = if trimmed.is_empty() {
                    "image"
                } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    "link"
                } else {
                    "text"
                };
                let image_data = image.map(|(_, data_url)| data_url).unwrap_or_default();

                ui.send(UiEvent::ClipboardNew {
                    text,
                    image_bmp_data_url: image_data,
                    kind: kind.to_string(),
                    at_ms: at,
                });
            }
        });
    }
}

pub fn read_clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT as u32);
            if handle.is_null() {
                return None;
            }
            let ptr = GlobalLock(handle as HGLOBAL);
            if ptr.is_null() {
                return None;
            }
            let wide = ptr as *const u16;
            let mut len = 0usize;
            while len < MAX_TEXT_UNITS && *wide.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            GlobalUnlock(handle as HGLOBAL);
            Some(text)
        })();
        CloseClipboard();
        result
    }
}

fn read_clipboard_dib() -> Option<Vec<u8>> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let result = (|| {
            let mut handle = GetClipboardData(CF_DIB as u32);
            if handle.is_null() {
                handle = GetClipboardData(CF_DIBV5 as u32);
            }
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle as HGLOBAL);
            if size == 0 || size > MAX_DIB_BYTES {
                return None;
            }
            let ptr = GlobalLock(handle as HGLOBAL);
            if ptr.is_null() {
                return None;
            }
            let data = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
            GlobalUnlock(handle as HGLOBAL);
            Some(data)
        })();
        CloseClipboard();
        result
    }
}

fn read_clipboard_image() -> Option<(u64, String)> {
    let dib = read_clipboard_dib()?;
    let sig = fnv1a64(&dib);
    let bmp = dib_to_bmp(&dib);
    if let Ok(img) = image::load_from_memory(&bmp) {
        let scaled = if img.width() > 960 || img.height() > 960 {
            img.resize(960, 960, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut png_buf = Vec::new();
        let mut cur = std::io::Cursor::new(&mut png_buf);
        if scaled.write_to(&mut cur, image::ImageFormat::Png).is_ok() {
            let data_url = format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&png_buf));
            return Some((sig, data_url));
        }
    }
    let data_url = format!("data:image/bmp;base64,{}", BASE64_STANDARD.encode(&bmp));
    Some((sig, data_url))
}

fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let offset = bmp_pixel_offset(dib);
    let file_size = (14 + dib.len()) as u32;
    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(dib);
    out
}

fn bmp_pixel_offset(dib: &[u8]) -> u32 {
    const BI_BITFIELDS: u32 = 3;
    if dib.len() < 12 {
        return 54;
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]);
    if header_size == 12 {
        return 14 + 12;
    }
    if header_size < 40 || dib.len() < 40 {
        return 54;
    }
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as u32;
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]);

    let mut offset = 14 + header_size;
    if compression == BI_BITFIELDS && header_size == 40 {
        offset += 12;
    }
    if bit_count > 0 && bit_count <= 8 {
        let entries = if clr_used == 0 { 1u32 << bit_count } else { clr_used };
        offset += entries * 4;
    }
    offset
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn copy_text_impl(text: &str, suppress_watcher: bool) -> bool {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        let ok = (|| -> bool {
            if EmptyClipboard() == 0 {
                return false;
            }
            let mut utf16: Vec<u16> = text.encode_utf16().collect();
            utf16.push(0);
            let byte_len = utf16.len() * 2;
            let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
            if hmem.is_null() {
                return false;
            }
            let dst = GlobalLock(hmem);
            if dst.is_null() {
                GlobalFree(hmem);
                return false;
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, dst as *mut u8, byte_len);
            GlobalUnlock(hmem);
            if SetClipboardData(CF_UNICODETEXT as u32, hmem as HANDLE).is_null() {
                GlobalFree(hmem);
                return false;
            }
            if suppress_watcher {
                let seq = GetClipboardSequenceNumber();
                SELF_COPY_SEQ.store(seq as u64 + 1, Ordering::SeqCst);
            }
            true
        })();
        CloseClipboard();
        ok
    }
}

pub fn copy_text(text: &str) -> bool {
    copy_text_impl(text, false)
}

pub fn copy_text_silent(text: &str) -> bool {
    copy_text_impl(text, true)
}

fn copy_image_bmp_impl(bmp_bytes: &[u8], suppress_watcher: bool) -> bool {
    let dib: &[u8] = if bmp_bytes.len() > 14 && &bmp_bytes[0..2] == b"BM" {
        &bmp_bytes[14..]
    } else {
        bmp_bytes
    };
    if dib.is_empty() {
        return false;
    }
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        let ok = (|| -> bool {
            if EmptyClipboard() == 0 {
                return false;
            }
            let hmem = GlobalAlloc(GMEM_MOVEABLE, dib.len());
            if hmem.is_null() {
                return false;
            }
            let dst = GlobalLock(hmem);
            if dst.is_null() {
                GlobalFree(hmem);
                return false;
            }
            std::ptr::copy_nonoverlapping(dib.as_ptr(), dst as *mut u8, dib.len());
            GlobalUnlock(hmem);
            if SetClipboardData(CF_DIB as u32, hmem as HANDLE).is_null() {
                GlobalFree(hmem);
                return false;
            }
            if suppress_watcher {
                let seq = GetClipboardSequenceNumber();
                SELF_COPY_SEQ.store(seq as u64 + 1, Ordering::SeqCst);
            }
            true
        })();
        CloseClipboard();
        ok
    }
}

pub fn copy_image_bmp(bmp_bytes: &[u8]) -> bool {
    copy_image_bmp_impl(bmp_bytes, false)
}

fn copy_image_bytes_impl(bytes: &[u8], suppress_watcher: bool) -> bool {
    if bytes.len() > 14 && &bytes[0..2] == b"BM" {
        return copy_image_bmp_impl(bytes, suppress_watcher);
    }
    if let Ok(img) = image::load_from_memory(bytes) {
        let mut bmp_buf = Vec::new();
        let mut cur = std::io::Cursor::new(&mut bmp_buf);
        if img.write_to(&mut cur, image::ImageFormat::Bmp).is_ok() {
            return copy_image_bmp_impl(&bmp_buf, suppress_watcher);
        }
    }
    false
}

pub fn copy_image_bytes(bytes: &[u8]) -> bool {
    copy_image_bytes_impl(bytes, false)
}

pub fn copy_image_bytes_silent(bytes: &[u8]) -> bool {
    copy_image_bytes_impl(bytes, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone)]
pub struct PastedAttachment {
    pub kind: AttachmentKind,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}

pub fn is_image_ext(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff"
    )
}

pub fn read_clipboard_files() -> Vec<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Vec::new();
        }
        let files = (|| -> Vec<String> {
            let handle = GetClipboardData(CF_HDROP as u32);
            if handle.is_null() {
                return Vec::new();
            }
            let hdrop = handle as HDROP;
            let count = DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0);
            let mut out = Vec::new();
            for i in 0..count.min(6) {
                let len = DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0);
                if len == 0 || len > 32_760 {
                    continue;
                }
                let mut buf = vec![0u16; (len + 1) as usize];
                let copied = DragQueryFileW(hdrop, i, buf.as_mut_ptr(), len + 1);
                if copied > 0 {
                    let s = String::from_utf16_lossy(&buf[..copied as usize]);
                    if !s.trim().is_empty() {
                        out.push(s);
                    }
                }
            }
            out
        })();
        CloseClipboard();
        files
    }
}

pub fn read_clipboard_attachments() -> Option<Vec<PastedAttachment>> {
    let files = read_clipboard_files();
    if !files.is_empty() {
        let mut out = Vec::new();
        for p in files {
            let Ok(meta) = std::fs::metadata(&p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = std::path::Path::new(&p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let kind = if is_image_ext(&p) {
                AttachmentKind::Image
            } else {
                AttachmentKind::File
            };
            out.push(PastedAttachment {
                kind,
                name,
                path: p,
                size_bytes: meta.len(),
            });
        }
        if !out.is_empty() {
            return Some(out);
        }
    }

    let dib = read_clipboard_dib()?;
    let bmp = dib_to_bmp(&dib);
    let img = image::load_from_memory(&bmp).ok()?;
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let name = format!("image-{}.png", ts_ms % 10000);
    let out_path = std::env::temp_dir().join(format!("selabar-paste-{}.png", ts_ms));
    let mut png_buf = Vec::new();
    let mut cur = std::io::Cursor::new(&mut png_buf);
    img.write_to(&mut cur, image::ImageFormat::Png).ok()?;
    std::fs::write(&out_path, &png_buf).ok()?;
    Some(vec![PastedAttachment {
        kind: AttachmentKind::Image,
        name,
        path: out_path.to_string_lossy().to_string(),
        size_bytes: png_buf.len() as u64,
    }])
}

pub fn data_url_to_dib(data_url: &str) -> Option<Vec<u8>> {
    let b64 = data_url.split_once("base64,")?.1;
    let raw = BASE64_STANDARD.decode(b64.trim()).ok()?;
    if raw.len() > 18 && &raw[0..2] == b"BM" {
        return Some(raw[14..].to_vec());
    }
    if let Ok(img) = image::load_from_memory(&raw) {
        let mut bmp_buf = Vec::new();
        let mut cur = std::io::Cursor::new(&mut bmp_buf);
        if img.write_to(&mut cur, image::ImageFormat::Bmp).is_ok() && bmp_buf.len() > 18 {
            return Some(bmp_buf[14..].to_vec());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::is_image_ext;

    #[test]
    fn detects_image_vs_document_extensions() {
        assert!(is_image_ext("C:\\Users\\Demo\\Pictures\\foto.PNG"));
        assert!(is_image_ext("screenshot.jpg"));
        assert!(is_image_ext("avatar.webp"));
        assert!(!is_image_ext("C:\\Docs\\laporan.pdf"));
        assert!(!is_image_ext("data.xlsx"));
        assert!(!is_image_ext("catatan.docx"));
        assert!(!is_image_ext("src/main.rs"));
        assert!(!is_image_ext("Makefile"));
    }
}
