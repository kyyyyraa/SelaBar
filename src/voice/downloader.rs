use crate::events::{UiEvent, UiSender};
use crate::voice::whisper;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const CPU_ZIP_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5907/whisper-bin-x64.zip";
pub const CUDA_ZIP_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5907/whisper-bin-win-cuda-12.4.0-x64.zip";
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin";
pub const MODEL_FILE: &str = "ggml-small-q5_1.bin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperBackendKind {
    Cpu,
    Cuda,
}

impl WhisperBackendKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda => "CUDA",
        }
    }

    pub fn folder_name(self) -> &'static str {
        match self {
            Self::Cpu => "whisper-cpu",
            Self::Cuda => "whisper-cuda",
        }
    }

    pub fn zip_url(self) -> &'static str {
        match self {
            Self::Cpu => CPU_ZIP_URL,
            Self::Cuda => CUDA_ZIP_URL,
        }
    }
}

/// Fast, non-blocking check for NVIDIA GPU presence (CUDA driver or `nvidia-smi`).
pub fn has_nvidia_gpu() -> bool {
    static DETECTED: OnceLock<bool> = OnceLock::new();
    *DETECTED.get_or_init(|| {
        [
            r"C:\Windows\System32\nvcuda.dll",
            r"C:\Windows\System32\nvidia-smi.exe",
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe",
        ]
        .iter()
        .any(|p| Path::new(p).is_file())
    })
}

pub fn install_root() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            return PathBuf::from(appdata).join("SelaBar");
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("SelaBar");
        }
    }
    PathBuf::from(".").join("SelaBar")
}

/// Recursively copies all `.exe` and `.dll` files from `src_dir` flat into `dst_dir`.
pub fn flatten_runtime_binaries(src_dir: &Path, dst_dir: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dst_dir)
        .map_err(|e| format!("gagal membuat folder {:?}: {}", dst_dir, e))?;

    let mut stack = vec![src_dir.to_path_buf()];
    let mut copied = 0usize;

    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("gagal membaca hasil ekstrak {:?}: {}", dir, e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let is_bin = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("dll"))
                .unwrap_or(false);
            if is_bin {
                if let Some(name) = path.file_name() {
                    let target = dst_dir.join(name);
                    std::fs::copy(&path, &target)
                        .map_err(|e| format!("gagal menyalin {:?}: {}", name, e))?;
                    copied += 1;
                }
            }
        }
    }

    if copied == 0 {
        return Err("Arsip runtime tidak berisi file .exe / .dll".to_string());
    }
    Ok(copied)
}

fn find_tar_exe() -> &'static str {
    if Path::new(r"C:\Windows\System32\tar.exe").is_file() {
        r"C:\Windows\System32\tar.exe"
    } else {
        "tar.exe"
    }
}

pub struct WhisperDownloader {
    active: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Default for WhisperDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperDownloader {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_downloading(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn start(&self, kind: WhisperBackendKind, rt: &tokio::runtime::Handle, ui: UiSender) {
        if self
            .active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);

        let active = Arc::clone(&self.active);
        let cancel = Arc::clone(&self.cancel);
        rt.spawn(async move {
            let res = run_download_job(kind, &cancel, &ui).await;
            active.store(false, Ordering::SeqCst);
            match res {
                Ok(msg) => ui.send(UiEvent::WhisperDownloadDone { ok: true, msg }),
                Err(DownloadOutcome::Cancelled) => ui.send(UiEvent::WhisperDownloadDone {
                    ok: false,
                    msg: "Unduhan dibatalkan.".to_string(),
                }),
                Err(DownloadOutcome::Failed(err)) => ui.send(UiEvent::WhisperDownloadDone {
                    ok: false,
                    msg: format!("Gagal mengunduh: {}", err),
                }),
            }
        });
    }
}

enum DownloadOutcome {
    Cancelled,
    Failed(String),
}

async fn download_file_streaming(
    client: &reqwest::Client,
    url: &str,
    dest_part: &Path,
    stage: &str,
    fallback_total_mb: f32,
    cancel: &AtomicBool,
    ui: &UiSender,
) -> Result<(), DownloadOutcome> {
    if cancel.load(Ordering::SeqCst) {
        return Err(DownloadOutcome::Cancelled);
    }

    if let Some(parent) = dest_part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadOutcome::Failed(format!("buat folder: {}", e)))?;
    }

    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadOutcome::Failed(format!("koneksi: {}", e)))?;

    if !resp.status().is_success() {
        return Err(DownloadOutcome::Failed(format!(
            "HTTP {}",
            resp.status().as_u16()
        )));
    }

    let total_bytes = resp.content_length().unwrap_or(0);
    let total_mb = if total_bytes > 0 {
        total_bytes as f32 / (1024.0 * 1024.0)
    } else {
        fallback_total_mb
    };

    let mut file = tokio::fs::File::create(dest_part)
        .await
        .map_err(|e| DownloadOutcome::Failed(format!("tulis file: {}", e)))?;

    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now() - Duration::from_millis(200);

    ui.send(UiEvent::WhisperDownloadProgress {
        stage: stage.to_string(),
        downloaded_mb: 0.0,
        total_mb,
        pct: 0.0,
    });

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| DownloadOutcome::Failed(format!("baca stream: {}", e)))?
    {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = tokio::fs::remove_file(dest_part).await;
            return Err(DownloadOutcome::Cancelled);
        }

        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(dest_part).await;
            return Err(DownloadOutcome::Failed(format!("simpan data: {}", e)));
        }

        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(120) {
            last_emit = Instant::now();
            let dl_mb = downloaded as f32 / (1024.0 * 1024.0);
            let pct = if total_mb > 0.01 {
                (dl_mb / total_mb).clamp(0.0, 0.99)
            } else {
                0.0
            };
            ui.send(UiEvent::WhisperDownloadProgress {
                stage: stage.to_string(),
                downloaded_mb: dl_mb,
                total_mb,
                pct,
            });
        }
    }

    if let Err(e) = file.flush().await {
        drop(file);
        let _ = tokio::fs::remove_file(dest_part).await;
        return Err(DownloadOutcome::Failed(format!("flush file: {}", e)));
    }
    drop(file);

    let final_mb = downloaded as f32 / (1024.0 * 1024.0);
    ui.send(UiEvent::WhisperDownloadProgress {
        stage: stage.to_string(),
        downloaded_mb: final_mb,
        total_mb: total_mb.max(final_mb),
        pct: 1.0,
    });

    Ok(())
}

async fn run_download_job(
    kind: WhisperBackendKind,
    cancel: &AtomicBool,
    ui: &UiSender,
) -> Result<String, DownloadOutcome> {
    let need_cli = match kind {
        WhisperBackendKind::Cuda => !whisper::has_cuda_cli(),
        WhisperBackendKind::Cpu => !whisper::has_cpu_cli(),
    };
    let need_model = !whisper::has_model();

    let total_steps = (need_cli as usize) + (need_model as usize);
    if total_steps == 0 {
        return Ok(format!("Model suara {} sudah terpasang.", kind.label()));
    }

    let root = install_root();
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| DownloadOutcome::Failed(format!("buat folder SelaBar: {}", e)))?;

    let client = reqwest::Client::builder()
        .user_agent("SelaBar/0.2.0")
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| DownloadOutcome::Failed(format!("http client: {}", e)))?;

    let mut step_num = 0usize;

    if need_cli {
        step_num += 1;
        let zip_part = root.join("_whisper_dl.zip.part");
        let zip_final = root.join("_whisper_dl.zip");
        let extract_tmp = root.join("_whisper_extract_tmp");
        let _ = tokio::fs::remove_file(&zip_part).await;
        let _ = tokio::fs::remove_file(&zip_final).await;
        let _ = tokio::fs::remove_dir_all(&extract_tmp).await;

        let stage = format!(
            "({}/{}) Mengunduh runtime Whisper {}...",
            step_num,
            total_steps,
            kind.label()
        );
        let fallback_mb = match kind {
            WhisperBackendKind::Cpu => 8.8,
            WhisperBackendKind::Cuda => 701.5,
        };

        if let Err(e) = download_file_streaming(
            &client,
            kind.zip_url(),
            &zip_part,
            &stage,
            fallback_mb,
            cancel,
            ui,
        )
        .await
        {
            let _ = tokio::fs::remove_file(&zip_part).await;
            return Err(e);
        }

        if cancel.load(Ordering::SeqCst) {
            let _ = tokio::fs::remove_file(&zip_part).await;
            return Err(DownloadOutcome::Cancelled);
        }

        tokio::fs::rename(&zip_part, &zip_final)
            .await
            .map_err(|e| DownloadOutcome::Failed(format!("simpan arsip zip: {}", e)))?;

        ui.send(UiEvent::WhisperDownloadProgress {
            stage: "Mengekstrak runtime Whisper...".to_string(),
            downloaded_mb: fallback_mb,
            total_mb: fallback_mb,
            pct: 0.99,
        });

        let zip_for_extract = zip_final.clone();
        let tmp_for_extract = extract_tmp.clone();
        let dst_dir = root.join(kind.folder_name());

        let extract_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
            std::fs::create_dir_all(&tmp_for_extract)
                .map_err(|e| format!("buat folder ekstrak: {}", e))?;

            let out = std::process::Command::new(find_tar_exe())
                .arg("-xf")
                .arg(&zip_for_extract)
                .arg("-C")
                .arg(&tmp_for_extract)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| format!("jalankan tar.exe: {}", e))?;

            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(format!("ekstrak zip gagal: {}", err));
            }

            flatten_runtime_binaries(&tmp_for_extract, &dst_dir)?;
            let has_exe =
                dst_dir.join("whisper-cli.exe").is_file() || dst_dir.join("main.exe").is_file();
            if !has_exe {
                return Err("whisper-cli.exe tidak ditemukan setelah ekstrak".to_string());
            }
            Ok(())
        })
        .await
        .map_err(|e| DownloadOutcome::Failed(format!("ekstrak task: {}", e)))?;

        let _ = tokio::fs::remove_file(&zip_final).await;
        let _ = tokio::fs::remove_dir_all(&extract_tmp).await;

        if let Err(err) = extract_res {
            return Err(DownloadOutcome::Failed(err));
        }

        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadOutcome::Cancelled);
        }
    }

    if need_model {
        step_num += 1;
        let models_dir = root.join("models");
        tokio::fs::create_dir_all(&models_dir)
            .await
            .map_err(|e| DownloadOutcome::Failed(format!("buat folder models: {}", e)))?;

        let model_part = models_dir.join(format!("{}.part", MODEL_FILE));
        let model_final = models_dir.join(MODEL_FILE);
        let _ = tokio::fs::remove_file(&model_part).await;

        let stage = format!(
            "({}/{}) Mengunduh model suara ({})...",
            step_num, total_steps, MODEL_FILE
        );

        if let Err(e) = download_file_streaming(
            &client,
            MODEL_URL,
            &model_part,
            &stage,
            181.3,
            cancel,
            ui,
        )
        .await
        {
            let _ = tokio::fs::remove_file(&model_part).await;
            return Err(e);
        }

        if cancel.load(Ordering::SeqCst) {
            let _ = tokio::fs::remove_file(&model_part).await;
            return Err(DownloadOutcome::Cancelled);
        }

        tokio::fs::rename(&model_part, &model_final)
            .await
            .map_err(|e| DownloadOutcome::Failed(format!("simpan model: {}", e)))?;
    }

    if !whisper::is_available() {
        return Err(DownloadOutcome::Failed(
            "Unduhan selesai, namun file runtime atau model belum terdeteksi.".to_string(),
        ));
    }

    Ok(format!("Model suara {} siap dipakai!", kind.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_runtime_binaries_extracts_nested_exe_and_dll() {
        let base = std::env::temp_dir().join(format!("selabar-flatten-test-{}", std::process::id()));
        let src = base.join("src");
        let nested = src.join("Release");
        let dst = base.join("dst");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&nested).unwrap();

        std::fs::write(nested.join("whisper-cli.exe"), b"exe").unwrap();
        std::fs::write(nested.join("whisper.dll"), b"dll").unwrap();
        std::fs::write(src.join("README.txt"), b"txt").unwrap();

        let count = flatten_runtime_binaries(&src, &dst).expect("flatten succeeds");
        assert_eq!(count, 2);
        assert!(dst.join("whisper-cli.exe").is_file());
        assert!(dst.join("whisper.dll").is_file());
        assert!(!dst.join("README.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
