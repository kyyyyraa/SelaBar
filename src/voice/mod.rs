pub mod downloader;
pub mod dsp;
pub mod edge_tts;
pub mod hotkey;
pub mod mic;
pub mod tts;
pub mod vad;
pub mod whisper;

use crate::events::{PttMode, UiEvent, UiSender};
use downloader::WhisperDownloader;
use hotkey::HotkeyManager;
use mic::MicCapture;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tts::WinRtTts;
use vad::{normalize_audio, trim_silence, VadConfig, VadFinishReason, VoiceActivityDetector};

type DoneCallback = Box<dyn FnOnce(u64, Vec<f32>, VadFinishReason) + Send + 'static>;

pub struct VoiceManager {
    pub tts: Option<WinRtTts>,
    pub hotkeys: HotkeyManager,
    pub downloader: Arc<WhisperDownloader>,
    current_mic: Arc<Mutex<Option<MicCapture>>>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    pending_done: Arc<Mutex<Option<DoneCallback>>>,
    session_gen: Arc<AtomicU64>,
    recording_gen: Arc<AtomicU64>,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    active_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

impl VoiceManager {
    pub fn new() -> Self {
        let tts = match WinRtTts::new() {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("[SelaBar] WinRT TTS unavailable: {:?}", e);
                None
            }
        };
        let hotkeys = HotkeyManager::new();
        Self {
            tts,
            hotkeys,
            downloader: Arc::new(WhisperDownloader::new()),
            current_mic: Arc::new(Mutex::new(None)),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            pending_done: Arc::new(Mutex::new(None)),
            session_gen: Arc::new(AtomicU64::new(0)),
            recording_gen: Arc::new(AtomicU64::new(0)),
            active_cancel: Arc::new(Mutex::new(None)),
            active_abort: Arc::new(Mutex::new(None)),
        }
    }

    pub fn list_voices(&self) -> Vec<serde_json::Value> {
        self.tts
            .as_ref()
            .map(|t| t.list_voices())
            .unwrap_or_default()
    }

    pub fn voice_infos(&self) -> Vec<tts::VoiceInfo> {
        self.tts
            .as_ref()
            .map(|t| t.voice_infos())
            .unwrap_or_default()
    }

    pub fn active_voice_label(&self) -> String {
        self.tts
            .as_ref()
            .map(|t| t.active_voice_label())
            .unwrap_or_else(|| "Tidak ada suara sistem".to_string())
    }

    pub fn has_indonesian_voice(&self) -> bool {
        self.tts
            .as_ref()
            .map(|t| t.has_indonesian_voice())
            .unwrap_or(false)
    }

    pub fn set_voice_preference(&self, preferred: &str) {
        if let Some(t) = self.tts.as_ref() {
            t.apply_voice_preference(preferred);
        }
    }

    pub fn speak(&self, text: &str) -> Result<u64, String> {
        let Some(tts) = self.tts.as_ref() else {
            return Err("TTS tidak tersedia di sistem ini".to_string());
        };
        self.hotkeys.set_escape_captured(true);
        tts.speak(text)
            .map_err(|e| format!("TTS synthesis error: {:?}", e))
    }

    pub fn test_speak(&self, text: &str) -> Result<u64, String> {
        let Some(tts) = self.tts.as_ref() else {
            return Err("TTS tidak tersedia di sistem ini".to_string());
        };
        self.hotkeys.set_escape_captured(true);
        tts.speak_with_options(text, true)
            .map_err(|e| format!("TTS synthesis error: {:?}", e))
    }

    pub fn is_speaking(&self) -> bool {
        self.tts.as_ref().map(|t| t.is_speaking()).unwrap_or(false)
    }

    pub fn stop_speech(&self) {
        if let Some(tts) = self.tts.as_ref() {
            tts.stop();
        }
    }

    pub fn is_recording(&self) -> bool {
        self.current_mic
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Starts native `cpal` 16kHz recording + adaptive VAD. Returns captured samples when finished.
    pub fn start_recording<FDone>(
        &self,
        mode: PttMode,
        ui: UiSender,
        on_done: FDone,
    ) -> Result<u64, String>
    where
        FDone: FnOnce(u64, Vec<f32>, VadFinishReason) + Send + 'static,
    {
        self.start_recording_with_timeout(mode, 8, ui, on_done)
    }

    pub fn start_recording_with_timeout<FDone>(
        &self,
        mode: PttMode,
        no_speech_secs: u64,
        ui: UiSender,
        on_done: FDone,
    ) -> Result<u64, String>
    where
        FDone: FnOnce(u64, Vec<f32>, VadFinishReason) + Send + 'static,
    {
        self.abort_recording();
        let gen = self.session_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.recording_gen.store(gen, Ordering::SeqCst);
        self.hotkeys.set_busy(true);
        self.hotkeys.set_escape_captured(true);

        let silence_ms = match mode {
            PttMode::Dictate => 2500,
            PttMode::Command => 1600,
        };
        let hard_cap_s = match mode {
            PttMode::Dictate => 90,
            PttMode::Command => 25,
        };

        let vad_config = VadConfig {
            min_threshold: 0.006,
            min_speech_chunks: 2,
            no_speech_timeout: std::time::Duration::from_secs(no_speech_secs.max(3)),
            silence_duration: std::time::Duration::from_millis(silence_ms),
            hard_cap_duration: std::time::Duration::from_secs(hard_cap_s),
        };

        let mut detector = VoiceActivityDetector::new(vad_config);
        let buffer = Arc::clone(&self.audio_buffer);
        if let Ok(mut buf) = buffer.lock() {
            *buf = Vec::new();
        }

        if let Ok(mut guard) = self.pending_done.lock() {
            *guard = Some(Box::new(on_done));
        }
        let done_cell = Arc::clone(&self.pending_done);
        let buffer_clone = Arc::clone(&buffer);
        let current_mic_ref = Arc::clone(&self.current_mic);
        let ui_c = ui.clone();

        let capture = MicCapture::start(move |chunk| {
            let (rms, _is_speech, finished) = detector.process_chunk(chunk);

            // Compute 5-bar visual level (scaled smoothly like dn-voice-ptt.js)
            let lv = (rms * 18.0).clamp(0.0, 1.0);
            ui_c.send(UiEvent::VoiceRms([
                (lv * 0.55).clamp(0.05, 1.0),
                (lv * 0.90).clamp(0.05, 1.0),
                (lv * 1.00).clamp(0.05, 1.0),
                (lv * 0.82).clamp(0.05, 1.0),
                (lv * 0.48).clamp(0.05, 1.0),
            ]));

            if let Ok(mut buf) = buffer_clone.lock() {
                buf.extend_from_slice(chunk);
            }

            if let Some(reason) = finished {
                if let Ok(mut done_guard) = done_cell.lock() {
                    if let Some(cb) = done_guard.take() {
                        if let Ok(mut mg) = current_mic_ref.lock() {
                            if let Some(cap) = mg.take() {
                                cap.stop();
                                // Drop cpal::Stream off the cpal_wasapi_in callback thread to avoid self-join deadlock
                                std::thread::spawn(move || drop(cap));
                            }
                        }
                        let samples = buffer_clone
                            .lock()
                            .map(|mut b| std::mem::take(&mut *b))
                            .unwrap_or_default();
                        cb(gen, samples, reason);
                    }
                }
            }
        })?;

        if let Ok(mut guard) = self.current_mic.lock() {
            *guard = Some(capture);
        }

        Ok(gen)
    }

    pub fn current_generation(&self) -> u64 {
        self.session_gen.load(Ordering::SeqCst)
    }

    pub fn cancel_active_task(&self) {
        if let Ok(mut guard) = self.active_cancel.lock() {
            if let Some(flag) = guard.take() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(mut guard) = self.active_abort.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }

    pub fn begin_voice_task(&self, gen: u64) -> Arc<AtomicBool> {
        self.cancel_active_task();
        if self.session_gen.load(Ordering::SeqCst) != gen {
            return Arc::new(AtomicBool::new(true));
        }
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.active_cancel.lock() {
            *guard = Some(Arc::clone(&flag));
        }
        if self.session_gen.load(Ordering::SeqCst) != gen {
            flag.store(true, Ordering::SeqCst);
        }
        flag
    }

    pub fn attach_voice_abort(&self, gen: u64, handle: tokio::task::AbortHandle) {
        if self.session_gen.load(Ordering::SeqCst) != gen {
            handle.abort();
            return;
        }
        if let Ok(mut guard) = self.active_abort.lock() {
            if let Some(prev) = guard.replace(handle) {
                prev.abort();
            }
        }
        if self.session_gen.load(Ordering::SeqCst) != gen {
            if let Ok(mut guard) = self.active_abort.lock() {
                if let Some(h) = guard.take() {
                    h.abort();
                }
            }
        }
    }

    pub fn cancel_all_voice(&self) {
        self.hotkeys.set_live(false);
        self.hotkeys.set_busy(false);
        self.hotkeys.set_escape_captured(false);
        self.abort_recording();
        self.stop_speech();
    }

    /// Gracefully stops active capture (when user toggles PTT or clicks Stop Mic) and transcribes captured speech.
    pub fn finish_recording(&self) {
        self.hotkeys.set_busy(false);
        if let Ok(mut guard) = self.current_mic.lock() {
            if let Some(cap) = guard.take() {
                cap.stop();
                drop(cap);
            }
        }
        let gen = self.recording_gen.load(Ordering::SeqCst);
        let cb_opt = self.pending_done.lock().ok().and_then(|mut g| g.take());
        if let Some(cb) = cb_opt {
            let samples = self
                .audio_buffer
                .lock()
                .map(|mut b| std::mem::take(&mut *b))
                .unwrap_or_default();
            cb(gen, samples, VadFinishReason::SilenceTimeout);
        }
    }

    pub fn abort_recording(&self) {
        self.session_gen.fetch_add(1, Ordering::SeqCst);
        self.cancel_active_task();
        self.hotkeys.set_busy(false);
        if let Ok(mut guard) = self.pending_done.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.current_mic.lock() {
            if let Some(cap) = guard.take() {
                cap.stop();
                drop(cap);
            }
        }
        if let Ok(mut buf) = self.audio_buffer.lock() {
            *buf = Vec::new();
        }
    }

    /// Drops retained PCM/WAV buffers once recording & TTS playback are idle.
    pub fn release_idle_memory(&self) {
        if !self.is_recording() {
            if let Ok(mut buf) = self.audio_buffer.lock() {
                *buf = Vec::new();
            }
        }
        if let Some(tts) = self.tts.as_ref() {
            tts.release_if_idle();
        }
    }

    /// Runs a 4.5-second Settings microphone + Whisper self-test.
    pub fn run_mic_test(&self, rt: &tokio::runtime::Handle, ui: UiSender) {
        self.abort_recording();
        let ui_c = ui.clone();
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let buf_c = Arc::clone(&buffer);
        let ui_lv = ui.clone();

        let cap = match MicCapture::start(move |chunk| {
            let rms = VoiceActivityDetector::calculate_rms(chunk);
            let lv = (rms * 18.0).clamp(0.0, 1.0);
            ui_lv.send(UiEvent::VoiceRms([
                (lv * 0.55).clamp(0.05, 1.0),
                (lv * 0.90).clamp(0.05, 1.0),
                (lv * 1.00).clamp(0.05, 1.0),
                (lv * 0.82).clamp(0.05, 1.0),
                (lv * 0.48).clamp(0.05, 1.0),
            ]));
            if let Ok(mut b) = buf_c.lock() {
                b.extend_from_slice(chunk);
            }
        }) {
            Ok(c) => c,
            Err(e) => {
                ui.send(UiEvent::VoiceMicTestResult { ok: false, text: e });
                return;
            }
        };

        rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(4500)).await;
            cap.stop();
            let raw = buffer
                .lock()
                .map(|mut b| std::mem::take(&mut *b))
                .unwrap_or_default();
            let trimmed = trim_silence(&raw, 16000);
            let (norm, peak) = normalize_audio(&trimmed, 0.85);
            if peak < 0.0005 {
                ui_c.send(UiEvent::VoiceMicTestResult {
                    ok: false,
                    text: "Suara terlalu pelan / tidak terdengar".to_string(),
                });
                return;
            }
            match whisper::transcribe_samples(norm, "id".to_string()).await {
                Ok((txt, _)) => ui_c.send(UiEvent::VoiceMicTestResult {
                    ok: true,
                    text: if txt.is_empty() {
                        "(tidak ada teks)".to_string()
                    } else {
                        txt
                    },
                }),
                Err(e) => ui_c.send(UiEvent::VoiceMicTestResult { ok: false, text: e }),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_all_voice_bumps_generation_and_flips_cancel_flag() {
        let vm = VoiceManager::new();
        let g0 = vm.current_generation();
        let flag = vm.begin_voice_task(g0);
        assert!(!flag.load(Ordering::SeqCst));

        vm.cancel_all_voice();
        assert!(flag.load(Ordering::SeqCst));
        assert!(vm.current_generation() > g0);

        // Stale generation cannot arm a live task flag
        let stale_flag = vm.begin_voice_task(g0);
        assert!(stale_flag.load(Ordering::SeqCst));
    }
}
