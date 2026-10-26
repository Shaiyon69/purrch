//! Now playing, from whatever app reports to the system media controls (Spotify, browsers, ...):
//! Windows media controls, or MPRIS on Linux. Those calls block, so a worker thread polls
//! and the UI reads a shared snapshot.

use crate::lyrics::{self, Lines};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
#[path = "win/media.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux/media.rs"]
mod imp;

pub use imp::{Volume, next_output};

/// Wakes the UI thread after the worker changed the snapshot.
type Notify = Arc<dyn Fn() + Send + Sync>;

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
    pub fn start(notify: impl Fn() + Send + Sync + 'static) -> Media {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (cmds, rx) = mpsc::channel();
        let worker_shared = shared.clone();
        std::thread::spawn(move || run(worker_shared, rx, Arc::new(notify)));
        Media { shared, cmds }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmds.send(cmd);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn track_key(t: &Track) -> String {
    format!("{}\u{1}{}", t.title, t.artist)
}

fn run(shared: Arc<Mutex<Snapshot>>, cmds: Receiver<Cmd>, notify: Notify) {
    let Some(manager) = imp::Manager::new() else { return };
    let mut last_key = String::new();
    // Lyrics already found this run, so going back to a song shows them at once.
    let cache: Arc<Mutex<HashMap<String, Lines>>> = Arc::default();
    loop {
        let session = manager.current();
        let track = session.as_ref().and_then(imp::read_track);
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
        notify();

        if changed {
            last_key = key;
            if let (Some(s), Some(t)) = (&session, &track) {
                // Players send an empty title mid-switch; the real one arrives as another change.
                if cached.is_none() && !t.title.is_empty() {
                    // Started before the album art read so lyrics arrive as fast as the track does.
                    // The lookup is a network call; off this thread so play/pause/seek stay instant.
                    let (shared, cache, t, key, notify) = (shared.clone(), cache.clone(), t.clone(), last_key.clone(), notify.clone());
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
                        notify();
                    });
                }
                let art = imp::read_art(s).filter(|b| !b.is_empty()).map(Arc::new);
                if let Ok(mut snap) = shared.lock() {
                    snap.art = art;
                    snap.art_id += 1;
                }
                notify();
            }
        }

        match cmds.recv_timeout(Duration::from_secs(1)) {
            Ok(cmd) => {
                if let Some(s) = &session {
                    imp::send(s, cmd);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
