//! MPRIS players over D-Bus for the now-playing worker, and the system volume through `pactl`
//! (PulseAudio, or PipeWire's pulse server).

use super::{Cmd, Track};
use crate::linux::{proxy, session};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PATH: &str = "/org/mpris/MediaPlayer2";

/// The bus name of the player being shown.
pub type Session = String;

pub struct Manager(&'static zbus::blocking::Connection);

impl Manager {
    pub fn new() -> Option<Manager> {
        session().map(Manager)
    }

    /// A playing player wins over a paused one, which wins over a stopped one.
    pub fn current(&self) -> Option<Session> {
        let bus = proxy(self.0, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").ok()?;
        let names: Vec<String> = bus.call("ListNames", &()).ok()?;
        names
            .into_iter()
            .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
            .filter_map(|n| {
                let status: String = proxy(self.0, &n, PATH, PLAYER).ok()?.get_property("PlaybackStatus").ok()?;
                let rank = match status.as_str() {
                    "Playing" => 0,
                    "Paused" => 1,
                    _ => 2,
                };
                Some((rank, n))
            })
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, n)| n)
    }
}

fn player(name: &str) -> Option<zbus::blocking::Proxy<'_>> {
    proxy(session()?, name, PATH, PLAYER).ok()
}

fn metadata(p: &zbus::blocking::Proxy) -> Option<HashMap<String, OwnedValue>> {
    p.get_property("Metadata").ok()
}

fn text(m: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let v: &Value = m.get(key)?;
    if let Ok(s) = v.downcast_ref::<&str>() {
        return Some(s.to_string());
    }
    // xesam:artist is a list of names.
    let Value::Array(a) = v else { return None };
    let names: Vec<&str> = a.inner().iter().filter_map(|v| v.downcast_ref::<&str>().ok()).collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// Microseconds, which players send as any integer type.
fn micros(v: Option<&OwnedValue>) -> Duration {
    let Some(v) = v else { return Duration::ZERO };
    let us = v.downcast_ref::<i64>().ok().or_else(|| v.downcast_ref::<u64>().ok().map(|u| u as i64)).or_else(|| v.downcast_ref::<i32>().ok().map(i64::from));
    Duration::from_micros(us.unwrap_or(0).max(0) as u64)
}

pub fn read_track(name: &Session) -> Option<Track> {
    let p = player(name)?;
    let m = metadata(&p)?;
    let status: String = p.get_property("PlaybackStatus").ok()?;
    let position: OwnedValue = p.get_property("Position").unwrap_or_else(|_| OwnedValue::from(0i64));
    Some(Track {
        title: text(&m, "xesam:title").unwrap_or_default(),
        artist: text(&m, "xesam:artist").unwrap_or_default(),
        playing: status == "Playing",
        length: micros(m.get("mpris:length")),
        position: micros(Some(&position)),
        sampled: Instant::now(),
    })
}

/// Art comes as a file:// path (local players) or an http(s) URL (Spotify, browsers).
pub fn read_art(name: &Session) -> Option<Vec<u8>> {
    let url = text(&metadata(&player(name)?)?, "mpris:artUrl")?;
    if url.starts_with("file://") {
        let (path, _) = gtk::glib::filename_from_uri(&url).ok()?;
        return std::fs::read(path).ok();
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        let out = Command::new("curl").args(["-fsSL", "--max-time", "10", &url]).output().ok()?;
        return out.status.success().then_some(out.stdout);
    }
    None
}

pub fn send(name: &Session, cmd: Cmd) {
    let Some(p) = player(name) else { return };
    let _ = match cmd {
        Cmd::PlayPause => p.call_method("PlayPause", &()),
        Cmd::Next => p.call_method("Next", &()),
        Cmd::Prev => p.call_method("Previous", &()),
        Cmd::Seek(at) => {
            let us = at.as_micros() as i64;
            let id = metadata(&p).and_then(|m| m.get("mpris:trackid").and_then(|v| v.downcast_ref::<ObjectPath>().ok().map(|o| o.into_owned())));
            match id {
                Some(id) => p.call_method("SetPosition", &(id, us)),
                // Without a track id only a relative seek is possible.
                None => {
                    let now: i64 = p.get_property("Position").unwrap_or(0);
                    p.call_method("Seek", &(us - now))
                }
            }
        }
    };
}

fn pactl(args: &[&str]) -> Option<String> {
    let out = Command::new("pactl").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Shared with a worker thread that runs `pactl`, so drawing never waits on a process.
#[derive(Default)]
struct Mixer {
    level: f32,
    muted: bool,
    /// The first read finished; false after it means there is no pactl or no sound server.
    ready: bool,
    available: bool,
    set: Option<f32>,
    toggle: bool,
}

fn mixer() -> &'static (Mutex<Mixer>, Condvar) {
    static MIXER: OnceLock<(Mutex<Mixer>, Condvar)> = OnceLock::new();
    MIXER.get_or_init(|| {
        std::thread::spawn(|| {
            let (lock, wake) = mixer();
            loop {
                let (set, toggle) = lock.lock().map(|mut m| (m.set.take(), std::mem::take(&mut m.toggle))).unwrap_or_default();
                if let Some(v) = set {
                    pactl(&["set-sink-volume", "@DEFAULT_SINK@", &format!("{}%", (v * 100.0).round())]);
                }
                if toggle {
                    pactl(&["set-sink-mute", "@DEFAULT_SINK@", "toggle"]);
                }
                let level = pactl(&["get-sink-volume", "@DEFAULT_SINK@"]).and_then(|s| parse_percent(&s));
                let muted = pactl(&["get-sink-mute", "@DEFAULT_SINK@"]).is_some_and(|s| s.contains("yes"));
                let Ok(mut m) = lock.lock() else { return };
                m.ready = true;
                m.available = level.is_some();
                // A newer set is still pending; don't show the old level for a frame.
                if m.set.is_none() {
                    m.level = level.unwrap_or(0.0);
                    m.muted = muted;
                }
                wake.notify_all();
                if m.set.is_none() && !m.toggle {
                    let _ = wake.wait_timeout(m, Duration::from_secs(1));
                }
            }
        });
        (Mutex::new(Mixer::default()), Condvar::new())
    })
}

/// First "NN%" in `pactl get-sink-volume` output, as 0..1.
fn parse_percent(s: &str) -> Option<f32> {
    let word = s.split_whitespace().find(|w| w.ends_with('%'))?;
    word.trim_end_matches('%').parse::<f32>().ok().map(|p| p / 100.0)
}

/// Peak level of the playback device, for the beat pulse.
// shortcut: no level reading on Linux, so the notch never pulses there; read a PipeWire monitor stream if asked.
pub struct Meter;

impl Meter {
    pub fn open() -> Option<Meter> {
        None
    }

    pub fn peak(&self) -> f32 {
        0.0
    }
}

/// Master volume of the default playback device, as a 0..1 scalar.
pub struct Volume;

impl Volume {
    /// Waits briefly for the first read so the controls show from launch.
    pub fn open() -> Option<Volume> {
        let (lock, wake) = mixer();
        let m = lock.lock().ok()?;
        let (m, _) = wake.wait_timeout_while(m, Duration::from_secs(1), |m| !m.ready).ok()?;
        m.available.then_some(Volume)
    }

    fn with<R>(f: impl FnOnce(&mut Mixer) -> R) -> R {
        f(&mut mixer().0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Has the worker apply a change (or re-read) now instead of at its next poll.
    fn wake() {
        mixer().1.notify_all();
    }

    pub fn level(&self) -> f32 {
        Volume::with(|m| m.level)
    }

    pub fn set(&self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        Volume::with(|m| {
            m.level = level;
            m.set = Some(level);
        });
        Volume::wake();
    }

    pub fn muted(&self) -> bool {
        Volume::with(|m| m.muted)
    }

    pub fn toggle_mute(&self) {
        Volume::with(|m| {
            m.muted = !m.muted;
            m.toggle = true;
        });
        Volume::wake();
    }
}

/// Makes the next output device the default, moves playing streams to it and returns its name.
pub fn next_output() -> Option<String> {
    let sinks: Vec<String> = pactl(&["list", "short", "sinks"])?.lines().filter_map(|l| l.split('\t').nth(1).map(String::from)).collect();
    if sinks.len() < 2 {
        return None;
    }
    let current = pactl(&["get-default-sink"]).unwrap_or_default();
    let at = sinks.iter().position(|s| *s == current.trim()).map_or(0, |i| (i + 1) % sinks.len());
    let next = &sinks[at];
    pactl(&["set-default-sink", next])?;
    // PulseAudio leaves running streams where they were; PipeWire already moved them.
    for input in pactl(&["list", "short", "sink-inputs"]).unwrap_or_default().lines() {
        if let Some(id) = input.split('\t').next() {
            pactl(&["move-sink-input", id, next]);
        }
    }
    Volume::wake();
    let described = pactl(&["--format=json", "list", "sinks"]).and_then(|j| {
        let list: serde_json::Value = serde_json::from_str(&j).ok()?;
        list.as_array()?.iter().find(|s| s["name"] == next.as_str())?["description"].as_str().map(String::from)
    });
    Some(described.unwrap_or_else(|| next.clone()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_pactl_volume() {
        let out = "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 26214 /  40% / -23.88 dB\n";
        assert_eq!(super::parse_percent(out), Some(0.4));
        assert_eq!(super::parse_percent("nothing"), None);
    }
}
