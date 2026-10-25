//! Now playing, from whatever app reports to Windows media controls (Spotify, browsers, ...).
//! WinRT calls block, so a worker thread polls and the UI reads a shared snapshot.

use crate::lyrics::{self, Lines};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Foundation::IAsyncOperation;
use windows::Media::Control::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

pub const WM_MEDIA: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

#[derive(Clone, Copy, PartialEq)]
pub enum Cmd {
    PlayPause,
    Next,
    Prev,
    Seek(Duration),
}

#[derive(Clone, Default, PartialEq)]
pub enum LyricState {
    #[default]
    None,
    Loading,
    Missing,
    Found(Lines),
}

#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub length: Duration,
    position: Duration,
    sampled: Instant,
}

impl Track {
    /// Position now, moving forward between the worker's samples while playing.
    pub fn position(&self) -> Duration {
        let pos = if self.playing { self.position + self.sampled.elapsed() } else { self.position };
        if self.length > Duration::ZERO { pos.min(self.length) } else { pos }
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub track: Option<Track>,
    pub lyrics: LyricState,
    /// Encoded album art (PNG/JPEG) and a counter that changes with it.
    pub art: Option<Arc<Vec<u8>>>,
    pub art_id: u64,
}

pub struct Media {
    pub shared: Arc<Mutex<Snapshot>>,
    cmds: Sender<Cmd>,
}

impl Media {
    pub fn start(hwnd: HWND) -> Media {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (cmds, rx) = mpsc::channel();
        let worker_shared = shared.clone();
        let hwnd = hwnd.0 as isize; // HWND is not Send; it is just an id here.
        std::thread::spawn(move || {
            let _ = run(worker_shared, rx, hwnd);
        });
        Media { shared, cmds }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmds.send(cmd);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn notify(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(HWND(hwnd as _), WM_MEDIA, WPARAM(0), LPARAM(0));
    }
}

/// WinRT DateTime and FILETIME share units (100 ns since 1601).
fn now_ticks() -> i64 {
    let ft = unsafe { GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn ticks(d: i64) -> Duration {
    Duration::from_nanos(d.max(0) as u64 * 100)
}

fn read_track(session: &GlobalSystemMediaTransportControlsSession) -> windows::core::Result<Track> {
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let timeline = session.GetTimelineProperties()?;
    let mut position = ticks(timeline.Position()?.Duration);
    if playing {
        // Players update the timeline only now and then; add the time since their last update.
        position += ticks(now_ticks() - timeline.LastUpdatedTime()?.UniversalTime);
    }
    Ok(Track {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        playing,
        length: ticks(timeline.EndTime()?.Duration),
        position,
        sampled: Instant::now(),
    })
}

fn read_art(session: &GlobalSystemMediaTransportControlsSession) -> windows::core::Result<Vec<u8>> {
    let stream = session.TryGetMediaPropertiesAsync()?.get()?.Thumbnail()?.OpenReadAsync()?.get()?;
    let size = stream.Size()? as u32;
    let reader = DataReader::CreateDataReader(&stream)?;
    reader.LoadAsync(size)?.cast::<IAsyncOperation<u32>>()?.get()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

fn track_key(t: &Track) -> String {
    format!("{}\u{1}{}", t.title, t.artist)
}

fn run(shared: Arc<Mutex<Snapshot>>, cmds: Receiver<Cmd>, hwnd: isize) -> windows::core::Result<()> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    let mut last_key = String::new();
    // Lyrics already found this run, so going back to a song shows them at once.
    let cache: Arc<Mutex<HashMap<String, Lines>>> = Arc::default();
    loop {
        let session = manager.GetCurrentSession().ok();
        let track = session.as_ref().and_then(|s| read_track(s).ok());
        let key = track.as_ref().map_or(String::new(), track_key);
        let changed = key != last_key;
        let cached = cache.lock().ok().and_then(|c| c.get(&key).cloned());
        if let Ok(mut snap) = shared.lock() {
            snap.track = track.clone();
            if changed {
                snap.lyrics = match (&track, cached.clone()) {
                    (None, _) => LyricState::None,
                    (Some(_), Some(lines)) => LyricState::Found(lines),
                    (Some(_), None) => LyricState::Loading,
                };
            }
        }
        notify(hwnd);

        if changed {
            last_key = key;
            if let (Some(s), Some(t)) = (&session, &track) {
                // Players send an empty title mid-switch; the real one arrives as another change.
                if cached.is_none() && !t.title.is_empty() {
                    // Started before the album art read so lyrics arrive as fast as the track does.
                    // The lookup is a network call; off this thread so play/pause/seek stay instant.
                    let (shared, cache, t, key) = (shared.clone(), cache.clone(), t.clone(), last_key.clone());
                    std::thread::spawn(move || {
                        let current = || shared.lock().ok().is_some_and(|s| s.track.as_ref().is_some_and(|now| track_key(now) == key));
                        // A failed connection retries a few times instead of reporting no lyrics.
                        let mut found = lyrics::fetch(&t.title, &t.artist, t.length);
                        for _ in 0..3 {
                            if found.is_ok() || !current() {
                                break;
                            }
                            std::thread::sleep(Duration::from_secs(2));
                            found = lyrics::fetch(&t.title, &t.artist, t.length);
                        }
                        let found = found.ok().flatten();
                        if let (Some(lines), Ok(mut c)) = (&found, cache.lock()) {
                            c.insert(key.clone(), lines.clone());
                        }
                        if let Ok(mut snap) = shared.lock() {
                            // The song may have changed while this was loading.
                            if snap.track.as_ref().is_some_and(|now| track_key(now) == key) {
                                snap.lyrics = found.map_or(LyricState::Missing, LyricState::Found);
                            }
                        }
                        notify(hwnd);
                    });
                }
                let art = read_art(s).ok().filter(|b| !b.is_empty()).map(Arc::new);
                if let Ok(mut snap) = shared.lock() {
                    snap.art = art;
                    snap.art_id += 1;
                }
                notify(hwnd);
            }
        }

        match cmds.recv_timeout(Duration::from_secs(1)) {
            Ok(cmd) => {
                if let Some(s) = &session {
                    let _ = match cmd {
                        Cmd::PlayPause => s.TryTogglePlayPauseAsync().and_then(|op| op.get()),
                        Cmd::Next => s.TrySkipNextAsync().and_then(|op| op.get()),
                        Cmd::Prev => s.TrySkipPreviousAsync().and_then(|op| op.get()),
                        // WinRT wants 100 ns ticks.
                        Cmd::Seek(at) => s.TryChangePlaybackPositionAsync((at.as_nanos() / 100) as i64).and_then(|op| op.get()),
                    };
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Master volume of the default playback device, as a 0..1 scalar.
pub struct Volume(windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume);

impl Volume {
    /// The current default output; call again to follow a device switch (headphones plugged in, ...).
    pub fn open() -> Option<Volume> {
        use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.Activate(CLSCTX_ALL, None).ok().map(Volume)
        }
    }

    pub fn level(&self) -> f32 {
        unsafe { self.0.GetMasterVolumeLevelScalar().unwrap_or(0.0) }
    }

    pub fn set(&self, level: f32) {
        unsafe {
            let _ = self.0.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null());
        }
    }

    pub fn muted(&self) -> bool {
        unsafe { self.0.GetMute().is_ok_and(|m| m.as_bool()) }
    }

    pub fn toggle_mute(&self) {
        unsafe {
            let _ = self.0.SetMute(!self.muted(), std::ptr::null());
        }
    }
}

/// Undocumented, but it is what the Windows sound settings use to change the default device.
/// Only `SetDefaultEndpoint` is called; the slots before it just keep the vtable in order.
mod policy {
    #![allow(non_snake_case)] // COM method names.
    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: windows_core::IUnknown {
        fn GetMixFormat(&self) -> windows_core::HRESULT;
        fn GetDeviceFormat(&self) -> windows_core::HRESULT;
        fn ResetDeviceFormat(&self) -> windows_core::HRESULT;
        fn SetDeviceFormat(&self) -> windows_core::HRESULT;
        fn GetProcessingPeriod(&self) -> windows_core::HRESULT;
        fn SetProcessingPeriod(&self) -> windows_core::HRESULT;
        fn GetShareMode(&self) -> windows_core::HRESULT;
        fn SetShareMode(&self) -> windows_core::HRESULT;
        fn GetPropertyValue(&self) -> windows_core::HRESULT;
        fn SetPropertyValue(&self) -> windows_core::HRESULT;
        fn SetDefaultEndpoint(&self, id: windows_core::PCWSTR, role: windows::Win32::Media::Audio::ERole) -> windows_core::HRESULT;
    }

    // The macro's methods are private to this module.
    pub fn set_default(policy: &IPolicyConfig, id: windows_core::PCWSTR, role: windows::Win32::Media::Audio::ERole) -> windows_core::Result<()> {
        unsafe { policy.SetDefaultEndpoint(id, role).ok() }
    }
}
use policy::IPolicyConfig;

const POLICY_CONFIG_CLIENT: windows_core::GUID = windows_core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Makes the next active output device the default and returns its name.
pub fn next_output() -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let id_of = |d: &IMMDevice| -> Option<String> {
            let p = d.GetId().ok()?;
            let id = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as _));
            id
        };
        let current = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| id_of(&d));
        let list = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).ok()?;
        let all: Vec<IMMDevice> = (0..list.GetCount().ok()?).filter_map(|i| list.Item(i).ok()).collect();
        if all.len() < 2 {
            return None;
        }
        let at = all.iter().position(|d| id_of(d) == current).map_or(0, |i| (i + 1) % all.len());
        let next = &all[at];
        let id = windows_core::HSTRING::from(id_of(next)?);
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL).ok()?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy::set_default(&policy, windows_core::PCWSTR(id.as_ptr()), role).ok()?;
        }
        let name = next.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(&PKEY_Device_FriendlyName)).map(|v| v.to_string());
        Some(name.unwrap_or_else(|_| "Output switched".into()))
    }
}
