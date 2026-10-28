//! System status changes that deserve a short pop-up: power, Caps Lock, internet, mic and camera use.

#[cfg(windows)]
#[path = "win/status.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux/status.rs"]
mod imp;

pub use imp::{capture, caps_lock, fullscreen_app, network, power};
#[cfg(windows)]
pub use imp::process_name;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Power {
    pub plugged: bool,
    /// 0..=100, or None when there is no battery.
    pub percent: Option<u8>,
}

/// What to tell the user when power goes from `old` to `new`, if anything.
pub fn power_message(old: Power, new: Power) -> Option<(String, String)> {
    let pct = new.percent?;
    let was = old.percent.unwrap_or(pct);
    if new.plugged != old.plugged {
        return Some(if new.plugged {
            ("Charging".into(), format!("{pct}%"))
        } else {
            ("On battery".into(), format!("{pct}% left"))
        });
    }
    if new.plugged {
        return (pct == 100 && was < 100).then(|| ("Fully charged".into(), "Unplug whenever you like".into()));
    }
    if pct <= 10 && was > 10 {
        Some(("Battery very low".into(), format!("{pct}% left, plug in now")))
    } else if pct <= 20 && was > 20 {
        Some(("Battery low".into(), format!("{pct}% left")))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Net {
    Online,
    Limited,
    Offline,
}

pub fn network_message(net: Net) -> (String, String) {
    match net {
        Net::Online => ("Back online".into(), "Internet connected".into()),
        Net::Limited => ("No internet".into(), "Connected, but no internet access".into()),
        Net::Offline => ("Offline".into(), "Network disconnected".into()),
    }
}

/// Which app is using the microphone and camera; `Some("")` when one is in use but unnamed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capture {
    pub mic: Option<String>,
    pub cam: Option<String>,
}

/// Toast only when a device starts being used, not while it stays in use or when it stops.
pub fn capture_message(old: &Capture, new: &Capture) -> Option<(String, String)> {
    let started = |was: &Option<String>, now: &Option<String>| now.clone().filter(|_| was.is_none());
    started(&old.cam, &new.cam)
        .map(|app| ("Camera in use".into(), app))
        .or_else(|| started(&old.mic, &new.mic).map(|app| ("Microphone in use".into(), app)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_toasts_only_when_use_starts() {
        let idle = Capture::default();
        let mic = Capture { mic: Some("Discord".into()), cam: None };
        assert_eq!(capture_message(&idle, &mic), Some(("Microphone in use".into(), "Discord".into())));
        assert_eq!(capture_message(&mic, &mic), None);
        assert_eq!(capture_message(&mic, &idle), None);
        let both = Capture { mic: Some("Discord".into()), cam: Some("Teams".into()) };
        assert_eq!(capture_message(&mic, &both).unwrap().0, "Camera in use");
    }

    const fn p(plugged: bool, pct: u8) -> Power {
        Power { plugged, percent: Some(pct) }
    }

    #[test]
    fn battery_thresholds_fire_once_on_the_way_down() {
        assert!(power_message(p(false, 21), p(false, 20)).unwrap().0.contains("low"));
        assert_eq!(power_message(p(false, 20), p(false, 19)), None);
        assert!(power_message(p(false, 11), p(false, 10)).unwrap().0.contains("very low"));
        assert_eq!(power_message(p(false, 50), p(false, 49)), None);
    }

    #[test]
    fn plug_events_and_full_charge() {
        assert_eq!(power_message(p(false, 40), p(true, 40)).unwrap().0, "Charging");
        assert_eq!(power_message(p(true, 40), p(false, 40)).unwrap().0, "On battery");
        assert_eq!(power_message(p(true, 99), p(true, 100)).unwrap().0, "Fully charged");
        assert_eq!(power_message(p(true, 100), p(true, 100)), None);
    }

    #[test]
    fn desktops_without_battery_stay_quiet() {
        let none = Power { plugged: true, percent: None };
        assert_eq!(power_message(none, Power { plugged: false, percent: None }), None);
    }
}
