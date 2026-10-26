//! The Linux notch: a transparent GTK4 window pinned to the top of the screen.
//! Wayland uses the layer-shell protocol; X11 (and GNOME, which lacks layer-shell) uses an
//! override-redirect window that the window manager leaves alone.

mod tray;
pub mod x11;

use crate::app::{self, App};
use crate::ui::{Painter, Rect};
use crate::{bluetooth, cat, media, shelf, status};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const APP_ID: &str = "io.github.shaiyon69.Purrch";

struct State {
    app: App,
    window: gtk::Window,
    area: gtk::DrawingArea,
    /// The X11 window, when not on Wayland.
    xid: Option<u32>,
    /// Device pixels per DIP.
    scale: f32,
    /// Top-left of the canvas on the X11 root window, in device pixels.
    origin: (i32, i32),
    /// Where the left button went down, to tell a click from a drag.
    press: Option<(f32, f32)>,
    /// A shelf item left through a drag; the button-up that follows is not a click.
    dragged_out: bool,
    bluetooth: Arc<Mutex<Vec<bluetooth::Change>>>,
    /// The open/close spring is moving; a tick callback renders a frame per refresh.
    animating: bool,
    pill_key: String,
    /// The notch's vertical slide while hiding over a full-screen app, in DIPs.
    slide: f32,
    input: Option<Rect>,
    open_timer: Option<glib::SourceId>,
    collapse_timer: Option<glib::SourceId>,
    fullscreen: bool,
    peeked: bool,
    away_since: Option<Instant>,
    tray: Option<ksni::blocking::Handle<tray::Tray>>,
    /// When the cursor last moved over the notch, for idle time when the system won't say.
    last_seen: Instant,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the shared state. Skips re-entrant calls instead of panicking on a double borrow.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs `f` on the GTK thread from any thread.
fn on_main(f: impl FnOnce(&mut State) + Send + 'static) {
    glib::idle_add_once(move || {
        with_state(f);
    });
}

impl State {
    fn render(&mut self) {
        self.area.queue_draw();
    }

    /// Repaints unless the notch is collapsed and the pill would look the same.
    fn refresh(&mut self) {
        if !self.app.closed() || self.app.pill_key() != self.pill_key {
            self.render();
        }
    }

    fn paint(&mut self, cr: &gtk::cairo::Context) {
        self.pill_key = self.app.pill_key();
        let bounds = self.app.bounds();
        // Slides by the notch's own height so it starts moving at once, and fades on the way.
        let peek = self.app.peek();
        self.slide = -(bounds.h + 4.0) * (1.0 - peek);
        let alpha = peek * peek * (3.0 - 2.0 * peek);
        cr.translate(0.0, self.slide as f64);
        cr.push_group();
        let app = &mut self.app;
        app.draw(&Painter::new(cr));
        let _ = cr.pop_group_to_source();
        let _ = cr.paint_with_alpha(alpha as f64);
        // Only the drawn notch takes the mouse; the rest of the canvas clicks through.
        let input = (alpha > 0.0).then_some(Rect { y: bounds.y + self.slide, ..bounds });
        if input != self.input {
            self.input = input;
            if let Some(surface) = self.window.surface() {
                let r = input.map_or(gtk::cairo::RectangleInt::new(0, 0, 0, 0), |r| {
                    gtk::cairo::RectangleInt::new(r.x.floor() as i32, r.y.floor() as i32, r.w.ceil() as i32 + 1, (r.y + r.h).ceil().max(0.0) as i32)
                });
                surface.set_input_region(&gtk::cairo::Region::create_rectangle(&r));
            }
        }
    }

    fn start_anim(&mut self) {
        if self.app.needs_animation() && !self.animating {
            self.animating = true;
            self.area.add_tick_callback(|_, _| {
                let more = with_state(|s| s.step_anim()).unwrap_or(false);
                if more { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
            });
        }
    }

    /// One frame of the open/close spring; false once it has settled.
    fn step_anim(&mut self) -> bool {
        if !self.app.animate() {
            self.animating = false;
        }
        self.render();
        if !self.app.shown && self.app.peek() == 0.0 {
            self.window.set_visible(false);
        }
        self.animating
    }

    /// Mouse position in canvas DIPs; the notch may be part-way through its slide.
    fn mouse(&self, x: f64, y: f64) -> (f32, f32) {
        (x as f32, y as f32 - self.slide)
    }

    /// The cursor in canvas DIPs. X11 knows it anywhere on screen; Wayland only over the notch.
    fn pointer(&self) -> Option<(f32, f32)> {
        if self.xid.is_some() {
            let (x, y) = x11::get()?.pointer()?;
            return Some(((x - self.origin.0) as f32 / self.scale, (y - self.origin.1) as f32 / self.scale));
        }
        self.app.mouse
    }

    fn idle_ms(&self) -> u32 {
        idle_ms().unwrap_or_else(|| self.last_seen.elapsed().as_millis().min(u32::MAX as u128) as u32)
    }

    /// Over a full-screen app the notch waits above the screen; pushing the cursor into the
    /// top-middle edge brings it back over that app until the cursor leaves it alone.
    fn auto_hide(&mut self) {
        if !self.fullscreen {
            self.peeked = false;
        } else if let Some((x, y)) = self.pointer() {
            if y <= 0.0 && (x - app::CANVAS_W / 2.0).abs() < 120.0 {
                self.peeked = true;
                self.away_since = None;
            } else if self.peeked && !self.app.open() && self.app.closed() {
                let since = *self.away_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_millis(600) {
                    self.peeked = false;
                }
            } else {
                self.away_since = None;
            }
        }
        let shown = !self.fullscreen || self.peeked;
        if shown == self.app.shown {
            return;
        }
        self.app.shown = shown;
        self.away_since = None;
        if shown {
            self.window.set_visible(true);
            // The full-screen window was raised over ours; climb back above it.
            if let (Some(x), Some(xid)) = (x11::get(), self.xid) {
                x.raise(xid);
            }
        }
        self.start_anim();
    }

    /// Point the cat's eyes at the cursor.
    fn aim_eyes(&mut self) {
        let Some((x, y)) = self.pointer() else { return };
        let (cx, cy) = self.app.cat_pos();
        self.app.mascot.look = cat::gaze(x - cx, y - cy);
    }

    /// Typing a task needs the keyboard. Layer-shell gives it on click (on-demand mode);
    /// an override-redirect window has to ask the X server.
    fn take_keyboard(&self) {
        if let (Some(x), Some(xid)) = (x11::get(), self.xid) {
            x.focus(xid);
        }
    }

    fn tick(&mut self) {
        self.fullscreen = status::fullscreen_app();
        let idle = self.idle_ms();
        self.app.tick(idle);
        if let (Some(x), Some(xid)) = (x11::get(), self.xid)
            && self.app.shown
        {
            // Newly mapped windows can stack over an override-redirect window.
            x.raise(xid);
        }
        self.start_anim();
        self.refresh();
    }

    fn eyes(&mut self) {
        self.auto_hide();
        let caps = self.app.poll_caps();
        if caps {
            self.start_anim();
        }
        // Lyric lines in the closed pill change faster than the 1 s tick.
        if self.app.poll_pill_lyric() {
            self.start_anim();
        }
        if self.app.closed() {
            self.refresh();
        }
        let now = Instant::now();
        if self.app.cat && self.idle_ms() < 400 {
            self.app.mascot.poke(now);
        }
        if self.app.cat && self.app.mascot.tracking(now) {
            self.aim_eyes();
        }
        // Blinks, snores, wake-ups and eye moves all run on the frame loop.
        self.start_anim();
        if caps && !self.animating {
            self.render();
        }
    }

    fn cancel_collapse(&mut self) {
        if let Some(id) = self.collapse_timer.take() {
            id.remove();
        }
    }
}

/// System idle time, sampled off the UI thread since D-Bus calls can block.
static IDLE: AtomicU32 = AtomicU32::new(u32::MAX);

/// u32::MAX means no source reports idle time.
fn idle_ms() -> Option<u32> {
    let v = IDLE.load(Ordering::Relaxed);
    (v != u32::MAX).then_some(v)
}

fn watch_idle() {
    std::thread::spawn(|| {
        loop {
            IDLE.store(read_idle().unwrap_or(u32::MAX), Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

/// GNOME's idle monitor, then X11's screensaver extension, then KDE's screensaver service.
fn read_idle() -> Option<u32> {
    let mutter = || -> Option<u32> {
        let p = proxy(session()?, "org.gnome.Mutter.IdleMonitor", "/org/gnome/Mutter/IdleMonitor/Core", "org.gnome.Mutter.IdleMonitor").ok()?;
        p.call::<_, _, u64>("GetIdletime", &()).ok().map(|ms| ms.min(u32::MAX as u64 - 1) as u32)
    };
    let kde = || -> Option<u32> {
        let p = proxy(session()?, "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver").ok()?;
        // KDE answers in milliseconds.
        p.call::<_, _, u32>("GetSessionIdleTime", &()).ok()
    };
    // XWayland only sees X clients' input, so its idle time is meaningless next to Wayland apps.
    let x = || if std::env::var_os("WAYLAND_DISPLAY").is_none() { x11::get()?.idle_ms() } else { None };
    mutter().or_else(x).or_else(kde)
}

pub fn session() -> Option<&'static zbus::blocking::Connection> {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    BUS.get_or_init(|| zbus::blocking::Connection::session().ok()).as_ref()
}

pub fn system() -> Option<&'static zbus::blocking::Connection> {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    BUS.get_or_init(|| zbus::blocking::Connection::system().ok()).as_ref()
}

/// A D-Bus proxy that reads properties on demand instead of caching and watching them.
pub fn proxy<'a>(conn: &zbus::blocking::Connection, dest: &'a str, path: &'a str, iface: &'a str) -> zbus::Result<zbus::blocking::Proxy<'a>> {
    zbus::blocking::proxy::Builder::new(conn)
        .destination(dest)?
        .path(path)?
        .interface(iface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

fn on_drag(event: shelf::DragEvent) {
    with_state(|s| {
        // The drag swallowed the mouse moves that would mark the notch hovered; stay open so
        // the new card is seen falling in.
        if matches!(event, shelf::DragEvent::Drop(_)) {
            s.app.hover = true;
        }
        s.app.drag(event);
        s.start_anim();
        s.render();
    });
}

fn on_tray(cmd: tray::Command) {
    match cmd {
        tray::Command::Autostart => tray::set_autostart(!tray::autostart_enabled()),
        tray::Command::About => tray::about(),
        tray::Command::Quit => {
            if let Some(app) = gio::Application::default() {
                app.quit();
            }
        }
        tray::Command::Cat | tray::Command::Compact => {
            with_state(|s| {
                if cmd == tray::Command::Cat {
                    s.app.cat = !s.app.cat;
                } else {
                    s.app.compact = !s.app.compact;
                }
                s.app.save_settings();
                s.render();
            });
        }
    }
    // The menu shows checkmarks from the tray's own copy of the settings.
    let settings = with_state(|s| (s.app.cat, s.app.compact));
    if let Some((cat, compact)) = settings {
        with_state(|s| {
            if let Some(handle) = &s.tray {
                handle.update(|t| {
                    t.cat = cat;
                    t.compact = compact;
                });
            }
        });
    }
}

/// GNOME's compositor has no layer-shell, so run through XWayland as an override-redirect window.
fn prefer_x11_on_gnome() {
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("GNOME")));
    let both = std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some();
    if gnome && both && std::env::var_os("GDK_BACKEND").is_none() {
        // SAFETY: still single-threaded; GTK reads this when it opens the display.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
}

pub fn run() {
    prefer_x11_on_gnome();
    // A second launch activates the first instance over D-Bus and exits.
    let application = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::default());
    application.connect_activate(build);
    application.run_with_args::<&str>(&[]);
}

fn build(application: &gtk::Application) {
    if STATE.with(|s| s.borrow().is_some()) {
        return;
    }
    let display = gdk::Display::default().expect("no display");
    let on_x11 = display.downcast_ref::<gdk4_x11::X11Display>().is_some();
    if on_x11 {
        x11::init();
    }

    let css = gtk::CssProvider::new();
    css.load_from_string(".purrch, .purrch > * { background: none; box-shadow: none; }");
    gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let (w, h) = (app::CANVAS_W as i32, app::CANVAS_H as i32);
    let window = gtk::Window::builder()
        .application(application)
        .title("Purrch")
        .decorated(false)
        .resizable(false)
        .default_width(w)
        .default_height(h)
        .css_classes(["purrch"])
        .build();
    let area = gtk::DrawingArea::builder().content_width(w).content_height(h).build();
    window.set_child(Some(&area));

    let mut origin = (0, 0);
    let mut scale = 1.0;
    if on_x11 {
        if let Some(x11) = display.downcast_ref::<gdk4_x11::X11Display>() {
            let m = x11.primary_monitor();
            let g = m.geometry();
            scale = m.scale_factor() as f32;
            let s = m.scale_factor();
            origin = (g.x() * s + (g.width() * s - w * s) / 2, g.y() * s);
        }
        window.connect_realize(move |win| {
            let Some(xid) = win.surface().and_downcast::<gdk4_x11::X11Surface>().map(|s| s.xid() as u32) else { return };
            if let Some(x) = x11::get() {
                // Before the first map: the window manager then never decorates, moves or tiles it.
                x.override_redirect(xid, origin);
            }
            with_state(|s| s.xid = Some(xid));
        });
    } else if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("purrch"));
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Top, true);
        // Clicks stay with other apps unless the notch is clicked, which typing a task needs.
        window.set_keyboard_mode(KeyboardMode::OnDemand);
    } else {
        eprintln!("purrch: this Wayland compositor has no layer-shell; the notch will be a normal window");
    }

    area.set_draw_func(|_, cr, _, _| {
        with_state(|s| s.paint(cr));
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(|_, x, y| {
        with_state(|s| {
            s.cancel_collapse();
            s.last_seen = Instant::now();
            let (x, y) = s.mouse(x, y);
            s.app.mouse = Some((x, y));
            if s.app.scrubbing() {
                s.app.scrub_to(x);
                s.render();
                return;
            }
            let hot = s.app.hot_rect();
            if hot != s.app.hot {
                s.app.hot = hot;
                s.area.set_cursor_from_name(hot.map(|_| "pointer"));
                s.render();
            }
            if !s.app.hover && s.open_timer.is_none() {
                // Brief hover intent so reaching past the notch for a tab or title bar doesn't pop it open.
                s.open_timer = Some(glib::timeout_add_local_once(Duration::from_millis(80), || {
                    with_state(|s| {
                        s.open_timer = None;
                        s.app.hover = true;
                        s.app.mascot.poke(Instant::now());
                        s.start_anim();
                    });
                }));
            }
        });
    });
    motion.connect_leave(|_| {
        with_state(|s| {
            if let Some(id) = s.open_timer.take() {
                id.remove();
            }
            s.app.mouse = None;
            s.app.hot = None;
            s.area.set_cursor_from_name(None);
            s.cancel_collapse();
            // Short grace period so brushing past the edge doesn't snap it shut.
            s.collapse_timer = Some(glib::timeout_add_local_once(Duration::from_millis(150), || {
                with_state(|s| {
                    s.collapse_timer = None;
                    if s.app.scrubbing() {
                        return;
                    }
                    s.app.hover = false;
                    s.start_anim();
                });
            }));
        });
    });
    area.add_controller(motion);

    // Presses, slider drags and clicks. A drag keeps reporting even outside the notch.
    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin(|_, x, y| {
        with_state(|s| {
            let (x, y) = s.mouse(x, y);
            s.dragged_out = false;
            if s.app.press(x, y) {
                s.render();
            } else {
                s.press = Some((x, y));
            }
        });
    });
    drag.connect_drag_update(|g, dx, _| {
        let Some((x0, _)) = g.start_point() else { return };
        with_state(|s| {
            if s.app.scrubbing() {
                s.app.scrub_to(x0 as f32 + dx as f32);
                s.render();
            }
        });
    });
    drag.connect_drag_end(|g, dx, dy| {
        let Some((x0, y0)) = g.start_point() else { return };
        with_state(|s| {
            let (x, y) = s.mouse(x0 + dx, y0 + dy);
            s.press = None;
            if s.app.release() {
                s.render();
                return;
            }
            if std::mem::take(&mut s.dragged_out) {
                return;
            }
            s.app.click(x, y);
            if s.app.editing.is_some() {
                s.take_keyboard();
            }
            s.start_anim();
            s.render();
        });
    });
    drag.connect_cancel(|_, _| {
        with_state(|s| {
            if s.app.release() {
                s.render();
            }
        });
    });
    area.add_controller(drag);

    // Dragging a shelf card out hands the file to any app that takes drops.
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY);
    source.connect_prepare(|_, _, _| {
        let path = with_state(|s| {
            let (px, py) = s.press?;
            let path = s.app.shelf_item_at(px, py);
            s.dragged_out = path.is_some();
            path
        })
        .flatten()?;
        let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
        Some(gdk::ContentProvider::for_value(&files.to_value()))
    });
    area.add_controller(source);

    let drop = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
    drop.set_types(&[gdk::FileList::static_type(), String::static_type()]);
    drop.connect_enter(|_, _, _| {
        on_drag(shelf::DragEvent::Enter);
        gdk::DragAction::COPY
    });
    drop.connect_leave(|_| on_drag(shelf::DragEvent::Leave));
    drop.connect_drop(|_, value, _, _| {
        let paths = if let Ok(files) = value.get::<gdk::FileList>() {
            files.files().into_iter().filter_map(|f| f.path()).collect()
        } else if let Ok(text) = value.get::<String>() {
            shelf::file_for_text(&text).into_iter().collect()
        } else {
            Vec::new()
        };
        on_drag(shelf::DragEvent::Drop(paths));
        true
    });
    area.add_controller(drop);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE);
    scroll.connect_scroll(|_, _, dy| {
        if dy != 0.0 {
            with_state(|s| {
                // Wheel up is negative, like Windows' positive delta mapping to -1.
                s.app.scroll(if dy < 0.0 { -1 } else { 1 });
                s.render();
            });
        }
        glib::Propagation::Stop
    });
    area.add_controller(scroll);

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, key, _, mods| {
        let handled = with_state(|s| {
            s.app.editing.as_ref()?;
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter => s.app.finish_edit(true),
                gdk::Key::Escape => s.app.finish_edit(false),
                gdk::Key::BackSpace => s.app.backspace(),
                _ => match key.to_unicode() {
                    Some(c) if !mods.contains(gdk::ModifierType::CONTROL_MASK) => s.app.type_char(c),
                    _ => return None,
                },
            }
            s.start_anim();
            s.render();
            Some(())
        })
        .flatten();
        if handled.is_some() { glib::Propagation::Stop } else { glib::Propagation::Proceed }
    });
    window.add_controller(keys);

    window.connect_is_active_notify(|w| {
        if !w.is_active() {
            with_state(|s| {
                if s.app.editing.is_some() {
                    s.app.finish_edit(true);
                    s.start_anim();
                    s.render();
                }
            });
        }
    });

    let mut app = App::load();
    app.media = Some(media::Media::start(|| {
        on_main(|s| {
            s.app.on_media();
            s.start_anim();
            s.refresh();
        })
    }));
    let bluetooth = bluetooth::start(|| {
        on_main(|s| {
            let changes = s.bluetooth.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            s.app.on_bluetooth(changes);
            s.start_anim();
            s.render();
        })
    });
    let tray = tray::spawn(app.cat, app.compact, on_tray);
    let state = State {
        app,
        window: window.clone(),
        area,
        xid: None,
        scale,
        origin,
        press: None,
        dragged_out: false,
        bluetooth,
        animating: false,
        pill_key: String::new(),
        slide: 0.0,
        input: None,
        open_timer: None,
        collapse_timer: None,
        fullscreen: false,
        peeked: false,
        away_since: None,
        tray,
        last_seen: Instant::now(),
    };
    STATE.with(|s| *s.borrow_mut() = Some(state));
    watch_idle();
    window.present();

    glib::timeout_add_local(Duration::from_secs(1), || {
        with_state(State::tick);
        glib::ControlFlow::Continue
    });
    glib::timeout_add_local(Duration::from_millis(120), || {
        with_state(State::eyes);
        glib::ControlFlow::Continue
    });
}
