use crate::events::{UiEvent, UiSender};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct ScreenshotWatcher {
    running: Arc<AtomicBool>,
}

impl ScreenshotWatcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, ui: UiSender) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = Arc::clone(&self.running);

        std::thread::spawn(move || {
            let watch_dirs: Vec<PathBuf> = screenshot_base_dirs()
                .into_iter()
                .filter(|d| d.exists())
                .collect();
            if watch_dirs.is_empty() {
                return;
            }

            let mut known_files: HashSet<PathBuf> = HashSet::new();
            for dir in &watch_dirs {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        known_files.insert(entry.path());
                    }
                }
            }

            while running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(700));
                for dir in &watch_dirs {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            let ext = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_lowercase();
                            if !is_screenshot_ext(&ext) {
                                continue;
                            }
                            if !known_files.contains(&path) && wait_for_file_stable(&path) {
                                known_files.insert(path.clone());
                                ui.send(UiEvent::ScreenshotNew(
                                    path.to_string_lossy().to_string(),
                                ));
                            }
                        }
                    }
                }
            }
        });
    }
}

fn wait_for_file_stable(path: &Path) -> bool {
    let mut last_size = 0u64;
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(meta) = path.metadata() {
            let sz = meta.len();
            if sz > 0 && sz == last_size {
                return true;
            }
            last_size = sz;
        }
    }
    last_size > 0
}

pub fn screenshot_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(user_profile).join("Pictures").join("Screenshots"));
    }
    if let Ok(one_drive) = std::env::var("OneDrive") {
        let p = PathBuf::from(one_drive).join("Pictures").join("Screenshots");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

fn is_screenshot_ext(ext: &str) -> bool {
    matches!(ext.to_lowercase().as_str(), "png" | "jpg" | "jpeg")
}

fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
    let s = p.as_os_str().to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{}", rest));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

pub fn validate_screenshot_path(raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("empty screenshot path".to_string());
    }
    let target = std::fs::canonicalize(raw)
        .map_err(|e| format!("screenshot path not found: {} ({})", raw, e))?;
    let canonical_bases: Vec<PathBuf> = screenshot_base_dirs()
        .iter()
        .filter_map(|b| std::fs::canonicalize(b).ok())
        .collect();
    if canonical_bases.is_empty() {
        return Err("no Screenshots directory available".to_string());
    }
    if !canonical_bases.iter().any(|b| target.starts_with(b)) {
        return Err("screenshot path escapes the Screenshots directory".to_string());
    }
    let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !is_screenshot_ext(ext) {
        return Err(format!("not a screenshot extension: {}", ext));
    }
    Ok(target)
}

pub fn sanitize_filename(name: &str) -> Result<String, String> {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        return Err("filename is empty".to_string());
    }
    if cleaned.contains('/')
        || cleaned.contains('\\')
        || cleaned.contains("..")
        || cleaned.contains(':')
        || cleaned.contains('\0')
    {
        return Err("filename contains path separators or traversal".to_string());
    }
    if Path::new(cleaned).file_name().and_then(|f| f.to_str()) != Some(cleaned) {
        return Err("filename is not a single plain name".to_string());
    }
    if cleaned.ends_with('.') || cleaned.ends_with(' ') {
        return Err("filename ends with '.' or space".to_string());
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return Err("filename is a reserved device name".to_string());
    }
    Ok(cleaned.to_string())
}

pub fn discard_screenshot(path: &str) -> Result<bool, String> {
    if path.is_empty() {
        return Ok(false);
    }
    let valid = validate_screenshot_path(path)?;
    std::fs::remove_file(&valid).map_err(|e| format!("Failed to delete screenshot: {}", e))?;
    Ok(true)
}

pub fn rename_screenshot(path: &str, new_name: &str) -> Result<String, String> {
    let old_p = validate_screenshot_path(path)?;
    let name = sanitize_filename(new_name)?;
    let ext = old_p.extension().and_then(|e| e.to_str()).unwrap_or("");
    let new_filename = if ext.is_empty() || name.ends_with(&format!(".{}", ext)) {
        name
    } else {
        format!("{}.{}", name, ext)
    };
    let parent = old_p
        .parent()
        .ok_or_else(|| "screenshot has no parent directory".to_string())?;
    let new_path = parent.join(new_filename);
    std::fs::rename(&old_p, &new_path)
        .map_err(|e| format!("Failed to rename screenshot: {}", e))?;
    Ok(strip_verbatim_prefix(new_path).to_string_lossy().to_string())
}
