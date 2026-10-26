//! Small OS services the app logic needs: local time, the reminder chime and the animation setting.

pub struct LocalTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

#[cfg(windows)]
pub fn local_time() -> LocalTime {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    LocalTime {
        year: t.wYear as u32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        weekday: t.wDayOfWeek as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
    }
}

#[cfg(target_os = "linux")]
pub fn local_time() -> LocalTime {
    // SAFETY: localtime_r only writes the tm we hand it.
    let t = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    LocalTime {
        year: (t.tm_year + 1900) as u32,
        month: (t.tm_mon + 1) as u32,
        day: t.tm_mday as u32,
        weekday: t.tm_wday as u32,
        hour: t.tm_hour as u32,
        minute: t.tm_min as u32,
        second: t.tm_sec.min(59) as u32,
    }
}

#[cfg(windows)]
pub fn chime() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
    unsafe {
        let _ = PlaySoundW(windows::core::w!("SystemAsterisk"), None, SND_ALIAS | SND_ASYNC);
    }
}

/// The desktop's notification sound through libcanberra, else the freedesktop sound file.
#[cfg(target_os = "linux")]
pub fn chime() {
    use std::process::{Command, Stdio};
    let quiet = |c: &mut Command| {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    };
    let mut canberra = Command::new("canberra-gtk-play");
    canberra.args(["-i", "message-new-instant"]);
    quiet(&mut canberra);
    if canberra.spawn().is_err() {
        let mut pa = Command::new("paplay");
        pa.arg("/usr/share/sounds/freedesktop/stereo/message-new-instant.oga");
        quiet(&mut pa);
        let _ = pa.spawn();
    }
}

/// Windows "Show animations in Windows" (Settings > Accessibility > Visual effects).
#[cfg(windows)]
pub fn animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut on = windows::Win32::Foundation::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

/// GTK's "enable animations" setting, which GNOME and KDE tie to their reduce-motion switches.
#[cfg(target_os = "linux")]
pub fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}
