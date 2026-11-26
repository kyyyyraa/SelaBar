use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::oneshot;
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

static CHIME_BUFFER: OnceLock<Mutex<Option<Vec<u8>>>> = OnceLock::new();

fn chime_slot() -> &'static Mutex<Option<Vec<u8>>> {
    CHIME_BUFFER.get_or_init(|| Mutex::new(None))
}

#[derive(Clone)]
pub struct ApprovalGate {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    counter: Arc<Mutex<u64>>,
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
        }
    }

    fn next_id(&self) -> String {
        let mut c = self.counter.lock().unwrap_or_else(|e| e.into_inner());
        *c += 1;
        format!("dn-local-{}-{}", std::process::id(), *c)
    }

    pub fn register(&self) -> (String, oneshot::Receiver<bool>) {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tx);
        (id, rx)
    }

    pub fn respond(&self, id: &str, decision: &str) -> bool {
        let allowed = matches!(decision, "allow" | "approve" | "yes" | "true" | "ok");
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.remove(id) {
            let _ = tx.send(allowed);
            true
        } else {
            false
        }
    }

    pub fn cancel(&self, id: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    pub fn has_pending(&self, id: &str) -> bool {
        self.pending
            .lock()
            .map(|g| g.contains_key(id))
            .unwrap_or(false)
    }

    pub fn deny_all(&self) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for (_, tx) in guard.drain() {
            let _ = tx.send(false);
        }
    }
}

pub async fn await_decision(
    gate: &ApprovalGate,
    id: String,
    rx: oneshot::Receiver<bool>,
    timeout_secs: u64,
) -> bool {
    match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
        Ok(Ok(allowed)) => allowed,
        Ok(Err(_)) | Err(_) => {
            gate.cancel(&id);
            false
        }
    }
}

fn synth_wav(notes: &[(f32, f32, f32, bool)]) -> Vec<u8> {
    let sr = 44100u32;
    let total_end = notes.iter().map(|n| n.2).fold(0.1_f32, f32::max) + 0.03;
    let n_samples = (total_end * sr as f32).ceil() as usize;
    let mut mix = vec![0.0_f32; n_samples];

    for &(freq, start, end, triangle) in notes {
        let i0 = (start * sr as f32).floor() as usize;
        let i1 = ((end * sr as f32).ceil() as usize).min(n_samples);
        let dur = (end - start).max(0.02);
        for i in i0..i1 {
            let t = i as f32 / sr as f32 - start;
            let env = if t < 0.012 {
                t / 0.012
            } else {
                let rem = ((t - 0.012) / (dur - 0.012)).clamp(0.0, 1.0);
                (1.0 - rem).powi(2)
            };
            let phase = (t * freq) % 1.0;
            let wave = if triangle {
                1.0 - 4.0 * (phase - 0.5).abs()
            } else {
                (phase * std::f32::consts::TAU).sin()
            };
            mix[i] += wave * env * 0.13;
        }
    }

    let data_len = mix.len() * 2;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sr.to_le_bytes());
    wav.extend_from_slice(&(sr * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in mix {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav
}

fn play_wav_async(wav: Vec<u8>) {
    let mut guard = chime_slot().lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(wav);
    if let Some(ref buf) = *guard {
        unsafe {
            PlaySoundW(
                buf.as_ptr() as *const u16,
                std::ptr::null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }
}

/// Exact two-note triangle-wave approval bell from `dn-extras.js`:
/// G5 (783.99 Hz) 0.01..0.15s -> E6 (1318.51 Hz) 0.17..0.39s.
pub fn play_approval_chime() {
    let wav = synth_wav(&[
        (783.99, 0.01, 0.15, true),
        (1318.51, 0.17, 0.39, true),
    ]);
    play_wav_async(wav);
}

/// 6-note + chord sine arpeggio chime for due reminders.
pub fn play_reminder_chime() {
    let wav = synth_wav(&[
        (523.25, 0.01, 0.16, false),
        (659.25, 0.11, 0.26, false),
        (783.99, 0.21, 0.36, false),
        (1046.50, 0.31, 0.56, false),
        (783.99, 0.44, 0.62, false),
        (1046.50, 0.54, 0.85, false),
        (523.25, 0.54, 0.85, false),
    ]);
    play_wav_async(wav);
}

/// Focus-complete triangle chime (880 Hz -> 1318.51 Hz).
pub fn play_done_chime() {
    let wav = synth_wav(&[
        (880.0, 0.01, 0.15, true),
        (1318.51, 0.15, 0.40, true),
    ]);
    play_wav_async(wav);
}

/// Gentle two-note AI Agent calendar alert chime (E5 659.25 Hz -> B5 987.77 Hz),
/// audibly distinct from the standard reminder arpeggio.
pub fn play_agent_chime() {
    let wav = synth_wav(&[
        (659.25, 0.01, 0.16, false),
        (987.77, 0.15, 0.42, false),
    ]);
    play_wav_async(wav);
}
