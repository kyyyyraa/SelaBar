//! Emotion/mood prosody tuning, RBJ biquad smooth EQ, stereo polish, and PCM WAV codec.

use std::f64::consts::PI;

const Q_BUTTER: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// High-level expressive delivery mood inferred from utterance text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMood {
    Neutral,
    Cheerful,
    Sad,
}

/// SSML prosody parameters tailored for Edge Neural TTS and Windows WinRT SSML.
#[derive(Debug, Clone, Copy)]
pub struct MoodProsody {
    pub edge_rate: &'static str,
    pub edge_pitch: &'static str,
    pub winrt_rate: &'static str,
    pub winrt_pitch: &'static str,
}

/// Returns tuned prosody parameters for the given expressive mood.
pub fn prosody_for_mood(mood: VoiceMood) -> MoodProsody {
    match mood {
        VoiceMood::Cheerful => MoodProsody {
            edge_rate: "+8%",
            edge_pitch: "+6Hz",
            winrt_rate: "+8%",
            winrt_pitch: "+5%",
        },
        VoiceMood::Sad => MoodProsody {
            edge_rate: "-11%",
            edge_pitch: "-5Hz",
            winrt_rate: "-10%",
            winrt_pitch: "-5%",
        },
        VoiceMood::Neutral => MoodProsody {
            edge_rate: "+4%",
            edge_pitch: "+2Hz",
            winrt_rate: "+4%",
            winrt_pitch: "+0%",
        },
    }
}

fn contains_word_cue(lower: &str, cue: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = lower[start..].find(cue) {
        let abs_start = start + pos;
        let abs_end = abs_start + cue.len();
        let before_ok = lower[..abs_start]
            .chars()
            .next_back()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true);
        let after_ok = lower[abs_end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        start = abs_start + cue.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    false
}

/// Detects whether raw utterance text calls for a `Cheerful`, `Sad`, or `Neutral` delivery.
/// Inspects raw text before sanitization so punctuation (`!`, `...`) and emojis contribute.
pub fn detect_voice_mood(raw_text: &str) -> VoiceMood {
    let lower = raw_text.to_lowercase();
    let mut cheerful_score: i32 = 0;
    let mut sad_score: i32 = 0;

    const CHEERFUL_STRONG_WORDS: &[&str] = &[
        "yeay",
        "hore",
        "mantap",
        "keren",
        "berhasil",
        "selesai",
        "sukses",
        "selamat",
        "bagus",
        "hebat",
        "asyik",
        "asik",
        "wih",
        "wah",
        "cerah",
        "terima kasih",
        "makasih",
        "senang",
        "semangat",
        "siap membantu",
        "lancar",
        "awesome",
        "great",
    ];
    const CHEERFUL_STRONG_SYMBOLS: &[&str] = &["🎉", "😊", "😄", "✨", "🔥"];
    const CHEERFUL_MILD_WORDS: &[&str] = &["halo", "hai", "pagi", "siap", "oke", "aman"];

    const SAD_STRONG_WORDS: &[&str] = &[
        "maaf",
        "mohon maaf",
        "gagal",
        "error",
        "terputus",
        "offline",
        "tidak ditemukan",
        "nggak ketemu",
        "gak ketemu",
        "tidak bisa",
        "belum bisa",
        "nggak bisa",
        "gak bisa",
        "dibatalkan",
        "ditolak",
        "sedih",
        "berat",
        "capek",
        "lelah",
        "istirahat",
        "sayang sekali",
        "sayangnya",
        "rusak",
        "habis",
        "kehabisan",
        "sorry",
        "failed",
    ];
    const SAD_STRONG_SYMBOLS: &[&str] = &["😢", "😔", "😞", "💔"];
    const SAD_MILD_WORDS: &[&str] = &[
        "yah",
        "hemm",
        "hmm",
        "kosong",
        "batal",
        "tenang",
        "sabar",
        "peringatan",
    ];

    for kw in CHEERFUL_STRONG_WORDS {
        if contains_word_cue(&lower, kw) {
            cheerful_score += 2;
        }
    }
    for sym in CHEERFUL_STRONG_SYMBOLS {
        if raw_text.contains(sym) {
            cheerful_score += 2;
        }
    }
    let excl_count = raw_text.chars().filter(|&c| c == '!').count().min(2) as i32;
    cheerful_score += excl_count;
    for kw in CHEERFUL_MILD_WORDS {
        if contains_word_cue(&lower, kw) {
            cheerful_score += 1;
        }
    }

    for kw in SAD_STRONG_WORDS {
        if contains_word_cue(&lower, kw) {
            sad_score += 2;
        }
    }
    for sym in SAD_STRONG_SYMBOLS {
        if raw_text.contains(sym) {
            sad_score += 2;
        }
    }
    if raw_text.contains("...") || raw_text.contains('…') {
        sad_score += 1;
    }
    for kw in SAD_MILD_WORDS {
        if contains_word_cue(&lower, kw) {
            sad_score += 1;
        }
    }

    if sad_score >= 2 && sad_score > cheerful_score {
        VoiceMood::Sad
    } else if cheerful_score >= 2 && cheerful_score > sad_score {
        VoiceMood::Cheerful
    } else {
        VoiceMood::Neutral
    }
}

/// Second-order IIR biquad filter implemented in Direct Form II Transposed.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn from_raw(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        let inv_a0 = 1.0 / a0;
        Self {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub fn highpass(sr: f64, f0: f64, q: f64) -> Self {
        let freq = f0.clamp(20.0, 0.44 * sr);
        let w0 = 2.0 * PI * freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let b0 = (1.0 + cos_w0) * 0.5;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    pub fn lowpass(sr: f64, f0: f64, q: f64) -> Self {
        let freq = f0.clamp(20.0, 0.44 * sr);
        let w0 = 2.0 * PI * freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let b0 = (1.0 - cos_w0) * 0.5;
        let b1 = 1.0 - cos_w0;
        let b2 = (1.0 - cos_w0) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    pub fn peaking(sr: f64, f0: f64, q: f64, gain_db: f64) -> Self {
        let freq = f0.clamp(20.0, 0.44 * sr);
        let w0 = 2.0 * PI * freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for x in samples.iter_mut() {
            let inp = *x;
            let out = self.b0 * inp + self.z1;
            self.z1 = self.b1 * inp - self.a1 * out + self.z2;
            self.z2 = self.b2 * inp - self.a2 * out;
            *x = out;
        }
    }
}

/// Applies silence trimming, mood-matched smooth EQ, gentle soft-clipping, and subtle Haas stereo
/// width to a mono `f64` stream, encoding the result into a 16-bit stereo RIFF PCM WAV buffer.
pub fn polish_mono_to_stereo_wav(samples: &[f64], sample_rate: u32, mood: VoiceMood) -> Vec<u8> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }

    let sr = sample_rate as f64;
    let nyquist_guard = 0.44 * sr;
    let silence_thresh = 0.0056; // ~ -45 dB

    // Trim leading/trailing silence while keeping natural attack/decay margins
    let first_non_silent = samples.iter().position(|&s| s.abs() >= silence_thresh);
    let last_non_silent = samples.iter().rposition(|&s| s.abs() >= silence_thresh);
    let mut y: Vec<f64> = match (first_non_silent, last_non_silent) {
        (Some(start), Some(end)) if start <= end => {
            let lead_pad = (0.02 * sr).round() as usize;
            let tail_pad = (0.04 * sr).round() as usize;
            let s_idx = start.saturating_sub(lead_pad);
            let e_idx = (end + 1 + tail_pad).min(samples.len());
            samples[s_idx..e_idx].to_vec()
        }
        _ => samples.to_vec(),
    };

    if y.is_empty() {
        return Vec::new();
    }

    // 1. Clean low-end rumble (85 Hz HPF)
    Biquad::highpass(sr, 85.0, Q_BUTTER).process_in_place(&mut y);

    // 2. Mood-matched tone shaping
    match mood {
        VoiceMood::Cheerful => {
            if 3200.0 < nyquist_guard {
                Biquad::peaking(sr, 3200.0, 1.2, 2.0).process_in_place(&mut y);
            }
            if 6200.0 < nyquist_guard {
                Biquad::peaking(sr, 6200.0, 1.3, 1.2).process_in_place(&mut y);
            }
        }
        VoiceMood::Sad => {
            if 240.0 < nyquist_guard {
                Biquad::peaking(sr, 240.0, 1.0, 2.2).process_in_place(&mut y);
            }
            if 6800.0 < nyquist_guard {
                Biquad::lowpass(sr, 6800.0, Q_BUTTER).process_in_place(&mut y);
            }
        }
        VoiceMood::Neutral => {
            if 3000.0 < nyquist_guard {
                Biquad::peaking(sr, 3000.0, 1.2, 1.6).process_in_place(&mut y);
            }
        }
    }

    // 3. Smooth fade-in (25 ms) & fade-out (55 ms)
    let len = y.len();
    let fade_in = ((sr * 0.025).round() as usize).min(len);
    for (i, sample) in y[..fade_in].iter_mut().enumerate() {
        let ramp = if fade_in > 1 {
            i as f64 / (fade_in - 1) as f64
        } else {
            1.0
        };
        *sample *= ramp;
    }
    let fade_out = ((sr * 0.055).round() as usize).min(len);
    let out_start = len - fade_out;
    for (i, sample) in y[out_start..].iter_mut().enumerate() {
        let ramp = if fade_out > 1 {
            1.0 - (i as f64 / (fade_out - 1) as f64)
        } else {
            0.0
        };
        *sample *= ramp;
    }

    // 4. Normalize peak to 0.90, then smooth tanh soft-clipper
    let peak = y
        .iter()
        .fold(0.0_f64, |acc, &s| if s.is_finite() { acc.max(s.abs()) } else { acc });
    let norm = if peak > 1e-6 { 0.90 / peak } else { 1.0 };
    for sample in y.iter_mut() {
        let scaled = if sample.is_finite() { *sample * norm } else { 0.0 };
        *sample = (scaled * 1.06).tanh() * 0.92;
    }

    // 5. Subtle stereo Haas air width + encode 16-bit PCM WAV
    let delay_sec = if mood == VoiceMood::Sad { 0.008 } else { 0.004 };
    let delay = ((sr * delay_sec).round() as usize).min(len);

    let data_bytes = (len * 4) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    let byte_rate = sample_rate.saturating_mul(4);

    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes()); // block_align = 2 ch * 2 bytes
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits_per_sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());

    for i in 0..len {
        let left = y[i];
        let delayed = if i >= delay { y[i - delay] } else { 0.0 };
        let right = 0.88 * left + 0.12 * delayed;

        let l_i16 = (left * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
        let r_i16 = (right * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
        wav.extend_from_slice(&l_i16.to_le_bytes());
        wav.extend_from_slice(&r_i16.to_le_bytes());
    }

    wav
}

/// Decodes an uncompressed 16-bit PCM RIFF WAV buffer into mono `f64` samples in `[-1.0, 1.0]`
/// alongside its sample rate.
pub fn decode_wav_mono_f64(wav: &[u8]) -> Option<(Vec<f64>, u32)> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12usize;
    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut found_fmt = false;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= wav.len() {
        let chunk_id = &wav[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(chunk_size).min(wav.len());

        if chunk_id == b"fmt " && payload_end - payload_start >= 16 {
            let f = &wav[payload_start..payload_end];
            audio_format = u16::from_le_bytes([f[0], f[1]]);
            channels = u16::from_le_bytes([f[2], f[3]]);
            sample_rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            bits_per_sample = u16::from_le_bytes([f[14], f[15]]);
            found_fmt = true;
        } else if chunk_id == b"data" {
            data_slice = Some(&wav[payload_start..payload_end]);
        }

        // RIFF chunks are word-aligned (even byte boundary)
        let advance = 8 + chunk_size + (chunk_size & 1);
        pos = pos.saturating_add(advance);
        if found_fmt && data_slice.is_some() {
            break;
        }
    }

    if !found_fmt || audio_format != 1 || bits_per_sample != 16 || sample_rate < 8000 {
        return None;
    }
    if !(1..=2).contains(&channels) {
        return None;
    }

    let data = data_slice?;
    let frame_bytes = (channels as usize) * 2;
    if data.len() < frame_bytes {
        return None;
    }

    let frame_count = data.len() / frame_bytes;
    let mut mono = Vec::with_capacity(frame_count);
    for frame in data.chunks_exact(frame_bytes) {
        if channels == 1 {
            let s = i16::from_le_bytes([frame[0], frame[1]]) as f64 / 32768.0;
            mono.push(s);
        } else {
            let l = i16::from_le_bytes([frame[0], frame[1]]) as f64 / 32768.0;
            let r = i16::from_le_bytes([frame[2], frame[3]]) as f64 / 32768.0;
            mono.push((l + r) * 0.5);
        }
    }

    Some((mono, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::{
        decode_wav_mono_f64, detect_voice_mood, polish_mono_to_stereo_wav, VoiceMood,
    };
    use std::f64::consts::PI;

    #[test]
    fn mood_detection_distinguishes_cheerful_sad_and_neutral() {
        assert_eq!(
            detect_voice_mood("Yeay! Akhirnya semua tugas selesai dengan mantap!"),
            VoiceMood::Cheerful
        );
        assert_eq!(
            detect_voice_mood("Halo! Suara SelaBar siap membantu hari ini!"),
            VoiceMood::Cheerful
        );
        assert_eq!(
            detect_voice_mood("Maaf ya, koneksi lagi terputus dan prosesnya gagal..."),
            VoiceMood::Sad
        );
        assert_eq!(
            detect_voice_mood("Hemm... kelihatannya hari ini cukup berat, istirahat dulu ya."),
            VoiceMood::Sad
        );
        assert_eq!(
            detect_voice_mood("Saat ini jam 3 sore di Jakarta dengan suhu 29 derajat."),
            VoiceMood::Neutral
        );
        // Ensure short cue substrings inside unrelated words don't cause false positives
        assert_eq!(
            detect_voice_mood("Cek dokumen halaman bawah token ini."),
            VoiceMood::Neutral
        );
    }

    #[test]
    fn polish_and_decode_round_trip_produces_valid_stereo_wav() {
        let sr = 24000u32;
        let n = sr as usize; // 1.0 second test tone
        let mut mono = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / sr as f64;
            mono.push(0.45 * (2.0 * PI * 440.0 * t).sin());
        }

        for mood in [VoiceMood::Neutral, VoiceMood::Cheerful, VoiceMood::Sad] {
            let wav = polish_mono_to_stereo_wav(&mono, sr, mood);
            assert!(wav.len() > 44);
            assert_eq!(&wav[0..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            let byte_rate = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]);
            assert_eq!(byte_rate, sr * 4);

            let (decoded, dec_sr) = decode_wav_mono_f64(&wav).expect("valid PCM WAV");
            assert_eq!(dec_sr, sr);
            assert!(!decoded.is_empty());
            assert!(decoded.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
    }
}
