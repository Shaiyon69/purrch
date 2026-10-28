//! Windows status readers: battery, Caps Lock, internet, mic and camera use, and whether a full-screen app is in front.

use super::{Capture, Net, Power};
use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
use std::sync::{Mutex, Once};
use windows::Win32::Foundation::{CloseHandle, POINT, RECT, S_OK};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsZoomed, WS_CAPTION,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    MMDeviceEnumerator, eCapture,
};
use windows::Win32::Media::MediaFoundation::{
    IMFSensorActivitiesReport, IMFSensorActivitiesReportCallback, IMFSensorActivitiesReportCallback_Impl,
    MF_VERSION, MFCreateSensorActivityMonitor, MFSTARTUP_LITE, MFStartup,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{Interface, PWSTR, Result, implement};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};

pub fn power() -> Option<Power> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    Some(Power { plugged: s.ACLineStatus == 1, percent: (s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent) })
}

/// A full-screen app (game, video, slideshow) owns the screen, so the notch should get out of the way.
pub fn fullscreen_app() -> bool {
    matches!(
        unsafe { SHQueryUserNotificationState() },
        Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    ) || covers_primary_monitor()
}

/// Browser F11 and many borderless games don't flag the shell as busy, so also check
/// whether the foreground window covers the whole primary monitor (where the notch sits).
/// Borderless apps that go "full screen" by maximizing cover only the work area, so a
/// maximized window counts once it has no title bar; ordinary maximized windows keep one.
fn covers_primary_monitor() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let zoomed = IsZoomed(hwnd).as_bool();
        if zoomed && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        // The desktop also covers the monitor.
        let mut class = [0u16; 16];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if class == "Progman" || class == "WorkerW" {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if monitor != MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) {
            return false;
        }
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut r = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        let m = if zoomed { info.rcWork } else { info.rcMonitor };
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

pub fn caps_lock() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
}

pub fn network() -> Net {
    let level = NetworkInformation::GetInternetConnectionProfile()
        .and_then(|p| p.GetNetworkConnectivityLevel())
        .unwrap_or(NetworkConnectivityLevel::None);
    match level {
        NetworkConnectivityLevel::InternetAccess => Net::Online,
        NetworkConnectivityLevel::None => Net::Offline,
        _ => Net::Limited,
    }
}

/// Live mic and camera use. The ConsentStore registry keys behind Privacy settings stopped
/// updating on recent Windows 11 builds, so ask the audio engine and the camera frame server.
pub fn capture() -> Capture {
    static START: Once = Once::new();
    START.call_once(watch_cameras);
    Capture { mic: mic_user(), cam: CAMERA.lock().ok().and_then(|c| c.clone()) }
}

/// The first app with an active recording session on any input device.
fn mic_user() -> Option<String> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let inputs = devices.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE).ok()?;
        for i in 0..inputs.GetCount().ok()? {
            let Ok(manager) = inputs.Item(i).and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)) else { continue };
            let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(s) = sessions.GetSession(j) else { continue };
                if s.GetState() != Ok(AudioSessionStateActive) {
                    continue;
                }
                let Ok(s2) = s.cast::<IAudioSessionControl2>() else { continue };
                if s2.IsSystemSoundsSession() != S_OK {
                    return Some(s2.GetProcessId().map_or(String::new(), process_name));
                }
            }
        }
        None
    }
}

/// Which app is streaming from a camera, kept up to date by the sensor monitor's callback.
static CAMERA: Mutex<Option<String>> = Mutex::new(None);

#[implement(IMFSensorActivitiesReportCallback)]
struct CameraWatch;

impl IMFSensorActivitiesReportCallback_Impl for CameraWatch_Impl {
    fn OnActivitiesReport(&self, report: Option<&IMFSensorActivitiesReport>) -> Result<()> {
        let mut user = None;
        if let Some(report) = report {
            unsafe {
                'all: for i in 0..report.GetCount()? {
                    let device = report.GetActivityReport(i)?;
                    for j in 0..device.GetProcessCount()? {
                        let p = device.GetProcessActivity(j)?;
                        if p.GetStreamingState()?.as_bool() {
                            user = Some(p.GetProcessId().map_or(String::new(), process_name));
                            break 'all;
                        }
                    }
                }
            }
        }
        if let Ok(mut c) = CAMERA.lock() {
            *c = user;
        }
        Ok(())
    }
}

/// Starts the camera frame server's activity monitor for the rest of the run. Without it
/// (older Windows) the camera simply never reads as in use.
fn watch_cameras() {
    unsafe {
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            return;
        }
        let callback: IMFSensorActivitiesReportCallback = CameraWatch.into();
        if let Ok(monitor) = MFCreateSensorActivityMonitor(&callback)
            && monitor.Start().is_ok()
        {
            // Lives as long as Purrch; stopping it would only matter at exit.
            std::mem::forget(monitor);
        }
    }
}

pub fn process_name(pid: u32) -> String {
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
    }
    exe_stem(&String::from_utf16_lossy(&buf[..len as usize]))
}

/// "C:\Program Files\Discord\Discord.exe" becomes "Discord".
fn exe_stem(path: &str) -> String {
    let file = path.rsplit('\\').next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string()
}

#[cfg(test)]
mod tests {
    use super::exe_stem;

    #[test]
    fn exe_stem_drops_folder_and_extension() {
        assert_eq!(exe_stem(r"C:\Program Files\Discord\Discord.exe"), "Discord");
        assert_eq!(exe_stem("chrome.EXE"), "chrome");
        assert_eq!(exe_stem(""), "");
    }
}
