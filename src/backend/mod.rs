pub mod approval_gate;
pub mod audio_out;
pub mod click_through;
pub mod clipboard;
pub mod crypto;
pub mod display;
pub mod drop_target;
pub mod installer;
pub mod logfile;
pub mod lrc;
pub mod notif;
pub mod ocr;
pub mod pipe_server;
pub mod privacy;
pub mod rates;
pub mod screenshot;
pub mod smtc;
pub mod store;
pub mod system;
pub mod ui_auto;
pub mod updater;
pub mod web;
pub mod win;
pub mod window_watcher;

use crate::events::UiSender;
use crate::voice::VoiceManager;
use approval_gate::ApprovalGate;
use click_through::ClickThroughManager;
use ocr::WinRtOcr;
use pipe_server::PipeServerManager;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use store::StoreManager;
use window_watcher::WindowWatcher;

#[derive(Default)]
pub struct AskRunController {
    active_run_id: AtomicU64,
    cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
    abort_handle: Mutex<Option<tokio::task::AbortHandle>>,
    pending_approval_id: Mutex<Option<String>>,
}

impl AskRunController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_run(
        &self,
        run_id: u64,
        approval: &ApprovalGate,
        pipe: &PipeServerManager,
    ) -> Arc<AtomicBool> {
        self.cancel_active(approval, pipe);
        self.active_run_id.store(run_id, Ordering::SeqCst);
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut g) = self.cancel_flag.lock() {
            *g = Some(Arc::clone(&flag));
        }
        flag
    }

    pub fn attach_abort(&self, run_id: u64, handle: tokio::task::AbortHandle) {
        if self.active_run_id.load(Ordering::SeqCst) == run_id {
            if let Ok(mut g) = self.abort_handle.lock() {
                *g = Some(handle);
            }
            // Re-check in case cancel_active raced between initial check and mutex lock
            if self.active_run_id.load(Ordering::SeqCst) != run_id {
                if let Ok(mut g) = self.abort_handle.lock() {
                    if let Some(h) = g.take() {
                        h.abort();
                    }
                }
            }
        } else {
            handle.abort();
        }
    }

    pub fn set_pending_approval(&self, id: Option<String>) {
        if let Ok(mut g) = self.pending_approval_id.lock() {
            *g = id;
        }
    }

    pub fn cancel_active(&self, approval: &ApprovalGate, pipe: &PipeServerManager) {
        self.active_run_id.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = self.cancel_flag.lock() {
            if let Some(flag) = g.take() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(mut g) = self.abort_handle.lock() {
            if let Some(handle) = g.take() {
                handle.abort();
            }
        }
        if let Ok(mut g) = self.pending_approval_id.lock() {
            if let Some(id) = g.take() {
                if !approval.respond(&id, "deny") {
                    pipe.respond(&id, "deny");
                }
            }
        }
    }

    pub fn finish_run(&self, run_id: u64) {
        if self.active_run_id.load(Ordering::SeqCst) == run_id {
            if let Ok(mut g) = self.cancel_flag.lock() {
                *g = None;
            }
            if let Ok(mut g) = self.abort_handle.lock() {
                *g = None;
            }
            if let Ok(mut g) = self.pending_approval_id.lock() {
                *g = None;
            }
        }
    }

    pub fn active_run_id(&self) -> u64 {
        self.active_run_id.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct BackendServices {
    pub store: StoreManager,
    pub click_through: ClickThroughManager,
    pub pipe: PipeServerManager,
    pub window_watcher: WindowWatcher,
    pub approval: ApprovalGate,
    pub ask_ctrl: Arc<AskRunController>,
    pub ocr: Option<Arc<WinRtOcr>>,
    pub voice: Arc<VoiceManager>,
    pub rt: tokio::runtime::Handle,
    pub ui: UiSender,
}

#[cfg(test)]
mod tests {
    use super::AskRunController;
    use crate::backend::approval_gate::ApprovalGate;
    use crate::backend::pipe_server::PipeServerManager;
    use std::sync::atomic::Ordering;

    #[test]
    fn ask_run_controller_cancels_flag_and_denies_approval() {
        let ctrl = AskRunController::new();
        let approval = ApprovalGate::new();
        let pipe = PipeServerManager::new();

        let flag1 = ctrl.start_run(10, &approval, &pipe);
        assert!(!flag1.load(Ordering::SeqCst));
        assert_eq!(ctrl.active_run_id(), 10);

        let (aid, mut rx) = approval.register();
        ctrl.set_pending_approval(Some(aid));

        ctrl.cancel_active(&approval, &pipe);
        assert!(flag1.load(Ordering::SeqCst));
        assert_ne!(ctrl.active_run_id(), 10);
        assert_eq!(rx.try_recv(), Ok(false));

        let flag2 = ctrl.start_run(11, &approval, &pipe);
        assert!(!flag2.load(Ordering::SeqCst));
        assert_eq!(ctrl.active_run_id(), 11);
        ctrl.finish_run(11);
        assert_eq!(ctrl.active_run_id(), 11);
    }
}
