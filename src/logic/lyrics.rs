use crate::backend::store::StoreManager;
use crate::events::{LrcLine, LrcResult, SmtcSessionInfo, UiSender};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const LYR_LEAD_MS: i64 = 900;

#[derive(Debug, Clone)]
pub struct LyricsState {
    pub track_key: String,
    pub status: String, // "no-track" | "loading" | "synced" | "plain" | "instrumental" | "not-found" | "error"
    pub lines: Vec<LrcLine>,
    pub plain: Option<String>,
    pub cur_idx: i32,
    pub base_pos_ms: u64,
    pub base_wall: Instant,
    pub dur_ms: u64,
    pub playing: bool,
    pub src_last_pos: u64,
    pub src_moves: i32,
    pub frozen_pos: Option<u64>,
    pub last_playing_wall: Option<Instant>,
    pub last_media_wall: Option<Instant>,
    pub offsets: HashMap<String, i64>,
    pub manual_scroll_until: Option<Instant>,
    pub last_updated_ticks: i64,
    pub last_raw_pos_ms: u64,
    pub seek_lock_until: Option<Instant>,
    pub zero_glitch_streak: u8,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self {
            track_key: String::new(),
            status: "no-track".to_string(),
            lines: Vec::new(),
            plain: None,
            cur_idx: -1,
            base_pos_ms: 0,
            base_wall: Instant::now(),
            dur_ms: 0,
            playing: false,
            src_last_pos: 0,
            src_moves: 0,
            frozen_pos: None,
            last_playing_wall: None,
            last_media_wall: None,
            offsets: HashMap::new(),
            manual_scroll_until: None,
            last_updated_ticks: 0,
            last_raw_pos_ms: 0,
            seek_lock_until: None,
            zero_glitch_streak: 0,
        }
    }
}

pub fn norm_offset_key(artist: &str, title: &str) -> String {
    let norm = |s: &str| -> String {
        s.to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect()
    };
    format!("{}|{}", norm(artist), norm(title))
}

impl LyricsState {
    pub fn load_offsets_from_store(&mut self, store: &StoreManager) {
        let raw = store.get("lrcOffsets");
        if let Some(map) = raw.as_object() {
            self.offsets.clear();
            for (k, v) in map {
                if let Some(n) = v.as_i64() {
                    self.offsets.insert(k.clone(), n.clamp(-3000, 3000));
                }
            }
        }
    }

    pub fn save_offsets_to_store(&self, store: &StoreManager, ui: Option<&UiSender>) {
        let mut map = Map::new();
        for (k, v) in &self.offsets {
            map.insert(k.clone(), Value::from(*v));
        }
        store.set(ui, "lrcOffsets", Value::Object(map));
    }

    pub fn cur_offset_ms(&self, artist: &str, title: &str) -> i64 {
        let k = norm_offset_key(artist, title);
        *self.offsets.get(&k).unwrap_or(&0)
    }

    pub fn adjust_offset(
        &mut self,
        artist: &str,
        title: &str,
        delta_ms: i64,
        store: &StoreManager,
        ui: Option<&UiSender>,
    ) {
        let k = norm_offset_key(artist, title);
        let cur = *self.offsets.get(&k).unwrap_or(&0);
        let next = (cur + delta_ms).clamp(-3000, 3000);
        self.offsets.insert(k, next);
        self.save_offsets_to_store(store, ui);
    }

    pub fn cur_ms(&self) -> u64 {
        if self.playing {
            self.base_pos_ms
                .saturating_add(self.base_wall.elapsed().as_millis() as u64)
        } else {
            self.base_pos_ms
        }
    }

    pub fn on_smtc_session(&mut self, info: Option<&SmtcSessionInfo>) -> Option<(String, String, String, f64)> {
        let now = Instant::now();
        let Some(np) = info else {
            self.playing = false;
            return None;
        };
        if np.title.trim().is_empty() && np.artist.trim().is_empty() {
            self.playing = false;
            return None;
        }
        self.last_media_wall = Some(now);
        if np.is_playing {
            self.last_playing_wall = Some(now);
        }

        let new_key = format!(
            "{}|{}",
            np.artist.trim().to_lowercase(),
            np.title.trim().to_lowercase()
        );
        let pos_ms = (np.position_sec.max(0.0) * 1000.0) as u64;
        let new_dur_ms = (np.duration_sec.max(0.0) * 1000.0) as u64;
        let was_playing = self.playing;
        self.playing = np.is_playing;

        if new_key != self.track_key {
            self.track_key = new_key.clone();
            self.status = "loading".to_string();
            self.lines.clear();
            self.plain = None;
            self.cur_idx = -1;
            self.base_pos_ms = pos_ms;
            self.base_wall = now;
            self.dur_ms = new_dur_ms;
            self.src_last_pos = pos_ms;
            self.src_moves = 0;
            self.frozen_pos = Some(pos_ms);
            self.last_updated_ticks = 0;
            self.last_raw_pos_ms = pos_ms;
            self.seek_lock_until = None;
            self.zero_glitch_streak = 0;
            return Some((new_key, np.title.clone(), np.artist.clone(), np.duration_sec));
        }

        if new_dur_ms > 0 {
            self.dur_ms = new_dur_ms;
        }

        // Same track (e.g. album art late arrival or play/pause flip)
        if was_playing != self.playing {
            if self.playing {
                self.base_wall = now;
                if (self.base_pos_ms as i64 - pos_ms as i64).abs() > 2500
                    && self.frozen_pos != Some(pos_ms)
                    && !(pos_ms < 500 && self.base_pos_ms > 3000)
                {
                    self.base_pos_ms = pos_ms;
                }
            } else {
                self.base_pos_ms = self.cur_ms();
                self.base_wall = now;
            }
        }
        None
    }

    pub fn on_lrc_loaded(&mut self, res: LrcResult) {
        if res.key != self.track_key {
            return;
        }
        self.status = res.status;
        self.lines = res.lines;
        self.plain = res.plain;
        self.cur_idx = -1;
    }

    pub fn on_position_tick(
        &mut self,
        pos_ms: u64,
        playing: bool,
        raw_pos_ms: u64,
        last_updated_ticks: i64,
        dur_ms: u64,
    ) {
        let now = Instant::now();
        self.last_media_wall = Some(now);
        if playing {
            self.last_playing_wall = Some(now);
        }
        if dur_ms > 0 {
            self.dur_ms = dur_ms;
        }

        let raw_changed = raw_pos_ms != self.last_raw_pos_ms
            || (last_updated_ticks != 0 && last_updated_ticks != self.last_updated_ticks);

        if pos_ms > self.src_last_pos.saturating_add(150) {
            self.src_moves = (self.src_moves + 1).min(10);
            self.frozen_pos = None;
        } else if pos_ms == self.src_last_pos && !raw_changed {
            self.src_moves = (self.src_moves - 1).max(0);
            if self.src_moves < 2 {
                self.frozen_pos = Some(pos_ms);
            }
        }
        let prev_raw_pos_ms = self.last_raw_pos_ms;
        self.src_last_pos = pos_ms;
        self.last_raw_pos_ms = raw_pos_ms;
        if last_updated_ticks != 0 {
            self.last_updated_ticks = last_updated_ticks;
        }

        if self.playing != playing {
            if !playing {
                self.base_pos_ms = self.cur_ms();
            }
            self.base_wall = now;
            self.playing = playing;
        }

        let cur = self.cur_ms();
        if let Some(until) = self.seek_lock_until {
            if now < until {
                if (cur as i64 - pos_ms as i64).abs() <= 1500 && pos_ms > 0 {
                    // Player caught up to user seek
                    self.seek_lock_until = None;
                } else {
                    return;
                }
            } else {
                self.seek_lock_until = None;
            }
        }

        // Anti-jump-to-zero glitch guard: if lyrics are already > 3s into a song
        // and SMTC reports < 1.5s, ignore frozen/single-tick glitches until sustained
        // for 3 consecutive ticks with real timeline movement/updates.
        if cur > 3000 && pos_ms < 1500 {
            if playing && pos_ms == 0 && raw_pos_ms == 0 && prev_raw_pos_ms == 0 {
                // Frozen 0 from browser SMTC (even if LastUpdatedTime ticks) — keep wall clock running.
                self.frozen_pos = Some(0);
                self.zero_glitch_streak = 0;
                return;
            }
            if (raw_changed || (raw_pos_ms > 0 && self.src_moves >= 2))
                && (!playing || pos_ms > 0 || prev_raw_pos_ms > 1500)
            {
                self.zero_glitch_streak = self.zero_glitch_streak.saturating_add(1);
            } else {
                self.frozen_pos = Some(pos_ms);
                self.zero_glitch_streak = 0;
                return;
            }
            if self.zero_glitch_streak < 3 {
                return;
            }
        } else {
            self.zero_glitch_streak = 0;
        }

        let diff = (cur as i64 - pos_ms as i64).abs();
        if self.src_moves >= 2 {
            if diff > 2000 {
                self.base_pos_ms = pos_ms;
                self.base_wall = now;
            }
        } else if diff > 2500 && self.frozen_pos != Some(pos_ms) {
            self.base_pos_ms = pos_ms;
            self.base_wall = now;
            self.frozen_pos = Some(pos_ms);
        }
    }

    pub fn seek_local(&mut self, pos_ms: u64) {
        self.base_pos_ms = pos_ms;
        self.base_wall = Instant::now();
        self.frozen_pos = None;
        self.seek_lock_until = Some(Instant::now() + Duration::from_millis(2500));
        self.zero_glitch_streak = 0;
    }

    pub fn update_active_index(&mut self, artist: &str, title: &str) -> bool {
        if self.status != "synced" || self.lines.is_empty() {
            let changed = self.cur_idx != -1;
            self.cur_idx = -1;
            return changed;
        }
        let target = (self.cur_ms() as i64 + LYR_LEAD_MS + self.cur_offset_ms(artist, title)).max(0) as u64;
        // Binary search largest index with lines[i].t_ms <= target
        let idx = match self.lines.binary_search_by_key(&target, |l| l.t_ms) {
            Ok(i) => i as i32,
            Err(0) => -1,
            Err(i) => (i - 1) as i32,
        };
        let changed = idx != self.cur_idx;
        self.cur_idx = idx;
        changed
    }

    pub fn current_line_text(&self) -> &str {
        if self.cur_idx >= 0 {
            if let Some(line) = self.lines.get(self.cur_idx as usize) {
                return &line.text;
            }
        }
        ""
    }

    pub fn song_ended(&self) -> bool {
        !self.playing && self.dur_ms > 0 && self.base_pos_ms >= self.dur_ms.saturating_sub(1500)
    }

    pub fn media_gone(&self) -> bool {
        match self.last_media_wall {
            Some(w) => w.elapsed() > Duration::from_secs(5),
            None => true,
        }
    }

    pub fn show_player_state(&self, has_song: bool) -> bool {
        if self.playing {
            return true;
        }
        if !has_song {
            return false;
        }
        match self.last_playing_wall {
            Some(w) => w.elapsed() < Duration::from_secs(90),
            None => true,
        }
    }

    pub fn mid_active(&self, lyrics_on_closed: bool) -> bool {
        self.status == "synced"
            && self.cur_idx >= 0
            && !self.current_line_text().trim().is_empty()
            && lyrics_on_closed
            && !self.song_ended()
            && !self.media_gone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_np(title: &str, artist: &str, dur: f64, pos: f64, playing: bool) -> SmtcSessionInfo {
        SmtcSessionInfo {
            source_app_id: "Zen".to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album: String::new(),
            is_playing: playing,
            position_sec: pos,
            duration_sec: dur,
            thumbnail_bytes: None,
            thumbnail_mime: None,
            accent_tint: None,
        }
    }

    #[test]
    fn duration_jitter_does_not_reset_track() {
        let mut st = LyricsState::default();
        let first = st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 210.0, 45.0, true)));
        assert!(first.is_some());
        st.status = "synced".to_string();
        st.base_pos_ms = 60_000;

        assert!(st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 210.4, 60.2, true))).is_none());
        assert_eq!(st.status, "synced");
        assert!(st.cur_ms() >= 60_000);

        assert!(st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 0.0, 61.0, true))).is_none());
        assert_eq!(st.dur_ms, 210_400);
        assert!(st.cur_ms() >= 60_000);
    }

    #[test]
    fn frozen_zero_does_not_reset_mid_song() {
        let mut st = LyricsState::default();
        st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 210.0, 0.0, true)));
        st.base_pos_ms = 60_000;
        for _ in 0..5 {
            st.on_position_tick(0, true, 0, 1000, 210_000);
        }
        assert!(st.cur_ms() >= 60_000);
    }

    #[test]
    fn transient_zero_glitch_ignored() {
        let mut st = LyricsState::default();
        st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 210.0, 75.0, true)));
        st.src_moves = 5;
        st.last_raw_pos_ms = 75_000;
        st.last_updated_ticks = 5000;
        st.base_pos_ms = 75_000;

        // Single bogus zero tick should not snap lyrics back to 0:00
        st.on_position_tick(0, true, 0, 5001, 210_000);
        assert!(st.cur_ms() >= 75_000);

        // Next valid tick recovers cleanly
        st.on_position_tick(75_500, true, 75_500, 5002, 210_000);
        assert_eq!(st.zero_glitch_streak, 0);
        assert!(st.cur_ms() >= 75_000);
    }

    #[test]
    fn genuine_seek_to_zero_and_seek_local_work() {
        let mut st = LyricsState::default();
        st.on_smtc_session(Some(&mk_np("Lagu", "Artis", 210.0, 80.0, true)));
        st.src_moves = 5;
        st.last_raw_pos_ms = 80_000;
        st.last_updated_ticks = 100;
        st.base_pos_ms = 80_000;

        // Sustained 3+ ticks near 0 with timeline movement snaps to 0
        st.on_position_tick(200, true, 200, 101, 210_000);
        assert!(st.cur_ms() >= 80_000);
        st.on_position_tick(600, true, 600, 102, 210_000);
        assert!(st.cur_ms() >= 80_000);
        st.on_position_tick(1000, true, 1000, 103, 210_000);
        assert!(st.cur_ms() < 5_000);

        // seek_local protects position from lagging SMTC ticks
        st.seek_local(95_000);
        st.on_position_tick(10_000, true, 10_000, 104, 210_000);
        assert!(st.cur_ms() >= 95_000);
    }
}
