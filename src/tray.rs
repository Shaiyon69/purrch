use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

pub const WM_TRAY: u32 = WM_APP + 1;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Purrch");

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Autostart = 1,
    Cat,
    Compact,
    About,
    Quit,
}

/// The embedded logo (resource id 1, see build.rs) at the tray's small-icon size.
fn icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        GetModuleHandleW(None)
            .and_then(|m| LoadImageW(m, PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR))
            .map(|h| HICON(h.0))
            .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon(),
        ..Default::default()
    };
    for (dst, src) in nid.szTip.iter_mut().zip("Purrch".encode_utf16()) {
        *dst = src;
    }
    nid
}

/// Also called after Explorer restarts, since that drops every tray icon.
pub fn add(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd));
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
}

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(value.as_ptr().cast()),
                (value.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

/// Shows the menu and returns the picked command. Pumps messages while open,
/// so callers must not hold app state borrowed across this call.
pub fn menu(hwnd: HWND, cat: bool, compact: bool) -> Option<Command> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let items = [
            (Command::Autostart, "Start with Windows", check(autostart_enabled())),
            (Command::Cat, "Show the cat", check(cat)),
            (Command::Compact, "Compact notch", check(compact)),
        ];
        for (cmd, label, flags) in items {
            let _ = AppendMenuW(menu, MF_STRING | flags, cmd as usize, &HSTRING::from(label));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, Command::About as usize, w!("About Purrch"));
        let _ = AppendMenuW(menu, MF_STRING, Command::Quit as usize, w!("Quit"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu won't close when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
        [Command::Autostart, Command::Cat, Command::Compact, Command::About, Command::Quit]
            .into_iter()
            .find(|c| *c as i32 == picked.0)
    }
}

pub fn about(hwnd: HWND) {
    unsafe {
        MessageBoxW(
            hwnd,
            w!("Purrch 0.1.0\n\nA cat that lives in your notch.\nOpen source under the MIT license."),
            w!("About Purrch"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
