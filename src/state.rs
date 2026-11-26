use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}
fn default_notch_style() -> String {
    "black".to_string()
}
fn default_dock_side() -> String {
    "bottom".to_string()
}
fn default_dock_size() -> String {
    "default".to_string()
}
fn default_notch_size() -> String {
    "default".to_string()
}
fn default_tts_voice() -> String {
    "auto".to_string()
}
fn default_companion_sleeps() -> String {
    "auto".to_string()
}
fn default_collapsed_right() -> String {
    "time".to_string()
}
fn default_avatar() -> String {
    "ghost".to_string()
}
fn default_companion_mode() -> String {
    "time".to_string()
}
fn default_focus_minutes() -> i32 {
    25
}
fn default_display_id() -> String {
    "primary".to_string()
}
fn default_router_url() -> String {
    String::new()
}
fn default_ai_primary_provider() -> String {
    "auto".to_string()
}
fn default_hotkey_voice() -> String {
    "Ctrl+Alt+V".to_string()
}
fn default_hotkey_dictate() -> String {
    "Ctrl+Alt+D".to_string()
}
fn default_hotkey_ask() -> String {
    "Ctrl+Alt+C".to_string()
}
fn default_hotkey_chat() -> String {
    "Alt+C".to_string()
}
fn default_ai_access_mode() -> String {
    "auto".to_string()
}

fn de_i32_from_num<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Value::deserialize(deserializer)?;
    let n = match val {
        Value::Number(num) => num
            .as_i64()
            .map(|v| v as f64)
            .or_else(|| num.as_f64())
            .unwrap_or(25.0),
        Value::String(s) => s.trim().parse::<f64>().unwrap_or(25.0),
        _ => 25.0,
    };
    if !n.is_finite() || n <= 0.0 {
        return Ok(25);
    }
    Ok(n.round().max(1.0) as i32)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_true")]
    pub show_music: bool,
    #[serde(default = "default_true")]
    pub lyrics_on_closed: bool,
    #[serde(default = "default_false")]
    pub show_tasks: bool,
    #[serde(default = "default_true")]
    pub show_avatar: bool,
    #[serde(default = "default_false")]
    pub show_focus: bool,
    #[serde(default = "default_true")]
    pub show_ai_usage: bool,
    #[serde(default = "default_false")]
    pub show_status: bool,
    #[serde(default = "default_true")]
    pub ambient_video: bool,
    #[serde(default = "default_true")]
    pub album_tint: bool,
    #[serde(default = "default_true")]
    pub start_on_boot: bool,
    #[serde(default = "default_notch_style")]
    pub notch_style: String,
    #[serde(default = "default_dock_side")]
    pub dock_side: String,
    #[serde(default = "default_dock_size")]
    pub dock_size: String,
    #[serde(default = "default_notch_size")]
    pub notch_size: String,
    #[serde(default)]
    pub hidden_views: Vec<String>,
    #[serde(default = "default_collapsed_right")]
    pub collapsed_right: String,
    #[serde(default = "default_true")]
    pub catch_screenshots: bool,
    #[serde(default = "default_avatar")]
    pub avatar: String,
    #[serde(default = "default_companion_mode")]
    pub companion_mode: String,
    #[serde(default = "default_companion_sleeps")]
    pub companion_sleeps: String,
    #[serde(default = "default_focus_minutes", deserialize_with = "de_i32_from_num")]
    pub focus_minutes: i32,
    #[serde(default)]
    pub hidden_limits: Vec<String>,
    #[serde(default = "default_display_id")]
    pub selected_display_id: String,
    #[serde(default = "default_true")]
    pub hide_on_fullscreen: bool,
    #[serde(default = "default_false")]
    pub tuck_for_browsers: bool,
    #[serde(default = "default_false")]
    pub hide_in_screenshots: bool,
    #[serde(default = "default_false")]
    pub dnd: bool,
    #[serde(default = "default_false")]
    pub dn_voice_enabled: bool,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    #[serde(default = "default_true")]
    pub dn_stt_enabled: bool,
    #[serde(default = "default_true")]
    pub dn_extras_ai: bool,
    #[serde(default = "default_hotkey_voice")]
    pub hotkey_voice: String,
    #[serde(default = "default_hotkey_dictate")]
    pub hotkey_dictate: String,
    #[serde(default = "default_hotkey_ask")]
    pub hotkey_ask: String,
    #[serde(default = "default_hotkey_chat")]
    pub hotkey_chat: String,
    #[serde(default = "default_router_url")]
    pub dn_router_base_url: String,
    #[serde(default)]
    pub dn_router_key_sealed: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dn_router_key: String,
    #[serde(default)]
    pub dn_router_model: String,
    #[serde(default)]
    pub dn_router_voice_model: String,
    #[serde(default)]
    pub dn_router_convo_model: String,
    #[serde(default)]
    pub dn_gemini_key_sealed: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dn_gemini_key: String,
    #[serde(default)]
    pub dn_gemini_model: String,
    #[serde(default)]
    pub dn_openai_base_url: String,
    #[serde(default)]
    pub dn_openai_key_sealed: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dn_openai_key: String,
    #[serde(default)]
    pub dn_openai_model: String,
    #[serde(default = "default_ai_primary_provider")]
    pub dn_ai_primary_provider: String,
    #[serde(default = "default_ai_access_mode")]
    pub dn_ai_access_mode: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_music: true,
            lyrics_on_closed: true,
            show_tasks: false,
            show_avatar: true,
            show_focus: false,
            show_ai_usage: true,
            show_status: false,
            ambient_video: true,
            album_tint: true,
            start_on_boot: true,
            notch_style: default_notch_style(),
            dock_side: default_dock_side(),
            dock_size: default_dock_size(),
            notch_size: default_notch_size(),
            hidden_views: Vec::new(),
            collapsed_right: default_collapsed_right(),
            catch_screenshots: true,
            avatar: default_avatar(),
            companion_mode: default_companion_mode(),
            companion_sleeps: default_companion_sleeps(),
            focus_minutes: default_focus_minutes(),
            hidden_limits: Vec::new(),
            selected_display_id: default_display_id(),
            hide_on_fullscreen: true,
            tuck_for_browsers: false,
            hide_in_screenshots: false,
            dnd: false,
            dn_voice_enabled: false,
            tts_voice: default_tts_voice(),
            dn_stt_enabled: true,
            dn_extras_ai: true,
            hotkey_voice: default_hotkey_voice(),
            hotkey_dictate: default_hotkey_dictate(),
            hotkey_ask: default_hotkey_ask(),
            hotkey_chat: default_hotkey_chat(),
            dn_router_base_url: default_router_url(),
            dn_router_key_sealed: String::new(),
            dn_router_key: String::new(),
            dn_router_model: String::new(),
            dn_router_voice_model: String::new(),
            dn_router_convo_model: String::new(),
            dn_gemini_key_sealed: String::new(),
            dn_gemini_key: String::new(),
            dn_gemini_model: String::new(),
            dn_openai_base_url: String::new(),
            dn_openai_key_sealed: String::new(),
            dn_openai_key: String::new(),
            dn_openai_model: String::new(),
            dn_ai_primary_provider: default_ai_primary_provider(),
            dn_ai_access_mode: default_ai_access_mode(),
            extra: HashMap::new(),
        }
    }
}

impl Settings {
    pub fn from_value(v: &Value) -> Self {
        let mut s: Settings = if let Some(obj) = v.as_object() {
            if let Ok(parsed) = serde_json::from_value(v.clone()) {
                parsed
            } else {
                // Field-by-field fallback so one legacy/malformed key never resets all toggles
                let mut cur = serde_json::to_value(Settings::default()).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
                if let Some(cur_obj) = cur.as_object_mut() {
                    for (k, val) in obj {
                        let fixed = if k == "focusMinutes" {
                            val.as_f64()
                                .map(|f| Value::from(if f.is_finite() && f > 0.0 { f.round().max(1.0) as i32 } else { 25 }))
                                .unwrap_or_else(|| Value::from(25))
                        } else {
                            val.clone()
                        };
                        let probe_key_val = serde_json::json!({ k: fixed });
                        let mut trial_obj = cur_obj.clone();
                        trial_obj.insert(k.clone(), fixed.clone());
                        if serde_json::from_value::<Settings>(Value::Object(trial_obj)).is_ok() {
                            cur_obj.insert(k.clone(), fixed);
                        } else {
                            let _ = probe_key_val;
                        }
                    }
                }
                serde_json::from_value(cur).unwrap_or_default()
            }
        } else {
            Settings::default()
        };
        s.apply_migrations();
        s
    }

    pub fn apply_migrations(&mut self) {
        self.show_focus = false;
        match self.notch_style.as_str() {
            "translucent" => self.notch_style = "glass".to_string(),
            "mica" | "" => self.notch_style = "black".to_string(),
            _ => {}
        }
        match self.companion_mode.as_str() {
            "ai" | "screen" => self.companion_mode = "calendar".to_string(),
            "tasks" | "reminder" => self.companion_mode = "focus".to_string(),
            "" | "false" => self.companion_mode = "time".to_string(),
            _ => {}
        }
        if self.focus_minutes <= 0 {
            self.focus_minutes = 25;
        }
        match self.dn_ai_primary_provider.trim().to_ascii_lowercase().as_str() {
            "router" => self.dn_ai_primary_provider = "router".to_string(),
            "gemini" => self.dn_ai_primary_provider = "gemini".to_string(),
            "openai" => self.dn_ai_primary_provider = "openai".to_string(),
            _ => self.dn_ai_primary_provider = "auto".to_string(),
        }
        if self.avatar.is_empty() {
            self.avatar = "ghost".to_string();
        }
        if self.notch_style == "glass" {
            self.hide_in_screenshots = true;
        }
        if self.dn_router_voice_model.trim().eq_ignore_ascii_case("voice") {
            self.dn_router_voice_model.clear();
        }
        if self.dn_router_convo_model.trim().eq_ignore_ascii_case("voice") {
            self.dn_router_convo_model.clear();
        }
        if self.hotkey_voice.trim().is_empty() {
            self.hotkey_voice = default_hotkey_voice();
        }
        if self.hotkey_dictate.trim().is_empty() {
            self.hotkey_dictate = default_hotkey_dictate();
        }
        if self.hotkey_ask.trim().is_empty() {
            self.hotkey_ask = default_hotkey_ask();
        }
        if self.hotkey_chat.trim().is_empty() {
            self.hotkey_chat = default_hotkey_chat();
        }
        if self.dn_ai_access_mode.trim().eq_ignore_ascii_case("manual") {
            self.dn_ai_access_mode = "manual".to_string();
        } else {
            self.dn_ai_access_mode = "auto".to_string();
        }
        if !matches!(self.notch_size.as_str(), "small" | "default" | "large") {
            self.notch_size = default_notch_size();
        }
        if self.companion_sleeps == "time" || self.companion_sleeps.trim().is_empty() {
            self.companion_sleeps = default_companion_sleeps();
        }
        if self.tts_voice.trim().is_empty() {
            self.tts_voice = default_tts_voice();
        }
    }

    pub fn notch_scale(&self) -> f32 {
        match self.notch_size.as_str() {
            "small" => 0.86,
            "large" => 1.14,
            _ => 1.0,
        }
    }

    pub fn accent_hex(&self) -> &'static str {
        if self.avatar == "photo" {
            return "#FFFFFF";
        }
        avatar_hex(&self.avatar)
    }
}

pub fn avatar_hex(id: &str) -> &'static str {
    match id {
        "ghost" => "#F4F2FA",
        "cat" => "#FF8C42",
        "blob" => "#2FCB7A",
        "clover" => "#35B8FF",
        "droid" => "#D5DBEA",
        "alien" => "#9BE85A",
        "cloud" => "#CFE6FF",
        _ => "#F4F2FA",
    }
}

pub fn avatar_rgb(id: &str) -> (u8, u8, u8) {
    hex_to_rgb(avatar_hex(id))
}

pub fn next_avatar_species(cur: &str, has_photo: bool) -> &'static str {
    let mut list: Vec<&'static str> =
        vec!["ghost", "cat", "blob", "clover", "droid", "alien", "cloud"];
    if has_photo {
        list.push("photo");
    }
    if let Some(pos) = list.iter().position(|&s| s == cur) {
        list[(pos + 1) % list.len()]
    } else {
        list[0]
    }
}

pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(244);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(242);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(250);
        return (r, g, b);
    }
    (244, 242, 250)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    #[serde(default)]
    pub id: String,
    #[serde(default, alias = "text")]
    pub label: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderItem {
    #[serde(default)]
    pub id: String,
    #[serde(default, alias = "due")]
    pub at: i64,
    #[serde(default, alias = "text")]
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub end_time: String,
    #[serde(default)]
    pub end_at: i64,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHit {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCardInfo {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvoEntry {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub at: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd_ok: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiMemoryEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub fact: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifEntry {
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub raw_app: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub launch: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub jid: String,
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub chat: String,
    #[serde(default)]
    pub logo: String,
    #[serde(default)]
    pub actions: Vec<crate::events::ToastAction>,
    #[serde(default)]
    pub inputs: Vec<crate::events::ToastInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusLogEntry {
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub minutes: i32,
}

pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use serde_json::json;

    #[test]
    fn settings_from_value_accepts_float_focus_minutes_without_resetting_toggles() {
        let s = Settings::from_value(&json!({
            "focusMinutes": 0.16666666666666666,
            "showTasks": true,
            "showAvatar": true,
            "avatar": "cloud"
        }));
        assert_eq!(s.focus_minutes, 1);
        assert!(s.show_tasks);
        assert!(s.show_avatar);
        assert_eq!(s.avatar, "cloud");
    }

    #[test]
    fn notch_scale_maps_known_sizes_and_defaults() {
        let mut s = Settings::default();
        assert_eq!(s.notch_scale(), 1.0);
        s.notch_size = "small".to_string();
        assert_eq!(s.notch_scale(), 0.86);
        s.notch_size = "large".to_string();
        assert_eq!(s.notch_scale(), 1.14);
        s.notch_size = "unknown".to_string();
        assert_eq!(s.notch_scale(), 1.0);
    }

    #[test]
    fn fresh_install_leaves_router_url_empty() {
        assert!(Settings::default().dn_router_base_url.is_empty());
        assert!(Settings::from_value(&json!({})).dn_router_base_url.is_empty());
        assert_eq!(Settings::from_value(&json!({})).dn_ai_primary_provider, "auto");
    }
}
