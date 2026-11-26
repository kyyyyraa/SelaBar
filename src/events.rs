use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WM_APP_WAKE: u32 = 0x8001;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HwndHandle(pub isize);

impl HwndHandle {
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool: String,
    #[serde(default, alias = "provider", alias = "client", alias = "app")]
    pub source: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub ts: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PrivacySnapshot {
    pub mic: bool,
    pub camera: bool,
    pub mic_apps: Vec<String>,
    pub camera_apps: Vec<String>,
    pub live_mic_apps: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioDeviceKind {
    #[default]
    Speaker,
    Headphones,
    Bluetooth,
    Monitor,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioOutputDevice {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub kind: AudioDeviceKind,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioOutputSnapshot {
    pub volume_pct: u8,
    pub muted: bool,
    pub devices: Vec<AudioOutputDevice>,
}

impl Default for AudioOutputSnapshot {
    fn default() -> Self {
        Self {
            volume_pct: 50,
            muted: false,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SmtcSessionInfo {
    pub source_app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub is_playing: bool,
    pub position_sec: f64,
    pub duration_sec: f64,
    pub thumbnail_bytes: Option<Vec<u8>>,
    pub thumbnail_mime: Option<String>,
    pub accent_tint: Option<(u8, u8, u8)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToastAction {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub activation_type: String,
    #[serde(default)]
    pub input_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToastInput {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub placeholder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeetingProvider {
    GoogleMeet,
    Zoom,
    Teams,
}

impl MeetingProvider {
    pub fn label(&self) -> &'static str {
        match self {
            MeetingProvider::GoogleMeet => "Google Meet",
            MeetingProvider::Zoom => "Zoom",
            MeetingProvider::Teams => "Microsoft Teams",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingLink {
    pub provider: MeetingProvider,
    pub url: String,
    pub code: String,
    pub pwd: String,
}

#[derive(Debug, Clone)]
pub struct ActiveCallInfo {
    pub app: String,
    pub caller: String,
    pub started_at_ms: u64,
    pub call_hwnd: isize,
    pub raw_app: String,
    pub decline_args: String,
}

#[derive(Debug, Clone, Default)]
pub struct NotifPayload {
    pub id: String,
    pub launch: String,
    pub app: String,
    pub raw_app: String,
    pub site: String,
    pub chat: String,
    pub title: String,
    pub body: String,
    pub logo: String,
    pub at: String,
    pub jid: String,
    pub scenario: String,
    pub is_call: bool,
    pub actions: Vec<ToastAction>,
    pub inputs: Vec<ToastInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttMode {
    Command,
    Dictate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttAbortReason {
    Command,
    Dictate,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub work_x: i32,
    pub work_y: i32,
    pub work_width: u32,
    pub work_height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryState {
    pub level: f64,
    pub charging: bool,
    pub supported: bool,
}

impl Default for BatteryState {
    fn default() -> Self {
        Self {
            level: 1.0,
            charging: false,
            supported: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveUsage {
    pub cpu: f32,
    pub gpu: f32,
    pub mem: f32,
}

#[derive(Debug, Clone, Default)]
pub struct AiLimitItem {
    pub id: String,
    pub label: String,
    pub window: String,
    pub used_pct: f32,
    pub reset_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub aumid: String,
    pub path: String,
    pub use_count: u32,
}

#[derive(Debug, Clone)]
pub struct VoiceStepStatus {
    pub label: String,
    pub state: StepProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepProgress {
    Pending,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct LrcLine {
    pub t_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct LrcResult {
    pub key: String,
    pub status: String, // "no-track" | "loading" | "synced" | "plain" | "instrumental" | "not-found" | "error"
    pub lines: Vec<LrcLine>,
    pub plain: Option<String>,
    pub track_name: String,
    pub artist_name: String,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    PrivacyChanged(PrivacySnapshot),
    FilesDragEnter,
    FilesDragLeave,
    ShelfDropped(Vec<String>),
    AiApprovalRequested(ApprovalRequest),
    AiApprovalDismissed { id: String },
    CursorMoved { x: i32, y: i32 },
    ClipboardNew {
        text: String,
        image_bmp_data_url: String,
        kind: String,
        at_ms: u64,
    },
    NotifArrived(NotifPayload),
    NotifRemoved(String),
    IncomingCallDetected(NotifPayload),
    IncomingCallDismissed,
    ActiveCallStarted(ActiveCallInfo),
    CallUiProbeResult {
        call_started_ms: u64,
        still_active: Option<bool>,
    },
    ActiveCallEnded,
    ScreenshotNew(String),
    SmtcNowPlaying(Option<SmtcSessionInfo>),
    SmtcPosition {
        pos_ms: u64,
        playing: bool,
        raw_pos_ms: u64,
        last_updated_ticks: i64,
        dur_ms: u64,
    },
    LrcLoaded(LrcResult),
    AudioOutputsUpdated(AudioOutputSnapshot),
    StoreChanged { key: String, value: Value },
    NotchTucked(bool),
    HotkeyPttStart { mode: PttMode },
    HotkeyPttAbort { mode: PttAbortReason },
    HotkeyFinishPtt,
    HotkeyVoiceStop,
    HotkeyOpenChat,
    HotkeyOpenAsk,
    HotkeyRegistrationFailures(Vec<String>),
    VoiceState {
        gen: u64,
        status: String,
        text: String,
        steps: Option<Vec<VoiceStepStatus>>,
    },
    VoiceRms([f32; 5]),
    VoiceLiveActive(bool),
    VoiceMicTestResult { ok: bool, text: String },
    WhisperDownloadPrompt,
    WhisperDownloadProgress {
        stage: String,
        downloaded_mb: f32,
        total_mb: f32,
        pct: f32,
    },
    WhisperDownloadDone { ok: bool, msg: String },
    AskHarnessStep {
        run_id: u64,
        step_label: String,
        cmd_display: Option<String>,
        done: bool,
        ok: bool,
    },
    AskInlineApproval {
        run_id: u64,
        req: ApprovalRequest,
    },
    AskResponse {
        run_id: u64,
        ok: bool,
        text: String,
        proposal: Option<ScheduleProposal>,
    },
    ConvoThinking(bool),
    ConvoHarnessStep {
        cmd: String,
        running: bool,
        ok: Option<bool>,
    },
    ConvoAiDoneNotification { title: String, body: String },
    RouterModelsResult { ok: bool, models: Vec<String>, msg: String },
    UpdateCheckDone {
        manual: bool,
        outcome: crate::backend::updater::UpdateCheckOutcome,
    },
    UpdateDownloadDone { ok: bool, msg: String },
    BatteryUpdated(BatteryState),
    NetworkChanged { connected: bool },
    LiveUsageUpdated(LiveUsage),
    PhotoPicked(String),
    WindowBoundsResynced,
    RequestRedraw,
    SetClickThrough(bool),
    CaptureStatus(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleProposal {
    pub title: String,
    pub at_iso: String, // YYYY-MM-DDTHH:MM
    pub note: String,
}

#[derive(Clone)]
pub struct UiSender {
    tx: Sender<UiEvent>,
    hwnd: std::sync::Arc<std::sync::atomic::AtomicIsize>,
}

impl UiSender {
    pub fn pair() -> (Self, Receiver<UiEvent>) {
        let (tx, rx) = unbounded();
        (
            Self {
                tx,
                hwnd: std::sync::Arc::new(std::sync::atomic::AtomicIsize::new(0)),
            },
            rx,
        )
    }

    pub fn set_hwnd(&self, hwnd: HwndHandle) {
        self.hwnd.store(hwnd.0, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn get_hwnd(&self) -> HwndHandle {
        HwndHandle(self.hwnd.load(std::sync::atomic::Ordering::SeqCst))
    }

    pub fn send(&self, ev: UiEvent) {
        let _ = self.tx.send(ev);
        let h = self.hwnd.load(std::sync::atomic::Ordering::Relaxed);
        if h != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    h as windows_sys::Win32::Foundation::HWND,
                    WM_APP_WAKE,
                    0,
                    0,
                );
            }
        }
    }
}
