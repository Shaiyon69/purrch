//! Windows media controls (SMTC) for the now-playing worker, and the system volume through Core Audio.

use super::{Cmd, Track};
use std::time::{Duration, Instant};
use windows::Foundation::IAsyncOperation;
use windows::Media::Control::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::core::Interface;

pub type Session = GlobalSystemMediaTransportControlsSession;

pub struct Manager(GlobalSystemMediaTransportControlsSessionManager);

impl Manager {
    pub fn new() -> Option<Manager> {
        GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()).ok().map(Manager)
    }

    pub fn current(&self) -> Option<Session> {
        self.0.GetCurrentSession().ok()
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

pub fn read_track(session: &Session) -> Option<Track> {
    let read = || -> windows::core::Result<Track> {
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
    };
    read().ok()
}

pub fn read_art(session: &Session) -> Option<Vec<u8>> {
    let read = || -> windows::core::Result<Vec<u8>> {
        let stream = session.TryGetMediaPropertiesAsync()?.get()?.Thumbnail()?.OpenReadAsync()?.get()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream)?;
        reader.LoadAsync(size)?.cast::<IAsyncOperation<u32>>()?.get()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes)?;
        Ok(bytes)
    };
    read().ok()
}

pub fn send(s: &Session, cmd: Cmd) {
    let _ = match cmd {
        Cmd::PlayPause => s.TryTogglePlayPauseAsync().and_then(|op| op.get()),
        Cmd::Next => s.TrySkipNextAsync().and_then(|op| op.get()),
        Cmd::Prev => s.TrySkipPreviousAsync().and_then(|op| op.get()),
        // WinRT wants 100 ns ticks.
        Cmd::Seek(at) => s.TryChangePlaybackPositionAsync((at.as_nanos() / 100) as i64).and_then(|op| op.get()),
    };
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
