//! Tray icon over StatusNotifierItem (KDE, most panels; GNOME needs the AppIndicator extension),
//! plus the autostart entry and the About box.

use gtk::{gdk_pixbuf, glib};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Autostart,
    Cat,
    Compact,
    About,
    Quit,
}

pub struct Tray {
    pub cat: bool,
    pub compact: bool,
    icon: Vec<ksni::Icon>,
    on: fn(Command),
}

impl Tray {
    /// Menu clicks arrive on the tray's D-Bus thread; hand them to the GTK thread.
    fn post(&self, cmd: Command) {
        let on = self.on;
        glib::idle_add_once(move || on(cmd));
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "purrch".into()
    }

    fn title(&self) -> String {
        "Purrch".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let check = |label: &str, checked: bool, cmd: Command| {
            CheckmarkItem { label: label.into(), checked, activate: Box::new(move |t: &mut Tray| t.post(cmd)), ..Default::default() }.into()
        };
        let item = |label: &str, cmd: Command| StandardItem { label: label.into(), activate: Box::new(move |t: &mut Tray| t.post(cmd)), ..Default::default() }.into();
        vec![
            check("Start on login", autostart_enabled(), Command::Autostart),
            check("Show the cat", self.cat, Command::Cat),
            check("Compact notch", self.compact, Command::Compact),
            ksni::MenuItem::Separator,
            item("About Purrch", Command::About),
            item("Quit", Command::Quit),
        ]
    }
}

/// The logo as ARGB32 in network byte order, which is what StatusNotifierItem wants.
fn icon(size: i32) -> Option<ksni::Icon> {
    let full = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(include_bytes!("../../assets/logo.png"))).ok()?;
    let p = full.scale_simple(size, size, gdk_pixbuf::InterpType::Bilinear)?.add_alpha(false, 0, 0, 0).ok()?;
    let (stride, bytes) = (p.rowstride() as usize, p.read_pixel_bytes());
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size as usize {
        for x in 0..size as usize {
            let px = &bytes[y * stride + x * 4..][..4];
            data.extend([px[3], px[0], px[1], px[2]]);
        }
    }
    Some(ksni::Icon { width: size, height: size, data })
}

/// None when no tray host is running; the notch works without it.
pub fn spawn(cat: bool, compact: bool, on: fn(Command)) -> Option<ksni::blocking::Handle<Tray>> {
    let icon = [32, 64].into_iter().filter_map(icon).collect();
    match (Tray { cat, compact, icon, on }).spawn() {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("purrch: no tray ({e}); quit with `pkill purrch`");
            None
        }
    }
}

fn autostart_file() -> PathBuf {
    crate::store::xdg_dir("XDG_CONFIG_HOME", ".config").join("autostart").join("purrch.desktop")
}

pub fn autostart_enabled() -> bool {
    autostart_file().exists()
}

pub fn set_autostart(on: bool) {
    let path = autostart_file();
    if !on {
        let _ = std::fs::remove_file(path);
        return;
    }
    // An AppImage runs from a temporary mount; start the AppImage file itself.
    let Some(exe) = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok()) else { return };
    let mut quoted = String::new();
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    let entry = format!("[Desktop Entry]\nType=Application\nName=Purrch\nComment=A cat that lives in your notch\nExec=\"{quoted}\"\nX-GNOME-Autostart-enabled=true\n");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, entry);
}

pub fn about() {
    gtk::AlertDialog::builder()
        .message("About Purrch")
        .detail(concat!("Purrch ", env!("CARGO_PKG_VERSION"), "\n\nA cat that lives in your notch.\nOpen source under the MIT license."))
        .build()
        .show(None::<&gtk::Window>);
}
