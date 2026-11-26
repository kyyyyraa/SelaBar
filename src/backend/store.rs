use crate::events::{UiEvent, UiSender};
use crate::state::Settings;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn persist_atomic(file_path: &Path, data: &HashMap<String, Value>) {
    if let Ok(json_str) = serde_json::to_string_pretty(data) {
        let tmp = file_path.with_extension("json.tmp");
        if fs::write(&tmp, &json_str).is_ok() {
            if fs::rename(&tmp, file_path).is_err() {
                let _ = fs::write(file_path, &json_str);
                let _ = fs::remove_file(&tmp);
            }
        } else {
            let _ = fs::write(file_path, &json_str);
        }
    }
}

#[derive(Clone)]
pub struct StoreManager {
    file_path: PathBuf,
    data: Arc<Mutex<HashMap<String, Value>>>,
}

impl StoreManager {
    pub fn new() -> Self {
        let app_data = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        let dir = PathBuf::from(&app_data).join("SelaBar");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("store.json");

        let legacy_candidates = [
            PathBuf::from(&app_data).join("DeskNotchNative").join("store.json"),
            PathBuf::from(&app_data).join("DeskNotch").join("desknotch-store.json"),
            PathBuf::from(&app_data).join("DeskNotch").join("store.json"),
        ];

        let mut data = HashMap::new();
        let mut migrated_from_legacy = false;
        let read_target = if file_path.exists() {
            Some(file_path.clone())
        } else if let Some(found) = legacy_candidates.iter().find(|p| p.exists()) {
            migrated_from_legacy = true;
            Some(found.clone())
        } else {
            None
        };

        if let Some(p) = read_target {
            if let Ok(content) = fs::read_to_string(&p) {
                if let Ok(json_map) = serde_json::from_str::<HashMap<String, Value>>(&content) {
                    data = json_map;
                }
            }
        }

        if migrated_from_legacy && !data.is_empty() {
            persist_atomic(&file_path, &data);
        }

        Self {
            file_path,
            data: Arc::new(Mutex::new(data)),
        }
    }

    pub fn get(&self, key: &str) -> Value {
        let guard = self.data.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn set(&self, ui: Option<&UiSender>, key: impl Into<String>, value: Value) -> bool {
        let key = key.into();
        let mut guard = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let changed = match guard.get(&key) {
            Some(old_val) => old_val != &value,
            None => true,
        };

        if changed {
            guard.insert(key.clone(), value.clone());
            persist_atomic(&self.file_path, &guard);
            if let Some(sender) = ui {
                sender.send(UiEvent::StoreChanged { key, value });
            }
        }
        true
    }

    pub fn get_settings(&self) -> Settings {
        Settings::from_value(&self.get("settings"))
    }

    /// Read-modify-write settings safely, preserving unknown keys in the raw JSON map.
    pub fn patch_settings(&self, ui: Option<&UiSender>, patch: Map<String, Value>) -> Settings {
        let mut guard = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let mut obj = match guard.get("settings").and_then(|v| v.as_object()) {
            Some(m) => m.clone(),
            None => match serde_json::to_value(Settings::default()) {
                Ok(Value::Object(m)) => m,
                _ => Map::new(),
            },
        };
        for (k, v) in patch {
            if v.is_null() {
                obj.remove(&k);
            } else {
                obj.insert(k, v);
            }
        }
        for (sealed_k, plain_k) in [
            ("dnRouterKeySealed", "dnRouterKey"),
            ("dnGeminiKeySealed", "dnGeminiKey"),
            ("dnOpenaiKeySealed", "dnOpenaiKey"),
        ] {
            let has_sealed = obj
                .get(sealed_k)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if has_sealed {
                obj.remove(plain_k);
            }
        }
        let val = Value::Object(obj);
        let changed = guard.get("settings").map(|old| old != &val).unwrap_or(true);
        if changed {
            guard.insert("settings".to_string(), val.clone());
            persist_atomic(&self.file_path, &guard);
            if let Some(sender) = ui {
                sender.send(UiEvent::StoreChanged {
                    key: "settings".to_string(),
                    value: val.clone(),
                });
            }
        }
        Settings::from_value(&val)
    }

    pub fn save_settings(&self, ui: Option<&UiSender>, settings: &Settings) -> bool {
        let mut map = match self.get("settings").as_object() {
            Some(m) => m.clone(),
            None => Map::new(),
        };
        if let Ok(Value::Object(serialized)) = serde_json::to_value(settings) {
            for (k, v) in serialized {
                map.insert(k, v);
            }
        }
        self.set(ui, "settings", Value::Object(map))
    }

    fn read_sealed_api_key(
        &self,
        ui: Option<&UiSender>,
        sealed_key_name: &str,
        plain_key_name: &str,
    ) -> String {
        let raw_val = self.get("settings");
        let sealed = raw_val
            .get(sealed_key_name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let plain_legacy = raw_val
            .get(plain_key_name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if !sealed.is_empty() {
            return crate::backend::crypto::open_key(&sealed).unwrap_or_default();
        }
        if plain_legacy.is_empty() {
            return String::new();
        }
        if let Ok(plain) = crate::backend::crypto::open_key(&plain_legacy) {
            return plain;
        }
        if let Ok(new_sealed) = crate::backend::crypto::seal_key(&plain_legacy) {
            if !new_sealed.is_empty() {
                let mut patch = Map::new();
                patch.insert(sealed_key_name.to_string(), Value::String(new_sealed));
                self.patch_settings(ui, patch);
            }
        }
        plain_legacy
    }

    fn write_sealed_api_key(
        &self,
        ui: Option<&UiSender>,
        sealed_key_name: &str,
        plain_key_name: &str,
        api_key: &str,
    ) -> bool {
        let trimmed = api_key.trim();
        let mut patch = Map::new();
        if trimmed.is_empty() {
            patch.insert(sealed_key_name.to_string(), Value::Null);
            patch.insert(plain_key_name.to_string(), Value::Null);
            self.patch_settings(ui, patch);
            return false;
        }
        match crate::backend::crypto::seal_key(trimmed) {
            Ok(sealed) if !sealed.is_empty() => {
                patch.insert(sealed_key_name.to_string(), Value::String(sealed));
                patch.insert(plain_key_name.to_string(), Value::Null);
            }
            _ => {
                patch.insert(
                    plain_key_name.to_string(),
                    Value::String(trimmed.to_string()),
                );
            }
        }
        self.patch_settings(ui, patch);
        true
    }

    /// Reads the stored router API key, auto-migrating legacy plaintext keys into DPAPI.
    pub fn read_router_api_key(&self, ui: Option<&UiSender>) -> String {
        self.read_sealed_api_key(ui, "dnRouterKeySealed", "dnRouterKey")
    }

    pub fn write_router_api_key(&self, ui: Option<&UiSender>, api_key: &str) -> bool {
        self.write_sealed_api_key(ui, "dnRouterKeySealed", "dnRouterKey", api_key)
    }

    /// Reads the stored Google Gemini AI Studio API key (DPAPI-sealed).
    pub fn read_gemini_api_key(&self, ui: Option<&UiSender>) -> String {
        self.read_sealed_api_key(ui, "dnGeminiKeySealed", "dnGeminiKey")
    }

    pub fn write_gemini_api_key(&self, ui: Option<&UiSender>, api_key: &str) -> bool {
        self.write_sealed_api_key(ui, "dnGeminiKeySealed", "dnGeminiKey", api_key)
    }

    /// Reads the stored OpenAI-Compatible API key (DPAPI-sealed).
    pub fn read_openai_api_key(&self, ui: Option<&UiSender>) -> String {
        self.read_sealed_api_key(ui, "dnOpenaiKeySealed", "dnOpenaiKey")
    }

    pub fn write_openai_api_key(&self, ui: Option<&UiSender>, api_key: &str) -> bool {
        self.write_sealed_api_key(ui, "dnOpenaiKeySealed", "dnOpenaiKey", api_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealing_router_key_scrubs_plaintext_key_and_persists_atomically() {
        let dir = std::env::temp_dir().join(format!("selabar-store-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("store.json");

        let mut initial = HashMap::new();
        initial.insert(
            "settings".to_string(),
            serde_json::json!({ "dnRouterKey": "sk-legacy-plain" }),
        );
        persist_atomic(&file_path, &initial);

        let mgr = StoreManager {
            file_path: file_path.clone(),
            data: Arc::new(Mutex::new(initial)),
        };
        let mut patch = Map::new();
        patch.insert(
            "dnRouterKeySealed".to_string(),
            Value::String("sealed-blob".to_string()),
        );
        mgr.patch_settings(None, patch);

        let on_disk: HashMap<String, Value> =
            serde_json::from_str(&fs::read_to_string(&file_path).unwrap()).unwrap();
        let st_obj = on_disk.get("settings").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            st_obj.get("dnRouterKeySealed").and_then(|v| v.as_str()),
            Some("sealed-blob")
        );
        assert!(!st_obj.contains_key("dnRouterKey"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gemini_and_openai_keys_round_trip_sealed_and_scrub_plaintext() {
        let dir = std::env::temp_dir().join(format!(
            "selabar-store-multi-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("store.json");

        let mgr = StoreManager {
            file_path: file_path.clone(),
            data: Arc::new(Mutex::new(HashMap::new())),
        };
        assert!(mgr.write_gemini_api_key(None, "AIzaTestKey12345"));
        assert!(mgr.write_openai_api_key(None, "sk-openai-test-999"));

        assert_eq!(mgr.read_gemini_api_key(None), "AIzaTestKey12345");
        assert_eq!(mgr.read_openai_api_key(None), "sk-openai-test-999");

        let on_disk: HashMap<String, Value> =
            serde_json::from_str(&fs::read_to_string(&file_path).unwrap()).unwrap();
        let st_obj = on_disk.get("settings").and_then(|v| v.as_object()).unwrap();
        assert!(!st_obj.contains_key("dnGeminiKey"));
        assert!(!st_obj.contains_key("dnOpenaiKey"));
        assert!(st_obj
            .get("dnGeminiKeySealed")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()));
        assert!(st_obj
            .get("dnOpenaiKeySealed")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()));

        mgr.write_gemini_api_key(None, "");
        assert_eq!(mgr.read_gemini_api_key(None), "");
        let _ = fs::remove_dir_all(&dir);
    }
}
