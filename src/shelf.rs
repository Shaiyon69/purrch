//! A small shelf of files: drop things on the notch, drag them back out later.

use crate::store;
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "win/shelf.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux/shelf.rs"]
mod imp;

pub use imp::open;
#[cfg(windows)]
pub use imp::{DropTarget, drag_out};

pub const FILE: &str = "shelf.txt";

#[derive(Default)]
pub struct Shelf {
    pub items: Vec<PathBuf>,
}

impl Shelf {
    /// Items deleted or moved since last run are dropped.
    pub fn load() -> Shelf {
        let text = std::fs::read_to_string(store::dir().join(FILE)).unwrap_or_default();
        Shelf { items: text.lines().map(PathBuf::from).filter(|p| p.exists()).collect() }
    }

    pub fn save(&self) {
        let text: String = self.items.iter().map(|p| format!("{}\n", p.display())).collect();
        let _ = store::save(FILE, &text);
    }

    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if !self.items.contains(&p) {
                self.items.push(p);
            }
        }
        self.save();
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.save();
    }
}

pub fn safe_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .take(48)
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() { "snippet".to_string() } else { cleaned }
}

pub fn looks_like_link(text: &str) -> bool {
    let t = text.trim();
    !t.contains(char::is_whitespace) && (t.contains("://") || t.starts_with("www."))
}

/// Turns dropped text into a file on disk: a clickable .url for links, a .txt otherwise.
pub fn file_for_text(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let dir = imp::own_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let (stem, ext, body) = if looks_like_link(text) {
        let url = if text.contains("://") { text.to_string() } else { format!("https://{text}") };
        let host = url.split("://").nth(1).unwrap_or(&url).split('/').next().unwrap_or("link");
        (safe_name(host), "url", format!("[InternetShortcut]\r\nURL={url}\r\n"))
    } else {
        let first_line = text.lines().next().unwrap_or(text);
        (safe_name(first_line), "txt", text.to_string())
    };
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    std::fs::write(&path, body).ok()?;
    Some(path)
}

pub fn size_label(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else { return String::new() };
    if meta.is_dir() {
        return "folder".to_string();
    }
    let b = meta.len() as f64;
    match b {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB", b / 1e6),
        b if b >= 1e3 => format!("{:.0} KB", b / 1e3),
        b => format!("{b} B"),
    }
}

/// A dropped card's fall-in at `t` (0..1 over 0.75 s): drop from above with a little
/// overshoot, growing from 70%. Returns (y offset, scale, opacity).
pub fn fall(t: f32) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    // easeOutBack, close to Nusa's cubic-bezier(.34, 1.6, .5, 1).
    let (c1, u) = (1.9, t - 1.0);
    let e = 1.0 + (c1 + 1.0) * u * u * u + c1 * u * u;
    (-34.0 * (1.0 - e), 0.7 + 0.3 * e, (t / 0.35).min(1.0))
}

pub enum DragEvent {
    Enter,
    Leave,
    Drop(Vec<PathBuf>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_for_windows() {
        assert_eq!(safe_name("a/b:c*?\"<>|d."), "abcd");
        assert_eq!(safe_name("  ...  "), "snippet");
        assert_eq!(safe_name(&"x".repeat(100)).len(), 48);
    }

    #[test]
    fn cards_fall_in_and_settle() {
        let (dy, scale, alpha) = fall(0.0);
        assert!((dy + 34.0).abs() < 1e-3 && (scale - 0.7).abs() < 1e-3 && alpha == 0.0);
        assert_eq!(fall(1.0), (0.0, 1.0, 1.0));
        assert_eq!(fall(5.0), (0.0, 1.0, 1.0));
        // Overshoots below its resting place before settling.
        assert!((5..10).any(|i| fall(i as f32 / 10.0).0 > 0.0));
    }

    #[test]
    fn detects_links() {
        assert!(looks_like_link("https://example.com/a"));
        assert!(looks_like_link("www.example.com"));
        assert!(!looks_like_link("see https://example.com for more"));
        assert!(!looks_like_link("plain words"));
    }
}
