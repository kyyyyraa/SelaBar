use crate::backend::system::{
    direct_click_in_window, enum_visible_windows, find_active_browser_doc_for_hwnd, make_uia_i4,
    send_unicode_text, silent, WindowEnumItem, BROWSER_STEMS,
};
use crate::backend::win;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
    IUIAutomationScrollItemPattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, ToggleState_On, TreeScope_Descendants, TreeScope_Subtree,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId,
    UIA_ControlTypePropertyId, UIA_DataItemControlTypeId, UIA_EditControlTypeId,
    UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId, UIA_InvokePatternId,
    UIA_LegacyIAccessiblePatternId, UIA_ListItemControlTypeId, UIA_MenuItemControlTypeId,
    UIA_RadioButtonControlTypeId, UIA_ScrollItemPatternId, UIA_SelectionItemPatternId,
    UIA_SliderControlTypeId, UIA_SplitButtonControlTypeId, UIA_TabItemControlTypeId,
    UIA_TextControlTypeId, UIA_TogglePatternId, UIA_TreeItemControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, SetCursorPos};

#[derive(Clone, Debug)]
struct CachedRef {
    hwnd: isize,
    cx: i32,
    cy: i32,
    name: String,
    is_browser: bool,
    scroll_gen: u64,
}

static REF_CACHE: OnceLock<Mutex<HashMap<String, CachedRef>>> = OnceLock::new();
static HAS_AGENT_DESKTOP: OnceLock<bool> = OnceLock::new();
static BROWSER_SCROLL_GEN: AtomicU64 = AtomicU64::new(0);

fn current_scroll_gen() -> u64 {
    BROWSER_SCROLL_GEN.load(Ordering::Relaxed)
}

fn bump_scroll_gen() -> u64 {
    BROWSER_SCROLL_GEN.fetch_add(1, Ordering::Relaxed) + 1
}

fn ref_cache() -> &'static Mutex<HashMap<String, CachedRef>> {
    REF_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn has_agent_desktop_cli() -> bool {
    *HAS_AGENT_DESKTOP.get_or_init(|| {
        let mut cmd = Command::new("agent-desktop");
        cmd.arg("--version");
        silent(&mut cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn store_refs(prefix: &str, entries: Vec<(String, CachedRef)>) {
    if let Ok(mut g) = ref_cache().lock() {
        g.retain(|k, _| !k.starts_with(prefix));
        for (k, v) in entries {
            g.insert(k, v);
        }
    }
}

fn lookup_ref(id: &str) -> Option<CachedRef> {
    let key = id.trim();
    ref_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(key).cloned())
}

pub fn is_youtube_skip_ad_label(raw: &str) -> bool {
    let norm: String = raw
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if norm.is_empty() || norm.len() > 42 {
        return false;
    }
    const EXCLUDE: &[&str] = &[
        "skip navigation",
        "lewati navigasi",
        "skip to",
        "lewati ke",
        "skip forward",
        "skip backward",
        "skip intro",
        "lewati intro",
        "skip recap",
        "dalam ",
        " in ",
    ];
    if EXCLUDE.iter().any(|bad| norm.contains(bad)) {
        return false;
    }
    matches!(
        norm.as_str(),
        "lewati iklan" | "skip ad" | "skip ads" | "lewati" | "skip"
    ) || norm.contains("lewati iklan")
        || norm.contains("skip ad")
        || norm.contains("skip ads")
}

unsafe fn extract_uia_label(
    el: &IUIAutomationElement,
    ct: i32,
    text_cond: Option<&IUIAutomationCondition>,
) -> String {
    let clean = |raw: String| -> String {
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    if let Ok(nm) = el.CurrentName() {
        let s = clean(nm.to_string());
        if !s.is_empty() {
            return s;
        }
    }
    if let Ok(ht) = el.CurrentHelpText() {
        let s = clean(ht.to_string());
        if !s.is_empty() {
            return s;
        }
    }
    if let Ok(acc) =
        el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId)
    {
        if let Ok(nm) = acc.CurrentName() {
            let s = clean(nm.to_string());
            if !s.is_empty() {
                return s;
            }
        }
        if let Ok(desc) = acc.CurrentDescription() {
            let s = clean(desc.to_string());
            if !s.is_empty() {
                return s;
            }
        }
        if let Ok(val) = acc.CurrentValue() {
            let s = clean(val.to_string());
            if !s.is_empty() && s.len() <= 120 {
                return s;
            }
        }
    }
    if ct != UIA_TextControlTypeId.0 && ct != UIA_GroupControlTypeId.0 {
        if let Some(cond) = text_cond {
            if let Ok(child_txt) = el.FindFirst(TreeScope_Subtree, cond) {
                if let Ok(nm) = child_txt.CurrentName() {
                    let s = clean(nm.to_string());
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
        }
    }
    String::new()
}

unsafe fn invoke_or_click_element(
    el: &IUIAutomationElement,
    hwnd: isize,
    cx: i32,
    cy: i32,
    prefer_direct_click: bool,
) -> bool {
    if !prefer_direct_click {
        if let Ok(sel) =
            el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
        {
            if sel.Select().is_ok() {
                return true;
            }
        }
        if let Ok(inv) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            if inv.Invoke().is_ok() {
                return true;
            }
        }
        if let Ok(acc) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
            UIA_LegacyIAccessiblePatternId,
        ) {
            if acc.DoDefaultAction().is_ok() {
                return true;
            }
        }
    }
    let _ = el.SetFocus();
    direct_click_in_window(hwnd, cx, cy);
    true
}

fn ensure_window_restored_and_rect(hwnd: isize) -> RECT {
    let mut wr = RECT::default();
    if hwnd == 0 {
        return wr;
    }
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = GetWindowRect(h, &mut wr);
        let iconic = windows::Win32::UI::WindowsAndMessaging::IsIconic(h).as_bool();
        if iconic || wr.left < -10000 || (wr.right - wr.left) < 120 {
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                h,
                windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
            );
            win::focus_hwnd(hwnd);
            std::thread::sleep(Duration::from_millis(260));
            let _ = GetWindowRect(h, &mut wr);
        }
    }
    wr
}

fn find_browser_window<'a>(
    windows: &'a [WindowEnumItem],
    browser_hint: Option<&str>,
    prefer_youtube: bool,
) -> Option<&'a WindowEnumItem> {
    let tok = browser_hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(crate::backend::system::normalize_browser_token)
        .unwrap_or_default();
    if !tok.is_empty() {
        return windows
            .iter()
            .find(|w| w.stem == tok || w.stem.contains(&tok));
    }
    if prefer_youtube {
        if let Some(yt) = windows
            .iter()
            .find(|w| BROWSER_STEMS.contains(&w.stem.as_str()) && w.title.to_lowercase().contains("youtube"))
        {
            return Some(yt);
        }
    }
    windows
        .iter()
        .find(|w| BROWSER_STEMS.contains(&w.stem.as_str()))
}

pub(crate) fn compute_wheel_delta(dir: &str, amount: Option<i32>) -> i32 {
    let notches = amount.unwrap_or(5).clamp(1, 12);
    if dir.contains("up") || dir.contains("atas") {
        notches * 120
    } else {
        -notches * 120
    }
}

unsafe fn send_mouse_wheel_in_bounds(hwnd: isize, bounds: RECT, wheel_delta: i32) {
    let cx = (bounds.left + bounds.right) / 2;
    let cy = (bounds.top + bounds.bottom) / 2;
    let mut saved = POINT::default();
    let has_saved = GetCursorPos(&mut saved).is_ok();
    win::focus_hwnd(hwnd);
    let _ = SetCursorPos(cx, cy);
    std::thread::sleep(Duration::from_millis(18));
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: wheel_delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let _ = SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    std::thread::sleep(Duration::from_millis(24));
    if has_saved {
        let _ = SetCursorPos(saved.x, saved.y);
    }
}

unsafe fn scroll_element_into_view_and_center(
    el: &IUIAutomationElement,
    bounds: RECT,
) -> Option<(i32, i32)> {
    let mut br = el.CurrentBoundingRectangle().ok()?;
    let need_scroll = el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false)
        || br.top < bounds.top + 28
        || br.bottom > bounds.bottom - 28;
    if need_scroll {
        if let Ok(sc) = el.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId) {
            if sc.ScrollIntoView().is_ok() {
                std::thread::sleep(Duration::from_millis(75));
                if let Ok(new_br) = el.CurrentBoundingRectangle() {
                    br = new_br;
                }
            }
        }
    }
    if br.right <= bounds.left || br.left >= bounds.right || br.bottom <= bounds.top || br.top >= bounds.bottom {
        return None;
    }
    let cx = ((br.left + br.right) / 2).clamp(bounds.left + 2, bounds.right - 2);
    let cy = ((br.top + br.bottom) / 2).clamp(bounds.top + 2, bounds.bottom - 2);
    Some((cx, cy))
}

unsafe fn build_or_control_type_condition(
    uia: &IUIAutomation,
    types: &[(i32, &str)],
) -> Option<IUIAutomationCondition> {
    let mut acc: Option<IUIAutomationCondition> = None;
    for &(ct, _) in types {
        let v = make_uia_i4(ct);
        let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v).ok()?;
        acc = Some(match acc {
            None => cond,
            Some(prev) => uia.CreateOrCondition(&prev, &cond).ok()?,
        });
    }
    acc
}

struct CollectedUiItem {
    role: String,
    name: String,
    state_tag: String,
    cx: i32,
    cy: i32,
    el: IUIAutomationElement,
}

fn format_interactive_tag(name: &str, state_tag: &str) -> String {
    if state_tag.is_empty() {
        format!("\"{}\"", name)
    } else {
        format!("\"{}\" {}", name, state_tag)
    }
}

fn read_state_and_value(el: &IUIAutomationElement, ct: i32) -> String {
    unsafe {
        if matches!(
            ct,
            x if x == UIA_RadioButtonControlTypeId.0
                || x == UIA_CheckBoxControlTypeId.0
                || x == UIA_ListItemControlTypeId.0
                || x == UIA_TabItemControlTypeId.0
        ) {
            if let Ok(sel) =
                el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
            {
                if sel.CurrentIsSelected().map(|b| b.as_bool()).unwrap_or(false) {
                    return "[checked]".to_string();
                }
            }
            if let Ok(tog) =
                el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            {
                if tog.CurrentToggleState().ok() == Some(ToggleState_On) {
                    return "[checked]".to_string();
                }
            }
        }
        if ct == UIA_EditControlTypeId.0 || ct == UIA_ComboBoxControlTypeId.0 {
            if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if let Ok(v) = vp.CurrentValue() {
                    let clean_v: String = v
                        .to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(40)
                        .collect();
                    if !clean_v.is_empty() {
                        return format!("val=\"{}\"", clean_v);
                    }
                }
            }
        }
    }
    String::new()
}

fn collect_elements_single_pass(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    bounds: RECT,
    types: &[(i32, &str)],
    include_text: bool,
    query_low: Option<&str>,
    max_interactive: usize,
    max_text: usize,
) -> (Vec<CollectedUiItem>, Vec<CollectedUiItem>) {
    let mut scan_types = types.to_vec();
    if include_text && !scan_types.iter().any(|(ct, _)| *ct == UIA_TextControlTypeId.0) {
        scan_types.push((UIA_TextControlTypeId.0, "Text"));
    }

    let role_of = |ct_id: i32| -> Option<&str> {
        scan_types.iter().find(|&&(id, _)| id == ct_id).map(|&(_, r)| r)
    };

    let mut interactive = Vec::new();
    let mut texts = Vec::new();
    let mut seen: HashSet<(i32, i32, bool, String)> = HashSet::new();

    unsafe {
        let text_cond = uia
            .CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &make_uia_i4(UIA_TextControlTypeId.0),
            )
            .ok();

        let or_cond = build_or_control_type_condition(uia, &scan_types);
        let nodes = if let Some(cond) = or_cond {
            root.FindAll(TreeScope_Descendants, &cond).ok()
        } else {
            None
        };

        let mut handle_el = |el: IUIAutomationElement, ct: i32, role: &str| -> bool {
            let is_text = ct == UIA_TextControlTypeId.0;
            if is_text && texts.len() >= max_text * 2 {
                return interactive.len() >= max_interactive * 2
                    && (!include_text || texts.len() >= max_text * 2);
            }
            if !is_text && interactive.len() >= max_interactive * 2 {
                return interactive.len() >= max_interactive * 2
                    && (!include_text || texts.len() >= max_text * 2);
            }
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                return false;
            }
            if ct == UIA_GroupControlTypeId.0 {
                let focusable = el
                    .CurrentIsKeyboardFocusable()
                    .map(|b| b.as_bool())
                    .unwrap_or(false);
                let has_invoke = el
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .is_ok();
                let has_def = el
                    .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    )
                    .ok()
                    .and_then(|a| a.CurrentDefaultAction().ok())
                    .is_some_and(|s| !s.to_string().trim().is_empty());
                if !focusable && !has_invoke && !has_def {
                    return false;
                }
            }
            let name_clean = extract_uia_label(&el, ct, text_cond.as_ref());
            if name_clean.is_empty() {
                return false;
            }
            let name_low = name_clean.to_lowercase();
            if !is_text {
                if let Some(q) = query_low {
                    if !name_low.contains(q) && !role.to_lowercase().contains(q) {
                        return false;
                    }
                }
            }
            let Ok(br) = el.CurrentBoundingRectangle() else {
                return false;
            };
            let bw = br.right - br.left;
            let bh = br.bottom - br.top;
            if bw < 6
                || bh < 6
                || br.right <= bounds.left
                || br.left >= bounds.right
                || br.bottom <= bounds.top
                || br.top >= bounds.bottom
            {
                return false;
            }
            let cx = ((br.left + br.right) / 2).clamp(bounds.left + 2, bounds.right - 2);
            let cy = ((br.top + br.bottom) / 2).clamp(bounds.top + 2, bounds.bottom - 2);
            let key = (cx / 4, cy / 4, is_text, name_low);
            if seen.insert(key) {
                let short_name: String = name_clean.chars().take(160).collect();
                let state_tag = if is_text {
                    String::new()
                } else {
                    read_state_and_value(&el, ct)
                };
                let item = CollectedUiItem {
                    role: role.to_string(),
                    name: short_name,
                    state_tag,
                    cx,
                    cy,
                    el,
                };
                if is_text {
                    texts.push(item);
                } else {
                    interactive.push(item);
                }
            }
            interactive.len() >= max_interactive * 2
                && (!include_text || texts.len() >= max_text * 2)
        };

        if let Some(arr) = nodes {
            let count = arr.Length().unwrap_or(0).min(1600);
            for i in 0..count {
                let Ok(el) = arr.GetElement(i) else { continue };
                let Ok(ct_id) = el.CurrentControlType() else { continue };
                if let Some(role) = role_of(ct_id.0) {
                    if handle_el(el, ct_id.0, role) {
                        break;
                    }
                }
            }
        } else {
            for &(ct, role) in &scan_types {
                let v = make_uia_i4(ct);
                let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                    continue;
                };
                let Ok(arr) = root.FindAll(TreeScope_Descendants, &cond) else {
                    continue;
                };
                let max_scan = if ct == UIA_GroupControlTypeId.0 { 120 } else { 450 };
                let count = arr.Length().unwrap_or(0).min(max_scan);
                for i in 0..count {
                    let Ok(el) = arr.GetElement(i) else { continue };
                    let _ = handle_el(el, ct, role);
                }
            }
        }
    }

    interactive.sort_by_key(|it| (it.cy / 14, it.cx));
    interactive.truncate(max_interactive);
    texts.sort_by_key(|it| (it.cy / 14, it.cx));
    texts.truncate(max_text * 2);

    // Filter out Text nodes that duplicate an interactive label on the exact same visual row.
    let mut clean_texts = Vec::with_capacity(max_text);
    for t in texts {
        let t_low = t.name.to_lowercase();
        let dup_interactive = interactive.iter().any(|it| {
            (it.cy - t.cy).abs() <= 16 && it.name.to_lowercase() == t_low
        });
        if !dup_interactive {
            clean_texts.push(t);
            if clean_texts.len() >= max_text {
                break;
            }
        }
    }

    (interactive, clean_texts)
}

fn collect_elements_in_root(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    bounds: RECT,
    types: &[(i32, &str)],
    query_low: Option<&str>,
    limit: usize,
) -> Vec<(String, String, i32, i32, IUIAutomationElement)> {
    let (interactive, _) =
        collect_elements_single_pass(uia, root, bounds, types, false, query_low, limit, 0);
    interactive
        .into_iter()
        .map(|it| (it.role, it.name, it.cx, it.cy, it.el))
        .collect()
}

fn browser_snapshot_blocking(
    query: Option<&str>,
    browser: Option<&str>,
    include_text: bool,
) -> Result<String, String> {
    let windows = unsafe { enum_visible_windows() };
    let Some(bw) = find_browser_window(&windows, browser, false) else {
        return Err(if browser.map(str::trim).unwrap_or("").is_empty() {
            "Tidak ada jendela browser yang terbuka.".to_string()
        } else {
            format!(
                "Jendela browser '{}' tidak ditemukan terbuka.",
                browser.unwrap_or_default().trim()
            )
        });
    };
    let hwnd = bw.hwnd;
    let stem = bw.stem.clone();
    let title = bw.title.clone();
    let q_low = query
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    std::thread::spawn(move || unsafe {
        let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let res = (|| -> Result<String, String> {
            let uia = CoCreateInstance::<_, IUIAutomation>(
                &CUIAutomation,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(|e| format!("UIA init gagal: {:?}", e))?;

            let mut wr = ensure_window_restored_and_rect(hwnd);
            let is_gecko = matches!(stem.as_str(), "zen" | "zenbrowser" | "firefox");

            let mut root = uia
                .ElementFromHandle(HWND(hwnd as *mut _))
                .map_err(|_| "Gagal membaca elemen browser.".to_string())?;
            let mut doc_opt = find_active_browser_doc_for_hwnd(&uia, &root, hwnd, wr, None);
            if doc_opt.is_none() {
                // Wake lazy Gecko/Chromium UIA tree once if no document was exposed yet.
                win::focus_hwnd(hwnd);
                std::thread::sleep(Duration::from_millis(200));
                let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
                if let Ok(r2) = uia.ElementFromHandle(HWND(hwnd as *mut _)) {
                    root = r2;
                    doc_opt = find_active_browser_doc_for_hwnd(&uia, &root, hwnd, wr, None);
                }
            }

            let types = [
                (UIA_ButtonControlTypeId.0, "Button"),
                (UIA_HyperlinkControlTypeId.0, "Link"),
                (UIA_EditControlTypeId.0, "Edit"),
                (UIA_ListItemControlTypeId.0, "ListItem"),
                (UIA_DataItemControlTypeId.0, "DataItem"),
                (UIA_CheckBoxControlTypeId.0, "CheckBox"),
                (UIA_ComboBoxControlTypeId.0, "ComboBox"),
                (UIA_MenuItemControlTypeId.0, "MenuItem"),
                (UIA_TabItemControlTypeId.0, "Tab"),
                (UIA_SliderControlTypeId.0, "Slider"),
                (UIA_RadioButtonControlTypeId.0, "Radio"),
                (UIA_GroupControlTypeId.0, "Group"),
            ];

            let (mut search_root, mut bounds) = match &doc_opt {
                Some((d, r)) => (d.clone(), *r),
                None => (
                    root.clone(),
                    RECT {
                        left: wr.left + 44,
                        top: wr.top + 44,
                        right: wr.right,
                        bottom: wr.bottom,
                    },
                ),
            };

            let (mut items, mut text_nodes) = collect_elements_single_pass(
                &uia,
                &search_root,
                bounds,
                &types,
                include_text,
                q_low.as_deref(),
                100,
                90,
            );
            if is_gecko && items.is_empty() {
                win::focus_hwnd(hwnd);
                std::thread::sleep(Duration::from_millis(160));
                let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
                if let Ok(r2) = uia.ElementFromHandle(HWND(hwnd as *mut _)) {
                    root = r2;
                    doc_opt = find_active_browser_doc_for_hwnd(&uia, &root, hwnd, wr, None);
                    let content_fb = RECT {
                        left: wr.left + 44,
                        top: wr.top + 44,
                        right: wr.right,
                        bottom: wr.bottom,
                    };
                    let (sr, rb) = match &doc_opt {
                        Some((d, r)) => (d.clone(), *r),
                        None => (root.clone(), content_fb),
                    };
                    search_root = sr;
                    bounds = rb;
                    let pair = collect_elements_single_pass(
                        &uia,
                        &search_root,
                        bounds,
                        &types,
                        include_text,
                        q_low.as_deref(),
                        100,
                        90,
                    );
                    items = pair.0;
                    text_nodes = pair.1;
                }
            }
            if items.is_empty() && doc_opt.is_some() {
                let content_fb = RECT {
                    left: wr.left + 44,
                    top: wr.top + 44,
                    right: wr.right,
                    bottom: wr.bottom,
                };
                let pair = collect_elements_single_pass(
                    &uia,
                    &root,
                    content_fb,
                    &types,
                    include_text,
                    q_low.as_deref(),
                    100,
                    90,
                );
                items = pair.0;
                text_nodes = pair.1;
            }

            if items.is_empty() && text_nodes.is_empty() {
                return Ok(format!(
                    "Browser ({}): {}\nTidak ada elemen interaktif UIA yang cocok terlihat.",
                    stem, title
                ));
            }

            let gen = current_scroll_gen();
            let mut cache_entries = Vec::with_capacity(items.len());
            enum LineKind {
                Interactive {
                    ref_id: String,
                    role: String,
                    name: String,
                    state_tag: String,
                    cx: i32,
                    cy: i32,
                },
                Text {
                    name: String,
                    cx: i32,
                    cy: i32,
                },
            }

            let mut merged: Vec<LineKind> = Vec::with_capacity(items.len() + text_nodes.len());
            for (i, it) in items.into_iter().enumerate() {
                let ref_id = format!("@b{}", i + 1);
                cache_entries.push((
                    ref_id.clone(),
                    CachedRef {
                        hwnd,
                        cx: it.cx,
                        cy: it.cy,
                        name: it.name.clone(),
                        is_browser: true,
                        scroll_gen: gen,
                    },
                ));
                merged.push(LineKind::Interactive {
                    ref_id,
                    role: it.role,
                    name: it.name,
                    state_tag: it.state_tag,
                    cx: it.cx,
                    cy: it.cy,
                });
            }
            for t in text_nodes {
                merged.push(LineKind::Text {
                    name: t.name,
                    cx: t.cx,
                    cy: t.cy,
                });
            }
            // Interleave Text + @bN strictly top-to-bottom, left-to-right so prompts sit right above their inputs.
            merged.sort_by_key(|entry| match entry {
                LineKind::Text { cy, cx, .. } => (cy / 14, *cx, 0u8),
                LineKind::Interactive { cy, cx, .. } => (cy / 14, *cx, 1u8),
            });

            store_refs("@b", cache_entries);
            let mut lines = Vec::with_capacity(merged.len() + 2);
            lines.push(format!("Browser ({}): {}", stem, title));
            for entry in merged {
                match entry {
                    LineKind::Interactive {
                        ref_id,
                        role,
                        name,
                        state_tag,
                        cx,
                        cy,
                    } => {
                        let lbl = format_interactive_tag(&name, &state_tag);
                        lines.push(format!("{} [{}] {} ({},{})", ref_id, role, lbl, cx, cy));
                    }
                    LineKind::Text { name, .. } => {
                        lines.push(name);
                    }
                }
            }
            Ok(lines.join("\n"))
        })();

        if init_ok {
            CoUninitialize();
        }
        res
    })
    .join()
    .unwrap_or_else(|_| Err("Worker UIA browser terhenti.".to_string()))
}

pub async fn browser_snapshot(
    query: Option<String>,
    browser: Option<String>,
    include_text: bool,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        browser_snapshot_blocking(query.as_deref(), browser.as_deref(), include_text)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn find_best_target_in_root(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    bounds: RECT,
    types: &[(i32, &str)],
    target_low: &str,
) -> Option<(u8, bool, String, i32, i32, IUIAutomationElement)> {
    let items = collect_elements_in_root(uia, root, bounds, types, None, 200);
    let mut best: Option<(u8, bool, String, i32, i32, IUIAutomationElement)> = None;
    for (role, name, cx, cy, el) in items {
        let low = name.to_lowercase();
        let tier = if low == target_low {
            3
        } else if low.starts_with(target_low) {
            2
        } else if low.contains(target_low) {
            1
        } else {
            0
        };
        if tier == 0 {
            continue;
        }
        let is_text = role == "Text";
        let better = match &best {
            None => true,
            Some((bt, bt_text, _, _, _, _)) => (tier > *bt) || (tier == *bt && *bt_text && !is_text),
        };
        if better {
            best = Some((tier, is_text, name, cx, cy, el));
            if tier == 3 && !is_text {
                break;
            }
        }
    }
    best
}

fn browser_viewport_bounds(hwnd: isize) -> RECT {
    let mut wr = RECT::default();
    unsafe {
        let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
    }
    RECT {
        left: wr.left + 44,
        top: wr.top + 88,
        right: (wr.right - 20).max(wr.left + 120),
        bottom: (wr.bottom - 20).max(wr.top + 140),
    }
}

fn perform_browser_scroll_blocking(hwnd: isize, dir: &str, amount: Option<i32>) -> Result<String, String> {
    let d = dir.trim().to_lowercase();
    let _ = ensure_window_restored_and_rect(hwnd);
    win::focus_hwnd(hwnd);
    std::thread::sleep(Duration::from_millis(45));
    bump_scroll_gen();

    if d.contains("top") || d.contains("awal") {
        win::send_keys("ctrl+home")?;
        return Ok("Halaman digulir ke paling atas.".to_string());
    }
    if d.contains("bottom") || d.contains("bawah_akhir") || d == "end" {
        win::send_keys("ctrl+end")?;
        return Ok("Halaman digulir ke paling bawah.".to_string());
    }
    let bounds = browser_viewport_bounds(hwnd);
    let delta = compute_wheel_delta(&d, amount);
    unsafe {
        send_mouse_wheel_in_bounds(hwnd, bounds, delta);
    }
    Ok(format!(
        "Halaman digulir {} ({} langkah).",
        if delta > 0 { "ke atas" } else { "ke bawah" },
        amount.unwrap_or(5).clamp(1, 12)
    ))
}

unsafe fn perform_type_after_focus(txt: &str, clear: bool, enter_after: bool) {
    if clear {
        let _ = win::send_keys("ctrl+a");
        std::thread::sleep(Duration::from_millis(20));
        let _ = win::send_keys("backspace");
        std::thread::sleep(Duration::from_millis(25));
    }
    if !txt.is_empty() {
        std::thread::sleep(Duration::from_millis(45));
        send_unicode_text(txt);
    }
    if enter_after {
        std::thread::sleep(Duration::from_millis(40));
        let _ = win::send_keys("enter");
    }
}

fn append_snapshot_if_requested(
    mut msg: String,
    browser: Option<&str>,
    snapshot_after: bool,
    scrolled: bool,
) -> Result<String, String> {
    if !snapshot_after {
        return Ok(msg);
    }
    std::thread::sleep(Duration::from_millis(if scrolled { 150 } else { 80 }));
    if let Ok(snap) = browser_snapshot_blocking(None, browser, true) {
        msg.push_str("\n\n--- Snapshot Terbaru ---\n");
        msg.push_str(&snap);
    }
    Ok(msg)
}

#[allow(clippy::too_many_arguments)]
pub async fn browser_act(
    action: String,
    browser: Option<String>,
    target: Option<String>,
    text: Option<String>,
    key: Option<String>,
    actions: Option<Vec<Value>>,
    amount: Option<i32>,
    clear: Option<bool>,
    snapshot_after: Option<bool>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let act = action.trim().to_lowercase();
        let snap_after = snapshot_after.unwrap_or(false);
        let clear_first = clear.unwrap_or(false);
        let windows = unsafe { enum_visible_windows() };

        if act == "batch" {
            let Some(steps) = actions.filter(|a| !a.is_empty()) else {
                return Err("parameter `actions` kosong untuk action=batch".to_string());
            };
            let Some(bw) = find_browser_window(&windows, browser.as_deref(), false) else {
                return Err("Tidak ada jendela browser yang terbuka.".to_string());
            };
            let hwnd = bw.hwnd;
            let _ = ensure_window_restored_and_rect(hwnd);
            win::focus_hwnd(hwnd);
            std::thread::sleep(Duration::from_millis(55));

            let mut results: Vec<String> = Vec::new();
            let mut batch_scrolled = false;
            let expected_gen = current_scroll_gen();

            for (i, item) in steps.into_iter().take(15).enumerate() {
                let sub_act = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("click")
                    .trim()
                    .to_lowercase();
                let sub_tgt = item
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let sub_txt = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sub_key = item
                    .get("key")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let sub_clear = item.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
                let sub_amount = item
                    .get("amount")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);

                match sub_act.as_str() {
                    "scroll" => {
                        let dir = sub_tgt
                            .as_deref()
                            .or(sub_key.as_deref())
                            .unwrap_or("down");
                        match perform_browser_scroll_blocking(hwnd, dir, sub_amount) {
                            Ok(m) => {
                                batch_scrolled = true;
                                results.push(format!("{}. {}", i + 1, m));
                            }
                            Err(e) => {
                                results.push(format!("{}. Gagal scroll: {}", i + 1, e));
                                break;
                            }
                        }
                    }
                    "key" => {
                        if let Some(k) = sub_key.as_deref().or(sub_tgt.as_deref()) {
                            let _ = win::send_keys(k);
                            std::thread::sleep(Duration::from_millis(35));
                            results.push(format!("{}. Tombol {}", i + 1, k));
                        }
                    }
                    "click" | "type" => {
                        let Some(ref tid) = sub_tgt else {
                            results.push(format!("{}. Dilewati (target kosong)", i + 1));
                            continue;
                        };
                        if tid.starts_with("@b") {
                            if batch_scrolled {
                                results.push(format!(
                                    "{}. Berhenti: {} tidak bisa dipakai setelah scroll dalam satu batch (ambil snapshot baru).",
                                    i + 1, tid
                                ));
                                break;
                            }
                            let Some(cref) = lookup_ref(tid) else {
                                results.push(format!("{}. Ref {} tidak ditemukan.", i + 1, tid));
                                continue;
                            };
                            if !cref.is_browser || cref.scroll_gen != expected_gen {
                                results.push(format!(
                                    "{}. Ref {} usang karena halaman sudah di-scroll.",
                                    i + 1, tid
                                ));
                                break;
                            }
                            unsafe {
                                direct_click_in_window(cref.hwnd, cref.cx, cref.cy);
                                if sub_act == "type" {
                                    let enter_after = sub_key
                                        .as_deref()
                                        .is_some_and(|k| k.eq_ignore_ascii_case("enter"));
                                    perform_type_after_focus(&sub_txt, sub_clear, enter_after);
                                    results.push(format!("{}. Isi \"{}\"", i + 1, cref.name));
                                } else {
                                    std::thread::sleep(Duration::from_millis(35));
                                    results.push(format!("{}. Klik \"{}\"", i + 1, cref.name));
                                }
                            }
                        } else {
                            results.push(format!(
                                "{}. Target '{}' harus memakai ref @bN di dalam batch.",
                                i + 1, tid
                            ));
                        }
                    }
                    other => {
                        results.push(format!("{}. Aksi '{}' tidak didukung di batch.", i + 1, other));
                    }
                }
            }

            let summary = if results.is_empty() {
                "Batch selesai tanpa aksi.".to_string()
            } else {
                results.join("\n")
            };
            return append_snapshot_if_requested(summary, browser.as_deref(), snap_after, batch_scrolled);
        }

        if act == "skip_ad" {
            let Some(bw) = find_browser_window(&windows, browser.as_deref(), true) else {
                return Err("Tidak ada jendela browser yang terbuka.".to_string());
            };
            let hwnd = bw.hwnd;
            return std::thread::spawn(move || unsafe {
                let mut wr = ensure_window_restored_and_rect(hwnd);
                win::focus_hwnd(hwnd);
                std::thread::sleep(Duration::from_millis(70));
                let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
                let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
                let res = (|| -> Result<String, String> {
                    let uia = CoCreateInstance::<_, IUIAutomation>(
                        &CUIAutomation,
                        None,
                        CLSCTX_INPROC_SERVER,
                    )
                    .map_err(|e| format!("UIA init gagal: {:?}", e))?;
                    let Ok(root) = uia.ElementFromHandle(HWND(hwnd as *mut _)) else {
                        return Err("Gagal membaca jendela browser.".to_string());
                    };
                    let (search_root, bounds) =
                        match find_active_browser_doc_for_hwnd(&uia, &root, hwnd, wr, Some("youtube")) {
                            Some((d, r)) => (d, r),
                            None => (root, wr),
                        };
                    let text_cond = uia
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &make_uia_i4(UIA_TextControlTypeId.0),
                        )
                        .ok();

                    for ct in [
                        UIA_ButtonControlTypeId.0,
                        UIA_HyperlinkControlTypeId.0,
                        UIA_GroupControlTypeId.0,
                        UIA_TextControlTypeId.0,
                    ] {
                        let v = make_uia_i4(ct);
                        let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v) else {
                            continue;
                        };
                        let Ok(nodes) = search_root.FindAll(TreeScope_Descendants, &cond) else {
                            continue;
                        };
                        let count = nodes.Length().unwrap_or(0).min(450);
                        for i in 0..count {
                            let Ok(el) = nodes.GetElement(i) else {
                                continue;
                            };
                            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
                                continue;
                            }
                            let nm = extract_uia_label(&el, ct, text_cond.as_ref());
                            if !is_youtube_skip_ad_label(&nm) {
                                continue;
                            }
                            let Ok(br) = el.CurrentBoundingRectangle() else {
                                continue;
                            };
                            if (br.right - br.left) < 12
                                || (br.bottom - br.top) < 12
                                || br.right <= bounds.left
                                || br.left >= bounds.right
                                || br.bottom <= bounds.top
                                || br.top >= bounds.bottom
                            {
                                continue;
                            }
                            let cx = (br.left + br.right) / 2;
                            let cy = (br.top + br.bottom) / 2;
                            if invoke_or_click_element(&el, hwnd, cx, cy, true) {
                                return Ok("Iklan YouTube dilewati.".to_string());
                            }
                        }
                    }
                    Ok("Tombol lewati iklan belum muncul atau tidak ada iklan aktif.".to_string())
                })();
                if init_ok {
                    CoUninitialize();
                }
                res
            })
            .join()
            .unwrap_or_else(|_| Err("Worker skip iklan terhenti.".to_string()));
        }

        if act == "navigate" {
            let raw_url = text
                .as_deref()
                .or(target.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "parameter URL kosong untuk navigate".to_string())?;
            let nav_url = if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
                raw_url.to_string()
            } else {
                format!("https://{}", raw_url)
            };
            let Some(bw) = find_browser_window(&windows, browser.as_deref(), false) else {
                return Err("Tidak ada jendela browser yang terbuka.".to_string());
            };
            let _ = ensure_window_restored_and_rect(bw.hwnd);
            if crate::backend::system::navigate_tab_to_url_blocking(bw.hwnd, &nav_url) {
                bump_scroll_gen();
                let msg = format!("Navigasi tab ke {} di {}.", nav_url, bw.stem);
                return append_snapshot_if_requested(msg, browser.as_deref(), snap_after, true);
            }
            return Err("Gagal menavigasi tab browser.".to_string());
        }

        if act == "key" {
            let k = key
                .as_deref()
                .or(target.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "parameter key kosong".to_string())?
                .to_string();
            let Some(bw) = find_browser_window(&windows, browser.as_deref(), false) else {
                return Err("Tidak ada jendela browser yang terbuka.".to_string());
            };
            let _ = ensure_window_restored_and_rect(bw.hwnd);
            win::focus_hwnd(bw.hwnd);
            std::thread::sleep(Duration::from_millis(50));
            win::send_keys(&k)?;
            return append_snapshot_if_requested(
                format!("Tombol {} dikirim ke browser.", k),
                browser.as_deref(),
                snap_after,
                false,
            );
        }

        if act == "scroll" {
            let dir = target
                .as_deref()
                .or(key.as_deref())
                .unwrap_or("down")
                .trim()
                .to_lowercase();
            let Some(bw) = find_browser_window(&windows, browser.as_deref(), false) else {
                return Err("Tidak ada jendela browser yang terbuka.".to_string());
            };
            let msg = perform_browser_scroll_blocking(bw.hwnd, &dir, amount)?;
            return append_snapshot_if_requested(msg, browser.as_deref(), snap_after, true);
        }

        // click or type
        let tgt = target
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if act == "click" && tgt.is_none() {
            return Err("target elemen click kosong".to_string());
        }

        if let Some(ref tid) = tgt {
            if tid.starts_with("@b") {
                if let Some(cref) = lookup_ref(tid) {
                    if cref.is_browser {
                        if cref.scroll_gen != current_scroll_gen() {
                            return Err(format!(
                                "Ref '{}' sudah usang karena halaman di-scroll. Panggil browser_snapshot(include_text=true) lagi untuk mendapat @bN terbaru.",
                                tid
                            ));
                        }
                        let _ = ensure_window_restored_and_rect(cref.hwnd);
                        unsafe {
                            direct_click_in_window(cref.hwnd, cref.cx, cref.cy);
                        }
                        if act == "type" {
                            let txt = text.as_deref().unwrap_or("");
                            let enter_after = key
                                .as_deref()
                                .map(|k| k.eq_ignore_ascii_case("enter"))
                                .unwrap_or(false);
                            unsafe {
                                perform_type_after_focus(txt, clear_first, enter_after);
                            }
                            return append_snapshot_if_requested(
                                format!("Teks diketik ke \"{}\".", cref.name),
                                browser.as_deref(),
                                snap_after,
                                false,
                            );
                        }
                        return append_snapshot_if_requested(
                            format!("Elemen \"{}\" diklik.", cref.name),
                            browser.as_deref(),
                            snap_after,
                            false,
                        );
                    }
                }
            }
        }

        let Some(bw) = find_browser_window(&windows, browser.as_deref(), false) else {
            return Err("Tidak ada jendela browser yang terbuka.".to_string());
        };
        let hwnd = bw.hwnd;
        let is_type = act == "type";
        let type_text = text.unwrap_or_default();
        let post_key = key;

        let msg = std::thread::spawn(move || unsafe {
            let mut wr = ensure_window_restored_and_rect(hwnd);
            win::focus_hwnd(hwnd);
            std::thread::sleep(Duration::from_millis(75));
            let _ = GetWindowRect(HWND(hwnd as *mut _), &mut wr);
            let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let res = (|| -> Result<String, String> {
                let uia = CoCreateInstance::<_, IUIAutomation>(
                    &CUIAutomation,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .map_err(|e| format!("UIA init gagal: {:?}", e))?;
                let root = uia
                    .ElementFromHandle(HWND(hwnd as *mut _))
                    .map_err(|_| "Gagal membaca elemen browser.".to_string())?;
                let (search_root, bounds) = match find_active_browser_doc_for_hwnd(&uia, &root, hwnd, wr, None) {
                    Some((d, r)) => (d, r),
                    None => (
                        root.clone(),
                        RECT {
                            left: wr.left + 44,
                            top: wr.top + 44,
                            right: wr.right,
                            bottom: wr.bottom,
                        },
                    ),
                };

                if is_type && tgt.is_none() {
                    let edits = collect_elements_in_root(
                        &uia,
                        &search_root,
                        bounds,
                        &[(UIA_EditControlTypeId.0, "Edit")],
                        None,
                        5,
                    );
                    if let Some((_, name, cx, cy, el)) = edits.into_iter().next() {
                        let (fx, fy) = scroll_element_into_view_and_center(&el, bounds).unwrap_or((cx, cy));
                        let _ = el.SetFocus();
                        direct_click_in_window(hwnd, fx, fy);
                        let enter_after = post_key
                            .as_deref()
                            .map(|k| k.eq_ignore_ascii_case("enter"))
                            .unwrap_or(false);
                        perform_type_after_focus(&type_text, clear_first, enter_after);
                        return Ok(format!("Teks diketik ke \"{}\".", name));
                    }
                    return Err("Kolom input tidak ditemukan di halaman browser.".to_string());
                }

                let t_low = tgt.unwrap_or_default().to_lowercase();
                let types = if is_type {
                    vec![(UIA_EditControlTypeId.0, "Edit"), (UIA_ComboBoxControlTypeId.0, "ComboBox")]
                } else {
                    vec![
                        (UIA_ButtonControlTypeId.0, "Button"),
                        (UIA_RadioButtonControlTypeId.0, "Radio"),
                        (UIA_CheckBoxControlTypeId.0, "CheckBox"),
                        (UIA_HyperlinkControlTypeId.0, "Link"),
                        (UIA_ListItemControlTypeId.0, "ListItem"),
                        (UIA_DataItemControlTypeId.0, "DataItem"),
                        (UIA_MenuItemControlTypeId.0, "MenuItem"),
                        (UIA_TabItemControlTypeId.0, "Tab"),
                        (UIA_EditControlTypeId.0, "Edit"),
                        (UIA_GroupControlTypeId.0, "Group"),
                        (UIA_TextControlTypeId.0, "Text"),
                    ]
                };
                let hit = find_best_target_in_root(&uia, &search_root, bounds, &types, &t_low)
                    .or_else(|| {
                        let content_fb = RECT {
                            left: wr.left + 44,
                            top: wr.top + 44,
                            right: wr.right,
                            bottom: wr.bottom,
                        };
                        find_best_target_in_root(&uia, &root, content_fb, &types, &t_low)
                    });
                let Some((_, _is_text, name, cx, cy, el)) = hit else {
                    return Err(format!("Elemen browser '{}' tidak ditemukan.", t_low));
                };
                let (fx, fy) = scroll_element_into_view_and_center(&el, bounds).unwrap_or((cx, cy));
                if !invoke_or_click_element(&el, hwnd, fx, fy, true) {
                    return Err("Gagal berinteraksi dengan elemen browser.".to_string());
                }
                if is_type {
                    let enter_after = post_key
                        .as_deref()
                        .map(|k| k.eq_ignore_ascii_case("enter"))
                        .unwrap_or(false);
                    perform_type_after_focus(&type_text, clear_first, enter_after);
                    return Ok(format!("Teks diketik ke \"{}\".", name));
                }
                Ok(format!("Elemen \"{}\" diklik.", name))
            })();
            if init_ok {
                CoUninitialize();
            }
            res
        })
        .join()
        .unwrap_or_else(|_| Err("Worker aksi browser terhenti.".to_string()))?;

        append_snapshot_if_requested(msg, browser.as_deref(), snap_after, false)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn find_desktop_window<'a>(
    windows: &'a [WindowEnumItem],
    app_hint: Option<&str>,
) -> Result<&'a WindowEnumItem, String> {
    let hint_low = app_hint
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());

    let eligible: Vec<&WindowEnumItem> = windows
        .iter()
        .filter(|w| {
            let tl = w.title.to_lowercase();
            let sl = w.stem.as_str();
            if sl == "selabar"
                || sl == "desknotch"
                || tl == "program manager"
                || tl.contains("windows input experience")
                || sl == "textinputhost"
            {
                return false;
            }
            if hint_low.is_none() && BROWSER_STEMS.contains(&sl) {
                return false;
            }
            true
        })
        .collect();

    if let Some(ref h) = hint_low {
        if let Some(w) = eligible
            .iter()
            .find(|w| w.stem == *h || w.stem.contains(h.as_str()) || w.title.to_lowercase().contains(h.as_str()))
        {
            return Ok(*w);
        }
        let open_list: Vec<String> = eligible
            .iter()
            .take(8)
            .map(|w| format!("{} ({})", w.title, w.stem))
            .collect();
        return Err(format!(
            "Jendela aplikasi '{}' tidak ditemukan. Jendela terbuka: {}",
            h,
            if open_list.is_empty() {
                "tidak ada".to_string()
            } else {
                open_list.join(", ")
            }
        ));
    }

    eligible
        .into_iter()
        .next()
        .ok_or_else(|| "Tidak ada aplikasi desktop aktif di luar browser.".to_string())
}

pub async fn desktop_snapshot(app: Option<String>, query: Option<String>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        if has_agent_desktop_cli() {
            if let Some(ref app_name) = app.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                let mut cmd = Command::new("agent-desktop");
                cmd.args(["snapshot", "--app", app_name, "-i"]);
                if let Ok(out) = silent(&mut cmd).output() {
                    if out.status.success() {
                        let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if !txt.is_empty() {
                            return Ok(txt);
                        }
                    }
                }
            }
        }

        let windows = unsafe { enum_visible_windows() };
        let w = find_desktop_window(&windows, app.as_deref())?;
        let hwnd = w.hwnd;
        let stem = w.stem.clone();
        let title = w.title.clone();
        let q_low = query
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());

        std::thread::spawn(move || unsafe {
            let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let res = (|| -> Result<String, String> {
                let uia = CoCreateInstance::<_, IUIAutomation>(
                    &CUIAutomation,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .map_err(|e| format!("UIA init gagal: {:?}", e))?;
                let wr = ensure_window_restored_and_rect(hwnd);
                let root = uia
                    .ElementFromHandle(HWND(hwnd as *mut _))
                    .map_err(|_| "Gagal membaca UI jendela desktop.".to_string())?;

                let types = [
                    (UIA_ButtonControlTypeId.0, "Button"),
                    (UIA_MenuItemControlTypeId.0, "MenuItem"),
                    (UIA_EditControlTypeId.0, "Edit"),
                    (UIA_TabItemControlTypeId.0, "Tab"),
                    (UIA_ListItemControlTypeId.0, "ListItem"),
                    (UIA_TreeItemControlTypeId.0, "TreeItem"),
                    (UIA_DataItemControlTypeId.0, "DataItem"),
                    (UIA_CheckBoxControlTypeId.0, "CheckBox"),
                    (UIA_ComboBoxControlTypeId.0, "ComboBox"),
                    (UIA_SplitButtonControlTypeId.0, "SplitButton"),
                ];
                let items =
                    collect_elements_in_root(&uia, &root, wr, &types, q_low.as_deref(), 55);
                if items.is_empty() {
                    return Ok(format!(
                        "App ({}): {}\nTidak ada elemen UI interaktif yang terdeteksi.",
                        stem, title
                    ));
                }

                let mut cache_entries = Vec::with_capacity(items.len());
                let mut lines = Vec::with_capacity(items.len() + 1);
                lines.push(format!("App ({}): {}", stem, title));
                for (i, (role, name, cx, cy, _)) in items.into_iter().enumerate() {
                    let ref_id = format!("@d{}", i + 1);
                    lines.push(format!("{} [{}] \"{}\" ({},{})", ref_id, role, name, cx, cy));
                    cache_entries.push((
                        ref_id,
                        CachedRef {
                            hwnd,
                            cx,
                            cy,
                            name,
                            is_browser: false,
                            scroll_gen: 0,
                        },
                    ));
                }
                store_refs("@d", cache_entries);
                Ok(lines.join("\n"))
            })();
            if init_ok {
                CoUninitialize();
            }
            res
        })
        .join()
        .unwrap_or_else(|_| Err("Worker UIA desktop terhenti.".to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn desktop_act(
    action: String,
    app: Option<String>,
    target: Option<String>,
    text: Option<String>,
    key: Option<String>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let act = action.trim().to_lowercase();
        let tgt_str = target.as_deref().map(str::trim).unwrap_or("").to_string();

        if has_agent_desktop_cli() && tgt_str.starts_with("@s") {
            let mut cmd = Command::new("agent-desktop");
            match act.as_str() {
                "click" => {
                    cmd.args(["click", &tgt_str]);
                }
                "type" => {
                    let txt = text.as_deref().unwrap_or("");
                    cmd.args(["type", txt]);
                }
                "key" => {
                    let k = key.as_deref().unwrap_or(&tgt_str);
                    cmd.args(["press", k]);
                }
                _ => {}
            }
            if let Ok(out) = silent(&mut cmd).output() {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    return Ok(if s.is_empty() { "Aksi desktop selesai.".to_string() } else { s });
                }
            }
        }

        if act == "focus" {
            let windows = unsafe { enum_visible_windows() };
            let w = find_desktop_window(&windows, app.as_deref().or(target.as_deref()))?;
            let title = w.title.clone();
            let _ = ensure_window_restored_and_rect(w.hwnd);
            win::focus_hwnd(w.hwnd);
            return Ok(format!("Jendela '{}' difokuskan.", title));
        }

        if act == "key" {
            let k = key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "parameter key kosong".to_string())?
                .to_string();
            let windows = unsafe { enum_visible_windows() };
            let w = find_desktop_window(&windows, app.as_deref())?;
            let _ = ensure_window_restored_and_rect(w.hwnd);
            win::focus_hwnd(w.hwnd);
            std::thread::sleep(Duration::from_millis(60));
            win::send_keys(&k)?;
            return Ok(format!("Tombol {} dikirim ke {}.", k, w.title));
        }

        if tgt_str.starts_with("@d") {
            if let Some(cref) = lookup_ref(&tgt_str) {
                if !cref.is_browser {
                    let _ = ensure_window_restored_and_rect(cref.hwnd);
                    unsafe {
                        direct_click_in_window(cref.hwnd, cref.cx, cref.cy);
                    }
                    if act == "type" {
                        let txt = text.as_deref().unwrap_or("");
                        if !txt.is_empty() {
                            std::thread::sleep(Duration::from_millis(70));
                            unsafe {
                                send_unicode_text(txt);
                            }
                        }
                        if key.as_deref().map(|k| k.eq_ignore_ascii_case("enter")).unwrap_or(false) {
                            std::thread::sleep(Duration::from_millis(50));
                            let _ = win::send_keys("enter");
                        }
                        return Ok(format!("Teks diketik ke \"{}\".", cref.name));
                    }
                    return Ok(format!("Elemen \"{}\" diklik.", cref.name));
                }
            }
        }

        let windows = unsafe { enum_visible_windows() };
        let w = find_desktop_window(&windows, app.as_deref())?;
        let hwnd = w.hwnd;
        let is_type = act == "type";
        let type_text = text.unwrap_or_default();
        let post_key = key;

        if is_type && tgt_str.is_empty() {
            let _ = ensure_window_restored_and_rect(hwnd);
            win::focus_hwnd(hwnd);
            std::thread::sleep(Duration::from_millis(70));
            if !type_text.is_empty() {
                unsafe {
                    send_unicode_text(&type_text);
                }
            }
            if post_key.as_deref().map(|k| k.eq_ignore_ascii_case("enter")).unwrap_or(false) {
                std::thread::sleep(Duration::from_millis(50));
                let _ = win::send_keys("enter");
            }
            return Ok("Teks diketik ke aplikasi desktop.".to_string());
        }

        if tgt_str.is_empty() {
            return Err("target elemen desktop kosong".to_string());
        }

        std::thread::spawn(move || unsafe {
            win::focus_hwnd(hwnd);
            std::thread::sleep(Duration::from_millis(80));
            let init_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let res = (|| -> Result<String, String> {
                let uia = CoCreateInstance::<_, IUIAutomation>(
                    &CUIAutomation,
                    None,
                    CLSCTX_INPROC_SERVER,
                )
                .map_err(|e| format!("UIA init gagal: {:?}", e))?;
                let wr = ensure_window_restored_and_rect(hwnd);
                let root = uia
                    .ElementFromHandle(HWND(hwnd as *mut _))
                    .map_err(|_| "Gagal membaca UI jendela desktop.".to_string())?;
                let t_low = tgt_str.to_lowercase();
                let types = if is_type {
                    vec![
                        (UIA_EditControlTypeId.0, "Edit"),
                        (UIA_ComboBoxControlTypeId.0, "ComboBox"),
                    ]
                } else {
                    vec![
                        (UIA_ButtonControlTypeId.0, "Button"),
                        (UIA_MenuItemControlTypeId.0, "MenuItem"),
                        (UIA_TabItemControlTypeId.0, "Tab"),
                        (UIA_ListItemControlTypeId.0, "ListItem"),
                        (UIA_TreeItemControlTypeId.0, "TreeItem"),
                        (UIA_DataItemControlTypeId.0, "DataItem"),
                        (UIA_CheckBoxControlTypeId.0, "CheckBox"),
                        (UIA_SplitButtonControlTypeId.0, "SplitButton"),
                        (UIA_EditControlTypeId.0, "Edit"),
                    ]
                };
                let Some((_, _, name, cx, cy, el)) =
                    find_best_target_in_root(&uia, &root, wr, &types, &t_low)
                else {
                    return Err(format!("Elemen desktop '{}' tidak ditemukan.", tgt_str));
                };
                if !invoke_or_click_element(&el, hwnd, cx, cy, is_type) {
                    return Err("Gagal berinteraksi dengan elemen desktop.".to_string());
                }
                if is_type {
                    std::thread::sleep(Duration::from_millis(70));
                    if !type_text.is_empty() {
                        send_unicode_text(&type_text);
                    }
                    if post_key.as_deref().map(|k| k.eq_ignore_ascii_case("enter")).unwrap_or(false) {
                        std::thread::sleep(Duration::from_millis(50));
                        let _ = win::send_keys("enter");
                    }
                    return Ok(format!("Teks diketik ke \"{}\".", name));
                }
                Ok(format!("Elemen \"{}\" diklik.", name))
            })();
            if init_ok {
                CoUninitialize();
            }
            res
        })
        .join()
        .unwrap_or_else(|_| Err("Worker aksi desktop terhenti.".to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::{compute_wheel_delta, format_interactive_tag, is_youtube_skip_ad_label};

    #[test]
    fn youtube_skip_ad_label_matcher() {
        assert!(is_youtube_skip_ad_label("Lewati iklan"));
        assert!(is_youtube_skip_ad_label("  Lewati  "));
        assert!(is_youtube_skip_ad_label("Skip Ad"));
        assert!(is_youtube_skip_ad_label("Skip Ads"));
        assert!(is_youtube_skip_ad_label("Skip"));

        assert!(!is_youtube_skip_ad_label(""));
        assert!(!is_youtube_skip_ad_label("Skip navigation"));
        assert!(!is_youtube_skip_ad_label("Lewati ke konten utama"));
        assert!(!is_youtube_skip_ad_label("Skip forward 10 seconds"));
        assert!(!is_youtube_skip_ad_label("Lewati iklan dalam 4 detik"));
    }

    #[test]
    fn wheel_delta_and_state_tag_formatting() {
        assert_eq!(compute_wheel_delta("down", None), -600);
        assert_eq!(compute_wheel_delta("up", Some(3)), 360);
        assert_eq!(compute_wheel_delta("atas", Some(20)), 1440);
        assert_eq!(compute_wheel_delta("down", Some(0)), -120);

        assert_eq!(format_interactive_tag("Jakarta", ""), "\"Jakarta\"");
        assert_eq!(
            format_interactive_tag("Jakarta", "[checked]"),
            "\"Jakarta\" [checked]"
        );
    }
}
