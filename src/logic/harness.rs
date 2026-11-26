use crate::backend::BackendServices;
use crate::events::{ApprovalRequest, UiEvent};
use crate::logic::router::{convo_clear, list_models, model_for, RouterSurface};
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum ParsedInput {
    Command { name: String, args: String },
    Chat { text: String },
}

pub fn parse(line: &str) -> ParsedInput {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return ParsedInput::Chat {
            text: trimmed.to_string(),
        };
    }
    let after = &trimmed[1..];
    if after.is_empty() {
        return ParsedInput::Command {
            name: String::new(),
            args: String::new(),
        };
    }
    let mut parts = after.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    if !head
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return ParsedInput::Command {
            name: String::new(),
            args: String::new(),
        };
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    ParsedInput::Command {
        name: head.to_lowercase(),
        args,
    }
}

pub fn help_text() -> String {
    [
        "Perintah yang tersedia:",
        "/help - tampilkan bantuan ini",
        "/model - lihat daftar model AI (model chat ditandai *)",
        "/model <nama> - ganti model AI untuk chat",
        "/model voice <nama> - ganti model AI untuk suara",
        "/model chat <nama> - ganti model AI untuk chat",
        "/model-voice <nama> - sama dengan /model voice",
        "/model-chat <nama> - sama dengan /model chat",
        "/models - sama dengan /model",
        "/clear - hapus memori percakapan",
        "/memory - lihat/kelola memori jangka panjang AI (list|add|del|clear)",
        "/term <perintah> - jalankan perintah shell (minta izin dulu)",
    ]
    .join("\n")
}

async fn do_list_models(svc: &BackendServices) -> String {
    let chat = model_for(&svc.store, RouterSurface::Conversation);
    let voice = model_for(&svc.store, RouterSurface::Voice);
    match list_models(&svc.store, Some(&svc.ui)).await {
        Ok(list) if !list.is_empty() => {
            let mut lines = vec!["Model yang tersedia:".to_string()];
            for id in list {
                let mark = if id == chat { "* " } else { "  " };
                let suf = if id == voice { "  (suara)" } else { "" };
                lines.push(format!("{}{}{}", mark, id, suf));
            }
            lines.push(String::new());
            lines.push(format!(
                "Chat: {}",
                if chat.is_empty() { "(model bersama)" } else { &chat }
            ));
            lines.push(format!(
                "Suara: {}",
                if voice.is_empty() { "(model bersama)" } else { &voice }
            ));
            lines.join("\n")
        }
        _ => "Tidak ada model yang bisa diambil. Pastikan 9router jalan dan API key sudah diisi."
            .to_string(),
    }
}

fn do_set_model(svc: &BackendServices, name: &str, is_voice: bool) -> String {
    let target = name.trim();
    if target.is_empty() {
        return String::new();
    }
    let mut patch = Map::new();
    patch.insert(
        if is_voice {
            "dnRouterVoiceModel"
        } else {
            "dnRouterConvoModel"
        }
        .to_string(),
        Value::String(target.to_string()),
    );
    svc.store.patch_settings(Some(&svc.ui), patch);
    format!(
        "Model {} sekarang: {}.",
        if is_voice { "suara" } else { "chat" },
        target
    )
}

fn do_memory(svc: &BackendServices, args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        let list = crate::logic::memory::load(&svc.store);
        if list.is_empty() {
            return "Memori jangka panjang AI masih kosong. Pakai `/memory add <fakta>` atau ceritakan hal penting di chat.".to_string();
        }
        let mut lines = vec![format!("Memori jangka panjang AI ({}):", list.len())];
        for m in list {
            lines.push(format!("• [{}] ({}) {}", m.id, m.category, m.fact));
        }
        return lines.join("\n");
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").to_lowercase();
    let rest = parts.next().unwrap_or("").trim();

    match sub.as_str() {
        "add" | "tambah" | "save" | "simpan" => {
            if rest.is_empty() {
                return "Pakai: /memory add <fakta yang perlu diingat>".to_string();
            }
            match crate::logic::memory::save(&svc.store, Some(&svc.ui), rest, "umum") {
                Some(e) => format!("Disimpan ke memori [{}]: {}", e.id, e.fact),
                None => "Gagal menyimpan memori.".to_string(),
            }
        }
        "del" | "delete" | "rm" | "hapus" | "remove" => {
            if rest.is_empty() {
                return "Pakai: /memory del <id_atau_kata_kunci>".to_string();
            }
            let n = crate::logic::memory::delete(&svc.store, Some(&svc.ui), rest);
            if n > 0 {
                format!("Dihapus {} entri memori.", n)
            } else {
                format!("Tidak ada memori yang cocok dengan '{}'.", rest)
            }
        }
        "clear" | "reset" | "kosongkan" => {
            crate::logic::memory::clear(&svc.store, Some(&svc.ui));
            "Semua memori jangka panjang AI dihapus.".to_string()
        }
        _ => "Pakai: /memory [list | add <fakta> | del <id/kata> | clear]".to_string(),
    }
}

async fn do_term(svc: &BackendServices, cmd: &str) -> String {
    let c = cmd.trim();
    if c.is_empty() {
        return "Pakai: /term <perintah shell>".to_string();
    }

    let s = svc.store.get_settings();
    let needs_approval = s.dn_ai_access_mode.eq_ignore_ascii_case("manual")
        || !crate::logic::executor::is_read_only_shell(c);
    if needs_approval {
        let (id, rx) = svc.approval.register();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let req = ApprovalRequest {
            id: id.clone(),
            tool: "shell".to_string(),
            source: "SelaBar AI".to_string(),
            title: "Jalankan perintah?".to_string(),
            detail: format!("$ {}", c),
            cwd: String::new(),
            ts,
        };
        if s.dn_extras_ai && !s.dnd {
            crate::backend::approval_gate::play_approval_chime();
        }
        svc.ui.send(UiEvent::AiApprovalRequested(req));

        let allowed = crate::backend::approval_gate::await_decision(&svc.approval, id, rx, 45).await;
        if !allowed {
            return "Dibatalkan.".to_string();
        }
    }
    let res = crate::backend::system::run_sandboxed_shell(c).await;
    let code = res.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    let out = res.get("out").and_then(|v| v.as_str()).unwrap_or("");
    let clipped: String = out.chars().take(4000).collect();
    format!("exit {}\n{}", code, clipped)
}

pub async fn run_command(svc: &BackendServices, name: &str, args: &str) -> String {
    match name {
        "help" => help_text(),
        "models" => do_list_models(svc).await,
        "model" => {
            let mut parts = args.split_whitespace();
            let head = parts.next().unwrap_or("").to_lowercase();
            if head == "voice" || head == "suara" || head == "chat" {
                let rest: Vec<&str> = parts.collect();
                let joined = rest.join(" ");
                if joined.is_empty() {
                    do_list_models(svc).await
                } else {
                    do_set_model(svc, &joined, head == "voice" || head == "suara")
                }
            } else if args.trim().is_empty() {
                do_list_models(svc).await
            } else {
                do_set_model(svc, args, false)
            }
        }
        "model-voice" => {
            if args.trim().is_empty() {
                do_list_models(svc).await
            } else {
                do_set_model(svc, args, true)
            }
        }
        "model-chat" => {
            if args.trim().is_empty() {
                do_list_models(svc).await
            } else {
                do_set_model(svc, args, false)
            }
        }
        "clear" => {
            convo_clear(&svc.store, Some(&svc.ui));
            "Percakapan dihapus.".to_string()
        }
        "memory" | "memori" => do_memory(svc, args),
        "term" => do_term(svc, args).await,
        _ => "Perintah tidak dikenal. Bantuan: ketik /help".to_string(),
    }
}
