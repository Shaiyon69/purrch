//! Shelf helpers for Linux. Drag-and-drop itself lives in the GTK window (`linux/mod.rs`).

use gtk::gio;
use gtk::prelude::*;
use std::path::{Path, PathBuf};

/// Where dropped links and text snippets get written as real files.
pub fn own_dir() -> PathBuf {
    crate::store::xdg_dir("XDG_DATA_HOME", ".local/share").join("purrch").join("shelf")
}

/// Opens with the default app. Saved links (Windows-style .url files) open their URL directly,
/// since few Linux desktops understand that format.
pub fn open(path: &Path) {
    let link = (path.extension().is_some_and(|e| e == "url"))
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("URL=")).map(|u| u.trim().to_string()));
    let uri = link.unwrap_or_else(|| gio::File::for_path(path).uri().to_string());
    let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
}
