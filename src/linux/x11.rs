//! A second X11 connection for what GTK4 no longer exposes: override-redirect, stacking,
//! focus, the global cursor, the active window's full-screen state and idle time.

use std::sync::OnceLock;
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, InputFocus, StackMode, Window};
use x11rb::rust_connection::RustConnection;

pub struct X11 {
    conn: RustConnection,
    root: Window,
    active: u32,
    state: u32,
    fullscreen: u32,
}

static X: OnceLock<Option<X11>> = OnceLock::new();

pub fn init() {
    X.get_or_init(|| {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
        Some(X11 {
            active: atom("_NET_ACTIVE_WINDOW")?,
            state: atom("_NET_WM_STATE")?,
            fullscreen: atom("_NET_WM_STATE_FULLSCREEN")?,
            conn,
            root,
        })
    });
}

pub fn get() -> Option<&'static X11> {
    X.get()?.as_ref()
}

impl X11 {
    pub fn override_redirect(&self, w: Window, (x, y): (i32, i32)) {
        let _ = self.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().override_redirect(1));
        let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().x(x).y(y));
        let _ = self.conn.flush();
    }

    pub fn raise(&self, w: Window) {
        let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = self.conn.flush();
    }

    pub fn focus(&self, w: Window) {
        let _ = self.conn.set_input_focus(InputFocus::PARENT, w, x11rb::CURRENT_TIME);
        let _ = self.conn.flush();
    }

    /// Cursor position on the root window, in device pixels.
    pub fn pointer(&self) -> Option<(i32, i32)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as i32, r.root_y as i32))
    }

    pub fn idle_ms(&self) -> Option<u32> {
        Some(self.conn.screensaver_query_info(self.root).ok()?.reply().ok()?.ms_since_user_input)
    }

    /// Whether the focused window asked the window manager for full screen.
    pub fn fullscreen(&self) -> bool {
        let prop = |w: Window, name: u32, kind: AtomEnum, len: u32| self.conn.get_property(false, w, name, kind, 0, len).ok()?.reply().ok();
        let Some(active) = prop(self.root, self.active, AtomEnum::WINDOW, 1).and_then(|r| r.value32()?.next()) else { return false };
        if active == 0 {
            return false;
        }
        prop(active, self.state, AtomEnum::ATOM, 64).and_then(|r| r.value32().map(|mut v| v.any(|a| a == self.fullscreen))).unwrap_or(false)
    }
}
