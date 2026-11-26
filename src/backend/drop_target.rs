use crate::backend::click_through::ClickThroughManager;
use crate::events::{HwndHandle, UiEvent, UiSender};
use windows::core::{implement, Ref, Result};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

#[implement(IDropTarget)]
struct NotchDropTarget {
    hwnd: HwndHandle,
    ui: UiSender,
    click_through: ClickThroughManager,
}

impl NotchDropTarget {
    fn has_hdrop(pdataobj: &Option<&IDataObject>) -> bool {
        let Some(obj) = pdataobj else { return false };
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe { obj.QueryGetData(&fmt).is_ok() }
    }

    fn extract_paths(pdataobj: &Option<&IDataObject>) -> Vec<String> {
        let Some(obj) = pdataobj else { return Vec::new() };
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe {
            let Ok(mut stg) = obj.GetData(&fmt) else {
                return Vec::new();
            };
            let hdrop = HDROP(stg.u.hGlobal.0);
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
            let mut paths = Vec::with_capacity(count as usize);
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None);
                if len > 0 {
                    let mut buf = vec![0u16; len as usize + 1];
                    let copied = DragQueryFileW(hdrop, i, Some(&mut buf));
                    if copied > 0 {
                        paths.push(String::from_utf16_lossy(&buf[..copied as usize]));
                    }
                }
            }
            ReleaseStgMedium(&mut stg);
            paths
        }
    }
}

impl IDropTarget_Impl for NotchDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let obj = pdataobj.as_ref();
        let ok = NotchDropTarget::has_hdrop(&obj);
        if !pdweffect.is_null() {
            unsafe {
                *pdweffect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            }
        }
        if ok {
            self.click_through.set_drag_active(self.hwnd, true);
            self.ui.send(UiEvent::FilesDragEnter);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        if !pdweffect.is_null() {
            unsafe {
                *pdweffect = DROPEFFECT_COPY;
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.click_through.set_drag_active(self.hwnd, false);
        self.ui.send(UiEvent::FilesDragLeave);
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let obj = pdataobj.as_ref();
        let paths = NotchDropTarget::extract_paths(&obj);
        if !pdweffect.is_null() {
            unsafe {
                *pdweffect = if paths.is_empty() {
                    DROPEFFECT_NONE
                } else {
                    DROPEFFECT_COPY
                };
            }
        }
        self.click_through.set_drag_active(self.hwnd, false);
        if !paths.is_empty() {
            self.ui.send(UiEvent::ShelfDropped(paths));
        } else {
            self.ui.send(UiEvent::FilesDragLeave);
        }
        Ok(())
    }
}

pub fn register_drop_target(
    hwnd: HwndHandle,
    ui: UiSender,
    click_through: ClickThroughManager,
) -> Option<IDropTarget> {
    if !hwnd.is_valid() {
        return None;
    }
    let target: IDropTarget = NotchDropTarget {
        hwnd,
        ui,
        click_through,
    }
    .into();
    unsafe {
        if RegisterDragDrop(HWND(hwnd.0 as *mut _), &target).is_ok() {
            Some(target)
        } else {
            None
        }
    }
}
