use crate::events::{ApprovalRequest, UiEvent, UiSender};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::oneshot;

const MAX_PIPE_LINE_BYTES: u64 = 64 * 1024;
static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(1);

pub const PIPE_NAME: &str = r"\\.\pipe\selabar-ai";
pub const LEGACY_PIPE_NAME: &str = r"\\.\pipe\desknotch-ai";

#[derive(Clone)]
pub struct PipeServerManager {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

impl PipeServerManager {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn respond(&self, id: &str, decision: &str) -> bool {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.remove(id) {
            let _ = tx.send(decision.to_string());
            true
        } else {
            false
        }
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
            let _ = tx.send("deny".to_string());
        }
    }

    pub fn start(&self, rt: &tokio::runtime::Handle, ui: UiSender) {
        for pipe_name in [PIPE_NAME, LEGACY_PIPE_NAME] {
            let pending_map = Arc::clone(&self.pending);
            let ui_pipe = ui.clone();
            rt.spawn(async move {
                loop {
                    let server = match ServerOptions::new()
                        .first_pipe_instance(false)
                        .reject_remote_clients(true)
                        .create(pipe_name)
                    {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!(
                                "[PipeServer] Failed to create named pipe {}: {}. Retrying in 2s...",
                                pipe_name, e
                            );
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue;
                        }
                    };

                    if let Err(e) = server.connect().await {
                        eprintln!("[PipeServer] Connection error ({}): {}", pipe_name, e);
                        continue;
                    }

                    let ui_c = ui_pipe.clone();
                    let pending = Arc::clone(&pending_map);

                    tokio::spawn(async move {
                        let (reader, mut writer) = tokio::io::split(server);
                        let mut lines = BufReader::new(reader.take(MAX_PIPE_LINE_BYTES)).lines();

                        let Ok(Some(line)) = lines.next_line().await else {
                            return;
                        };

                        let mut req: ApprovalRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("[PipeServer] Invalid JSON request: {}", e);
                                let _ = writer
                                    .write_all(b"{\"decision\":\"deny\",\"error\":\"invalid json\"}\n")
                                    .await;
                                return;
                            }
                        };

                        if req.id.trim().is_empty() {
                            req.id = format!(
                                "pipe-{}",
                                NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed)
                            );
                        } else {
                            req.id = req.id.chars().take(128).collect();
                        }
                        req.tool = req.tool.chars().take(64).collect();
                        req.source = req.source.chars().take(64).collect();
                        req.title = req.title.chars().take(240).collect();
                        req.detail = req.detail.chars().take(2000).collect();
                        req.cwd = req.cwd.chars().take(512).collect();

                        let req_id = req.id.clone();
                        let (tx, rx) = oneshot::channel::<String>();

                        {
                            let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
                            guard.insert(req_id.clone(), tx);
                        }

                        ui_c.send(UiEvent::AiApprovalRequested(req));

                        let decision = tokio::select! {
                            // Biased so a reply that is already queued beats a
                            // simultaneous disconnect.
                            biased;
                            res = tokio::time::timeout(Duration::from_secs(112), rx) => match res {
                                Ok(Ok(dec)) => dec,
                                Ok(Err(_)) => "deny".to_string(),
                                Err(_) => {
                                    eprintln!("[PipeServer] Approval request timed out for id: {}", req_id);
                                    {
                                        let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
                                        guard.remove(&req_id);
                                    }
                                    ui_c.send(UiEvent::AiApprovalDismissed { id: req_id });
                                    "deny".to_string()
                                }
                            },
                            _ = lines.next_line() => {
                                // The hook hung up without waiting for a decision: the
                                // human answered in the agent's own prompt, so the card
                                // is stale and has to go.
                                eprintln!("[PipeServer] Client closed pipe for id: {}", req_id);
                                {
                                    let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
                                    guard.remove(&req_id);
                                }
                                ui_c.send(UiEvent::AiApprovalDismissed { id: req_id });
                                return;
                            }
                        };

                        let response = serde_json::json!({ "decision": decision });
                        let mut resp_str = response.to_string();
                        resp_str.push('\n');

                        let _ = writer.write_all(resp_str.as_bytes()).await;
                        let _ = writer.flush().await;
                    });
                }
            });
        }
    }
}
