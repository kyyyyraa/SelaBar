use crate::backend::store::StoreManager;
use crate::events::UiSender;
use crate::state::{now_unix_ms, AiMemoryEntry};
use serde_json::Value;

pub const MEMORY_STORE_KEY: &str = "dnAiMemory";
const MAX_MEMORY_ENTRIES: usize = 50;

pub fn load(store: &StoreManager) -> Vec<AiMemoryEntry> {
    let raw = store.get(MEMORY_STORE_KEY);
    let mut list: Vec<AiMemoryEntry> = serde_json::from_value(raw).unwrap_or_default();
    list.retain(|m| !m.fact.trim().is_empty());
    if list.len() > MAX_MEMORY_ENTRIES {
        list.truncate(MAX_MEMORY_ENTRIES);
    }
    list
}

fn persist(store: &StoreManager, ui: Option<&UiSender>, list: &[AiMemoryEntry]) {
    if let Ok(v) = serde_json::to_value(list) {
        store.set(ui, MEMORY_STORE_KEY, v);
    }
}

pub fn upsert_in_list(list: &mut Vec<AiMemoryEntry>, fact: &str, category: &str, now_ms: i64) -> Option<AiMemoryEntry> {
    let clean_fact = fact.trim();
    if clean_fact.is_empty() {
        return None;
    }
    let clean_cat = {
        let c = category.trim();
        if c.is_empty() { "umum" } else { c }
    };
    let low = clean_fact.to_lowercase();
    if let Some(pos) = list.iter().position(|m| m.fact.trim().to_lowercase() == low) {
        let mut existing = list.remove(pos);
        existing.fact = clean_fact.to_string();
        existing.category = clean_cat.to_string();
        existing.at = now_ms;
        list.insert(0, existing.clone());
        return Some(existing);
    }
    let entry = AiMemoryEntry {
        id: format!("mem-{}-{}", now_ms, list.len() + 1),
        fact: clean_fact.to_string(),
        category: clean_cat.to_string(),
        at: now_ms,
    };
    list.insert(0, entry.clone());
    if list.len() > MAX_MEMORY_ENTRIES {
        list.truncate(MAX_MEMORY_ENTRIES);
    }
    Some(entry)
}

pub fn delete_in_list(list: &mut Vec<AiMemoryEntry>, id_or_query: &str) -> usize {
    let q = id_or_query.trim();
    if q.is_empty() {
        return 0;
    }
    let q_low = q.to_lowercase();
    let before = list.len();
    if list.iter().any(|m| m.id.eq_ignore_ascii_case(q)) {
        list.retain(|m| !m.id.eq_ignore_ascii_case(q));
    } else {
        list.retain(|m| !m.fact.to_lowercase().contains(&q_low));
    }
    before.saturating_sub(list.len())
}

pub fn format_prompt_block(list: &[AiMemoryEntry]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = list
        .iter()
        .take(30)
        .map(|m| {
            let cat = if m.category.trim().is_empty() {
                "umum"
            } else {
                m.category.trim()
            };
            format!("- [{}] {}", cat, m.fact.trim())
        })
        .collect();
    format!("Memori jangka panjang tentang pengguna:\n{}", lines.join("\n"))
}

pub fn save(store: &StoreManager, ui: Option<&UiSender>, fact: &str, category: &str) -> Option<AiMemoryEntry> {
    let mut list = load(store);
    let entry = upsert_in_list(&mut list, fact, category, now_unix_ms())?;
    persist(store, ui, &list);
    Some(entry)
}

pub fn delete(store: &StoreManager, ui: Option<&UiSender>, id_or_query: &str) -> usize {
    let mut list = load(store);
    let removed = delete_in_list(&mut list, id_or_query);
    if removed > 0 {
        persist(store, ui, &list);
    }
    removed
}

pub fn clear(store: &StoreManager, ui: Option<&UiSender>) {
    store.set(ui, MEMORY_STORE_KEY, Value::Array(Vec::new()));
}

pub fn prompt_block(store: &StoreManager) -> String {
    let list = load(store);
    format_prompt_block(&list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_upsert_dedup_delete_and_prompt() {
        let mut list = Vec::new();
        assert!(upsert_in_list(&mut list, "   ", "profil", 100).is_none());
        let first = upsert_in_list(&mut list, "Suka pakai Zen Browser", "preferensi", 100).unwrap();
        upsert_in_list(&mut list, "Nama user Zacky", "profil", 200).unwrap();
        assert_eq!(list.len(), 2);

        // Case-insensitive duplicate updates existing instead of creating 3rd
        let dup = upsert_in_list(&mut list, "suka pakai zen browser", "browser", 300).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(dup.id, first.id);
        assert_eq!(dup.category, "browser");

        let block = format_prompt_block(&list);
        assert!(block.contains("Memori jangka panjang tentang pengguna:"));
        assert!(block.contains("[browser] suka pakai zen browser"));
        assert!(block.contains("[profil] Nama user Zacky"));

        assert_eq!(delete_in_list(&mut list, &first.id), 1);
        assert_eq!(list.len(), 1);
        assert_eq!(delete_in_list(&mut list, "zacky"), 1);
        assert!(list.is_empty());
        assert_eq!(format_prompt_block(&list), "");
    }
}
