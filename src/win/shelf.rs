//! Windows shell glue for the shelf: OLE drag-and-drop in and out, and opening items.

use super::{DragEvent, file_for_text};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{HBITMAP, HPALETTE};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SW_SHOWNORMAL};
use windows::core::{HRESULT, HSTRING, Result, implement, w};

/// Where dropped links and text snippets get written as real files.
pub fn own_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Purrch").join("Shelf")
}

pub fn open(path: &Path) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL);
    }
}

#[implement(IDropTarget)]
pub struct DropTarget {
    on: fn(DragEvent),
}

impl DropTarget {
    pub fn register(hwnd: HWND, on: fn(DragEvent)) -> Result<()> {
        let target: IDropTarget = DropTarget { on }.into();
        unsafe { RegisterDragDrop(hwnd, &target) }
    }
}

fn read_drop(data: &IDataObject) -> Vec<PathBuf> {
    let format = |cf: CLIPBOARD_FORMAT| FORMATETC {
        cfFormat: cf.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe {
        if let Ok(mut medium) = data.GetData(&format(CF_HDROP)) {
            let drop = HDROP(medium.u.hGlobal.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths = (0..count)
                .filter_map(|i| {
                    let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let len = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
                })
                .collect();
            ReleaseStgMedium(&mut medium);
            return paths;
        }
        if let Ok(mut medium) = data.GetData(&format(CF_UNICODETEXT)) {
            let ptr = GlobalLock(medium.u.hGlobal) as *const u16;
            let mut text = String::new();
            if !ptr.is_null() {
                let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
                text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(medium.u.hGlobal);
            }
            ReleaseStgMedium(&mut medium);
            return file_for_text(&text).into_iter().collect();
        }
    }
    Vec::new()
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, _: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = DROPEFFECT_COPY };
        (self.on)(DragEvent::Enter);
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = DROPEFFECT_COPY };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        (self.on)(DragEvent::Leave);
        Ok(())
    }

    fn Drop(&self, data: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = DROPEFFECT_COPY };
        let paths = data.map(read_drop).unwrap_or_default();
        (self.on)(DragEvent::Drop(paths));
        Ok(())
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Starts a real shell drag of `path`, so it can land in Explorer, a browser, a chat app...
/// Runs a modal loop; callers must not hold app state borrowed.
pub fn drag_out(path: &Path) {
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path.as_os_str()), None) else {
            return;
        };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return };
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT_NONE;
        let _ = DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
    }
}

/// Apps whose clipboard images are screenshots; other copied images are left alone.
const SCREENSHOT_TOOLS: [&str; 6] = ["snipping", "screenclipping", "screensketch", "sharex", "greenshot", "lightshot"];

/// A screenshot just put on the clipboard (Win+Shift+S and friends), saved as a PNG in the shelf folder.
pub fn clipboard_screenshot(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        IsClipboardFormatAvailable(CF_BITMAP.0 as u32).ok()?;
        let owner = GetClipboardOwner().ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(owner, Some(&mut pid));
        let name = crate::status::process_name(pid).to_lowercase();
        if !SCREENSHOT_TOOLS.iter().any(|t| name.contains(t)) {
            return None;
        }
        let dir = own_dir();
        std::fs::create_dir_all(&dir).ok()?;
        let t = GetLocalTime();
        let stem = format!("Screenshot {:02}-{:02}-{:02}", t.wHour, t.wMinute, t.wSecond);
        let mut path = dir.join(format!("{stem}.png"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{stem} ({n}).png"));
            n += 1;
        }
        OpenClipboard(hwnd).ok()?;
        let saved = GetClipboardData(CF_BITMAP.0 as u32).ok().is_some_and(|h| save_png(HBITMAP(h.0), &path).is_ok());
        let _ = CloseClipboard();
        if !saved {
            // Don't leave a half-written file on the shelf's folder.
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(path)
    }
}

fn save_png(bitmap: HBITMAP, path: &Path) -> Result<()> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let source = wic.CreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), WICBitmapIgnoreAlpha)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut props) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        frame.Initialize(props.as_ref())?;
        frame.WriteSource(&source, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()
    }
}
