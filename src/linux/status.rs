//! Linux status readers: battery and Caps Lock from sysfs, internet from NetworkManager,
//! and full-screen apps from the X11 window manager.

use super::{Net, Power};
use crate::linux::{proxy, system, x11};
use std::fs;
use std::path::Path;

fn read(path: &Path) -> String {
    fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

pub fn power() -> Option<Power> {
    let mut mains = None;
    let mut battery: Option<(u8, bool)> = None;
    for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let dir = entry.path();
        match read(&dir.join("type")).as_str() {
            "Mains" => mains = Some(mains.unwrap_or(false) || read(&dir.join("online")) == "1"),
            // Peripheral batteries (mice, headsets) report scope "Device".
            "Battery" if read(&dir.join("scope")) != "Device" && battery.is_none() => {
                let pct = read(&dir.join("capacity")).parse::<u8>().ok()?;
                battery = Some((pct.min(100), read(&dir.join("status")) != "Discharging"));
            }
            _ => {}
        }
    }
    Some(Power { plugged: mains.or(battery.map(|b| b.1)).unwrap_or(true), percent: battery.map(|b| b.0) })
}

/// Only X11 says which window is full screen. Wayland compositors already draw a full-screen
/// window over the notch's layer, so there is nothing to do there.
pub fn fullscreen_app() -> bool {
    x11::get().is_some_and(|x| x.fullscreen())
}

/// The keyboard's Caps Lock LED, which the kernel keeps in step on X11 and Wayland alike.
pub fn caps_lock() -> bool {
    let Ok(leds) = fs::read_dir("/sys/class/leds") else { return false };
    leds.flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with("::capslock"))
        .any(|e| read(&e.path().join("brightness")).parse::<u32>().is_ok_and(|b| b > 0))
}

pub fn network() -> Net {
    let nm = || -> Option<u32> {
        let p = proxy(system()?, "org.freedesktop.NetworkManager", "/org/freedesktop/NetworkManager", "org.freedesktop.NetworkManager").ok()?;
        p.get_property("State").ok()
    };
    match nm() {
        // NM_STATE_CONNECTED_GLOBAL
        Some(70) => Net::Online,
        // Connected to a local network or site only.
        Some(50 | 60) => Net::Limited,
        Some(_) => Net::Offline,
        // No NetworkManager: any interface besides loopback being up counts as online.
        None => {
            let up = fs::read_dir("/sys/class/net")
                .map(|d| d.flatten().any(|e| e.file_name() != "lo" && read(&e.path().join("operstate")) == "up"))
                .unwrap_or(false);
            if up { Net::Online } else { Net::Offline }
        }
    }
}
