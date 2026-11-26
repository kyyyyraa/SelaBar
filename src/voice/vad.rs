use std::time::{Duration, Instant};

pub struct VadConfig {
    pub min_threshold: f32,
    pub min_speech_chunks: u32,
    pub no_speech_timeout: Duration,
    pub silence_duration: Duration,
    pub hard_cap_duration: Duration,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            min_threshold: 0.006,
            min_speech_chunks: 2,
            no_speech_timeout: Duration::from_secs(6),
            silence_duration: Duration::from_millis(1200),
            hard_cap_duration: Duration::from_secs(30),
        }
    }
}

pub enum VadState {
    Listening,
    Speaking {
        speech_start: Instant,
        last_speech: Instant,
    },
    Finished(VadFinishReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadFinishReason {
    SilenceTimeout,
    HardCapTimeout,
    NoSpeechTimeout,
}

pub struct VoiceActivityDetector {
    config: VadConfig,
    noise_floor: f32,
    first_frame: bool,
    speech_streak: u32,
    speech_peak: f32,
    calibration_until: Instant,
    state: VadState,
    start_time: Instant,
}

impl VoiceActivityDetector {
    pub fn new(config: VadConfig) -> Self {
        let now = Instant::now();
        Self {
            noise_floor: config.min_threshold,
            first_frame: true,
            speech_streak: 0,
            speech_peak: 0.0,
            calibration_until: now + Duration::from_millis(300),
            config,
            state: VadState::Listening,
            start_time: now,
        }
    }

    pub fn calculate_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    pub fn process_chunk(&mut self, samples: &[f32]) -> (f32, bool, Option<VadFinishReason>) {
        self.process_chunk_at(samples, Instant::now())
    }

    fn process_chunk_at(
        &mut self,
        samples: &[f32],
        now: Instant,
    ) -> (f32, bool, Option<VadFinishReason>) {
        let rms = Self::calculate_rms(samples);
        let in_calibration =
            matches!(self.state, VadState::Listening) && now < self.calibration_until;

        if matches!(self.state, VadState::Listening) {
            if self.first_frame {
                self.first_frame = false;
                self.noise_floor = rms.clamp(0.003, 0.08);
            } else if in_calibration {
                self.noise_floor =
                    (self.noise_floor + (rms - self.noise_floor) * 0.25).clamp(0.003, 0.08);
            } else if rms < self.noise_floor {
                self.noise_floor =
                    (self.noise_floor + (rms - self.noise_floor) * 0.08).max(0.003);
            } else {
                self.noise_floor =
                    (self.noise_floor + (rms - self.noise_floor) * 0.02).min(0.08);
            }
        }

        if now.duration_since(self.start_time) >= self.config.hard_cap_duration {
            self.state = VadState::Finished(VadFinishReason::HardCapTimeout);
            return (rms, false, Some(VadFinishReason::HardCapTimeout));
        }

        match self.state {
            VadState::Listening => {
                let start_thresh = (self.noise_floor * 2.6).max(self.config.min_threshold);
                let is_speech_start = rms > start_thresh;
                if !in_calibration && is_speech_start {
                    self.speech_streak += 1;
                    if self.speech_streak >= self.config.min_speech_chunks.max(1) {
                        self.speech_peak = rms;
                        self.state = VadState::Speaking {
                            speech_start: now,
                            last_speech: now,
                        };
                        return (rms, true, None);
                    }
                    (rms, false, None)
                } else {
                    self.speech_streak = 0;
                    if now.duration_since(self.start_time) >= self.config.no_speech_timeout {
                        self.state = VadState::Finished(VadFinishReason::NoSpeechTimeout);
                        (rms, false, Some(VadFinishReason::NoSpeechTimeout))
                    } else {
                        (rms, false, None)
                    }
                }
            }
            VadState::Speaking {
                speech_start,
                mut last_speech,
            } => {
                self.speech_peak = (self.speech_peak * 0.985).max(rms);
                let stop_thresh = (self.noise_floor * 1.9)
                    .max(self.config.min_threshold * 0.85)
                    .max(self.speech_peak * 0.16);
                let is_speech_sample = rms > stop_thresh;
                if is_speech_sample {
                    last_speech = now;
                    self.state = VadState::Speaking {
                        speech_start,
                        last_speech,
                    };
                    (rms, true, None)
                } else if now.duration_since(last_speech) >= self.config.silence_duration {
                    self.state = VadState::Finished(VadFinishReason::SilenceTimeout);
                    (rms, false, Some(VadFinishReason::SilenceTimeout))
                } else {
                    self.state = VadState::Speaking {
                        speech_start,
                        last_speech,
                    };
                    (rms, true, None)
                }
            }
            VadState::Finished(reason) => (rms, false, Some(reason)),
        }
    }
}

pub fn trim_silence(audio: &[f32], sample_rate: usize) -> Vec<f32> {
    if audio.is_empty() {
        return Vec::new();
    }
    let peak = audio.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let threshold = (peak * 0.08).max(0.0015);
    let mut i = 0;
    while i < audio.len() && audio[i].abs() < threshold {
        i += 1;
    }
    let mut j = audio.len().saturating_sub(1);
    while j > i && audio[j].abs() < threshold {
        j -= 1;
    }
    if j <= i {
        return audio.to_vec();
    }
    let pad = (sample_rate as f32 * 0.15) as usize;
    let from = i.saturating_sub(pad);
    let to = (j + 1 + pad).min(audio.len());
    audio[from..to].to_vec()
}

pub fn normalize_audio(audio: &[f32], target_peak: f32) -> (Vec<f32>, f32) {
    let peak = audio.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    if peak < 1e-6 {
        return (audio.to_vec(), peak);
    }
    let gain = (target_peak / peak).min(60.0);
    let out = audio.iter().map(|s| (s * gain).clamp(-1.0, 1.0)).collect();
    (out, peak)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_noise_calibrates_without_false_trigger_then_speech_triggers_and_stops() {
        let mut vad = VoiceActivityDetector::new(VadConfig {
            min_threshold: 0.0015,
            min_speech_chunks: 2,
            no_speech_timeout: Duration::from_secs(6),
            silence_duration: Duration::from_millis(600),
            hard_cap_duration: Duration::from_secs(20),
        });
        let t0 = vad.start_time;
        let ambient = [0.025_f32; 32];
        for i in 0..8 {
            let t = t0 + Duration::from_millis(i * 60);
            let (_, sp, done) = vad.process_chunk_at(&ambient, t);
            assert!(!sp, "steady ambient noise at {}ms must not trigger speech", i * 60);
            assert!(done.is_none());
        }

        // Single brief transient spike must not trigger due to min_speech_chunks = 2
        let loud = [0.16_f32; 32];
        let (_, sp1, _) = vad.process_chunk_at(&loud, t0 + Duration::from_millis(500));
        assert!(!sp1);
        let (_, _, _) = vad.process_chunk_at(&ambient, t0 + Duration::from_millis(530));

        // Sustained speech (2 consecutive chunks above threshold) triggers Speaking
        let (_, _, _) = vad.process_chunk_at(&loud, t0 + Duration::from_millis(600));
        let (_, sp2, _) = vad.process_chunk_at(&loud, t0 + Duration::from_millis(630));
        assert!(sp2);

        // Returns to ambient -> SilenceTimeout after silence_duration (600ms)
        let (_, _, done_mid) = vad.process_chunk_at(&ambient, t0 + Duration::from_millis(900));
        assert!(done_mid.is_none());
        let (_, _, done_end) = vad.process_chunk_at(&ambient, t0 + Duration::from_millis(1250));
        assert_eq!(done_end, Some(VadFinishReason::SilenceTimeout));
    }

    #[test]
    fn brief_quiet_dip_followed_by_fan_noise_does_not_pin_speaking() {
        let mut vad = VoiceActivityDetector::new(VadConfig {
            min_threshold: 0.006,
            min_speech_chunks: 2,
            no_speech_timeout: Duration::from_secs(6),
            silence_duration: Duration::from_millis(800),
            hard_cap_duration: Duration::from_secs(20),
        });
        let t0 = vad.start_time;
        let fan = [0.006_f32; 32];
        let dip = [0.0001_f32; 32];
        let speech = [0.12_f32; 32];

        for i in 0..6 {
            let (_, sp, _) = vad.process_chunk_at(&fan, t0 + Duration::from_millis(i * 60));
            assert!(!sp);
        }
        // Brief near-zero dip after calibration
        let (_, sp_dip, _) = vad.process_chunk_at(&dip, t0 + Duration::from_millis(380));
        assert!(!sp_dip);
        // Back to fan noise: must NOT falsely trigger Speaking
        for i in 0..4 {
            let (_, sp, _) = vad.process_chunk_at(&fan, t0 + Duration::from_millis(420 + i * 30));
            assert!(!sp);
        }
        // Genuine speech starts
        let _ = vad.process_chunk_at(&speech, t0 + Duration::from_millis(600));
        let (_, sp_on, _) = vad.process_chunk_at(&speech, t0 + Duration::from_millis(630));
        assert!(sp_on);

        // User stops speaking back to fan noise -> ends right after 800ms silence_duration
        let (_, _, mid) = vad.process_chunk_at(&fan, t0 + Duration::from_millis(1100));
        assert!(mid.is_none());
        let (_, _, done) = vad.process_chunk_at(&fan, t0 + Duration::from_millis(1450));
        assert_eq!(done, Some(VadFinishReason::SilenceTimeout));
    }
}
