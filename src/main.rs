#![windows_subsystem = "windows"]

mod app;
mod bluetooth;
mod cat;
mod lyrics;
mod media;
mod pomodoro;
mod shelf;
mod status;
mod store;
mod todo;
mod tray;
mod ui;
mod water;

use app::App;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ui::{Rect, Renderer};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

const TIMER_TICK: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_COLLAPSE: usize = 3;
const TIMER_EYES: usize = 4;
const TIMER_OPEN: usize = 5;
const WM_MOUSELEAVE: u32 = 0x02A3;

struct State {
    hwnd: HWND,
    app: App,
    renderer: Renderer,
    /// Device pixels per DIP.
    scale: f32,
    origin: POINT,
    taskbar_created: u32,
    /// Where the left button went down, to tell a click from a drag-out.
    press: Option<(f32, f32)>,
    bluetooth: Arc<Mutex<Vec<bluetooth::Change>>>,
    /// The open/close spring is moving; the message loop renders a frame per refresh.
    animating: bool,
    last_frame: Instant,
    /// Where the notch was last drawn, so the next frame also clears that area.
    prev_bounds: Option<Rect>,
    pill_key: String,
    /// Hover-intent timer is running.
    open_pending: bool,
    /// A full-screen or borderless app owns the screen, so the notch hides above the edge.
    fullscreen: bool,
    /// The cursor was pushed into the top edge to bring the notch back over that app.
    peeked: bool,
    /// When the peeked notch was left alone, to slide it away after a short wait.
    away_since: Option<Instant>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the shared state. Skips re-entrant calls (menus and message boxes pump
/// messages) instead of panicking on a double borrow.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

impl State {
    fn render(&mut self) {
        let bounds = self.app.bounds();
        let dirty = self.prev_bounds.map(|b| b.union(bounds));
        self.prev_bounds = Some(bounds);
        self.pill_key = self.app.pill_key();
        self.last_frame = Instant::now();
        // Slides by the notch's own height so it starts moving at once, and fades on the way.
        let peek = self.app.peek();
        let pos = POINT { x: self.origin.x, y: self.origin.y - ((bounds.h + 4.0) * self.scale * (1.0 - peek)) as i32 };
        let alpha = (255.0 * peek * peek * (3.0 - 2.0 * peek)) as u8;
        let app = &mut self.app;
        let _ = self.renderer.frame(self.hwnd, dirty, pos, alpha, |p| app.draw(p));
    }

    /// Repaints unless the notch is collapsed and the pill would look the same.
    fn refresh(&mut self) {
        if !self.app.closed() || self.app.pill_key() != self.pill_key {
            self.render();
        }
    }

    fn start_anim(&mut self) {
        if self.app.needs_animation() && !self.animating {
            self.animating = true;
            // The message loop drives frames; this timer only keeps them coming inside modal loops (menus, drag-out).
            unsafe { SetTimer(self.hwnd, TIMER_ANIM, 16, None) };
        }
    }

    fn step_anim(&mut self) {
        if !self.app.animate() {
            self.animating = false;
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ANIM);
            }
        }
        self.render();
        if !self.app.shown && self.app.peek() == 0.0 {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Over a full-screen app the notch waits above the screen; pushing the cursor into the
    /// top-middle edge brings it back over that app until the cursor leaves it alone.
    fn auto_hide(&mut self) {
        if !self.fullscreen {
            self.peeked = false;
        } else {
            let mut pt = POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };
            let mid = self.origin.x + (app::CANVAS_W * self.scale) as i32 / 2;
            if pt.y <= self.origin.y && ((pt.x - mid).abs() as f32) < 120.0 * self.scale {
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
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                // The full-screen window is topmost too; climb back above it.
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        self.start_anim();
    }

    fn mouse(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xffff) as i16 as f32;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
        (x / self.scale, y / self.scale)
    }

    fn idle_ms() -> u32 {
        let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        unsafe {
            if GetLastInputInfo(&mut info).as_bool() { GetTickCount().wrapping_sub(info.dwTime) } else { 0 }
        }
    }

    /// Point the cat's eyes at the cursor, which may be anywhere on screen.
    fn aim_eyes(&mut self) -> bool {
        let mut pt = POINT::default();
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return false;
        }
        let (cx, cy) = self.app.cat_pos();
        let dx = (pt.x - self.origin.x) as f32 / self.scale - cx;
        let dy = (pt.y - self.origin.y) as f32 / self.scale - cy;
        // Same reach as Nusa's mascot.
        let look = cat::gaze(dx, dy);
        let old = self.app.mascot.look;
        let moved = (look.0 - old.0).abs() > 0.05 || (look.1 - old.1).abs() > 0.05;
        self.app.mascot.look = look;
        moved
    }
}

/// The window never activates on click, so take the keyboard explicitly for typing.
/// Windows sometimes refuses a background app the foreground; joining the focused
/// app's input queue for a moment lets the switch through.
fn take_keyboard(hwnd: HWND) {
    unsafe {
        if !SetForegroundWindow(hwnd).as_bool() || GetForegroundWindow() != hwnd {
            let fg = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let me = GetCurrentThreadId();
            if fg != 0 && fg != me && AttachThreadInput(me, fg, true).as_bool() {
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                let _ = AttachThreadInput(me, fg, false);
            }
        }
        let _ = SetFocus(hwnd);
    }
}

/// Asks for WM_MOUSELEAVE once the cursor leaves the notch (posted at once if it already has).
fn track_leave(hwnd: HWND) {
    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
    unsafe {
        let _ = TrackMouseEvent(&mut tme);
    }
}

fn on_drag(event: shelf::DragEvent) {
    with_state(|s| {
        // A drop happens on the notch, but the drag swallowed the mouse moves that would mark it hovered;
        // stay open so the new card is seen falling in.
        if matches!(event, shelf::DragEvent::Drop(_)) {
            s.app.hover = true;
            track_leave(s.hwnd);
        }
        s.app.drag(event);
        s.start_anim();
        s.render();
    });
}

/// Starts dragging a shelf item out once the pressed mouse has moved far enough.
fn maybe_drag_out(wp: WPARAM, lp: LPARAM) {
    let path = with_state(|s| {
        let (x, y) = s.mouse(lp);
        let (px, py) = s.press?;
        if wp.0 & MK_LBUTTON.0 as usize == 0 || ((x - px).powi(2) + (y - py).powi(2)) < 36.0 {
            return None;
        }
        s.press = None;
        s.app.shelf_item_at(px, py)
    })
    .flatten();
    // DoDragDrop runs a modal loop that calls back into on_drag, so no borrow here.
    if let Some(path) = path {
        shelf::drag_out(&path);
    }
}

fn on_tray(hwnd: HWND, event: u32) {
    if event != WM_RBUTTONUP && event != WM_CONTEXTMENU {
        return;
    }
    let Some((cat, compact)) = with_state(|s| (s.app.cat, s.app.compact)) else { return };
    // The menu runs its own message loop, so no state may be borrowed while it is open.
    let Some(cmd) = tray::menu(hwnd, cat, compact) else { return };
    match cmd {
        tray::Command::Autostart => tray::set_autostart(!tray::autostart_enabled()),
        tray::Command::About => tray::about(hwnd),
        tray::Command::Quit => unsafe {
            let _ = DestroyWindow(hwnd);
        },
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
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_state(|s| match wp.0 {
                TIMER_TICK => {
                    s.fullscreen = status::fullscreen_app();
                    s.app.tick(State::idle_ms());
                    s.start_anim();
                    s.refresh();
                }
                TIMER_ANIM => {
                    // Only step when the message loop is stuck in a modal loop and not rendering.
                    if s.last_frame.elapsed() > Duration::from_millis(30) {
                        s.step_anim();
                    }
                }
                TIMER_OPEN => {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_OPEN);
                    }
                    s.open_pending = false;
                    s.app.hover = true;
                    s.app.mascot.poke(Instant::now());
                    s.start_anim();
                }
                TIMER_COLLAPSE => {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_COLLAPSE);
                    }
                    if s.app.scrubbing() {
                        return;
                    }
                    s.app.hover = false;
                    s.start_anim();
                }
                TIMER_EYES => {
                    s.auto_hide();
                    let caps = s.app.poll_caps();
                    if caps {
                        s.start_anim();
                    }
                    // Lyric lines in the closed pill change faster than the 1 s tick.
                    if s.app.poll_pill_lyric() {
                        s.start_anim();
                    }
                    if s.app.closed() {
                        s.refresh();
                    }
                    let now = Instant::now();
                    if s.app.cat && State::idle_ms() < 400 {
                        s.app.mascot.poke(now);
                    }
                    if s.app.cat && s.app.mascot.tracking(now) {
                        s.aim_eyes();
                    }
                    // Blinks, snores, wake-ups and eye moves all run on the frame loop.
                    s.start_anim();
                    if caps && !s.animating {
                        s.render();
                    }
                }
                _ => {}
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_state(|s| {
                unsafe {
                    let _ = KillTimer(hwnd, TIMER_COLLAPSE);
                }
                track_leave(hwnd);
                let (x, y) = s.mouse(lp);
                s.app.mouse = Some((x, y));
                if s.app.scrubbing() {
                    s.app.scrub_to(x);
                    s.render();
                    return;
                }
                let hot = s.app.hot_rect();
                if hot != s.app.hot {
                    s.app.hot = hot;
                    s.render();
                }
                if !s.app.hover && !s.open_pending {
                    // Brief hover intent so reaching past the notch for a tab or title bar doesn't pop it open.
                    s.open_pending = true;
                    unsafe { SetTimer(hwnd, TIMER_OPEN, 80, None) };
                }
            });
            maybe_drag_out(wp, lp);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            with_state(|s| {
                let (x, y) = s.mouse(lp);
                if s.app.press(x, y) {
                    // Keep getting moves and the button-up even if the drag leaves the notch.
                    unsafe { SetCapture(hwnd) };
                    s.render();
                } else {
                    s.press = Some((x, y));
                }
            });
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_state(|s| {
                if s.app.release() {
                    s.render();
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_state(|s| {
                s.open_pending = false;
                s.app.mouse = None;
                s.app.hot = None;
            });
            unsafe {
                let _ = KillTimer(hwnd, TIMER_OPEN);
                // Short grace period so brushing past the edge doesn't snap it shut.
                SetTimer(hwnd, TIMER_COLLAPSE, 150, None);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_state(|s| {
                let (x, y) = s.mouse(lp);
                s.press = None;
                if s.app.release() {
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    // A drag that ended outside skipped the collapse; this re-posts the leave if so.
                    track_leave(hwnd);
                    s.render();
                    return;
                }
                s.app.click(x, y);
                if s.app.editing.is_some() {
                    take_keyboard(hwnd);
                }
                s.start_anim();
                s.render();
            });
            LRESULT(0)
        }
        WM_CHAR => {
            with_state(|s| {
                if let Some(c) = char::from_u32(wp.0 as u32) {
                    s.app.type_char(c);
                    s.render();
                }
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            with_state(|s| {
                match VIRTUAL_KEY(wp.0 as u16) {
                    VK_RETURN => s.app.finish_edit(true),
                    VK_ESCAPE => s.app.finish_edit(false),
                    VK_BACK => s.app.backspace(),
                    _ => return,
                }
                s.start_anim();
                s.render();
            });
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            with_state(|s| {
                s.app.finish_edit(true);
                s.start_anim();
                s.render();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wp.0 >> 16) as u16 as i16;
            with_state(|s| {
                s.app.scroll(if delta > 0 { -1 } else { 1 });
                s.render();
            });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR if with_state(|s| s.app.hot.is_some()).unwrap_or(false) => {
            unsafe {
                if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                    SetCursor(hand);
                }
            }
            LRESULT(1)
        }
        bluetooth::WM_BLUETOOTH => {
            with_state(|s| {
                let changes = s.bluetooth.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
                s.app.on_bluetooth(changes);
                s.start_anim();
                s.render();
            });
            LRESULT(0)
        }
        media::WM_MEDIA => {
            with_state(|s| {
                s.app.on_media();
                s.start_anim();
                s.refresh();
            });
            LRESULT(0)
        }
        tray::WM_TRAY => {
            on_tray(hwnd, (lp.0 & 0xffff) as u32);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            if with_state(|s| s.taskbar_created == msg).unwrap_or(false) {
                tray::add(hwnd);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _mutex = CreateMutexW(None, true, w!("Local\\Purrch.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("PurrchNotch"),
            ..Default::default()
        };
        RegisterClassExW(&class);

        let scale = GetDpiForSystem() as f32 / 96.0;
        let (w, h) = ((app::CANVAS_W * scale) as i32, (app::CANVAS_H * scale) as i32);
        let origin = POINT { x: (GetSystemMetrics(SM_CXSCREEN) - w) / 2, y: 0 };
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("PurrchNotch"),
            w!("Purrch"),
            WS_POPUP,
            origin.x,
            origin.y,
            w,
            h,
            None,
            None,
            instance,
            None,
        )?;

        // COM before App::load: it opens the audio endpoint for the volume controls.
        let ole = OleInitialize(None).is_ok();
        let mut app = App::load();
        app.media = Some(media::Media::start(hwnd));
        let state = State {
            hwnd,
            app,
            renderer: Renderer::new(w, h)?,
            scale,
            origin,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            press: None,
            bluetooth: bluetooth::start(hwnd),
            animating: false,
            last_frame: Instant::now(),
            prev_bounds: None,
            pill_key: String::new(),
            open_pending: false,
            fullscreen: false,
            peeked: false,
            away_since: None,
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        with_state(|s| s.render());
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        tray::add(hwnd);
        // Without drag-and-drop the shelf is unusable, but the rest of the app still works.
        if ole {
            let _ = shelf::DropTarget::register(hwnd, on_drag);
        }
        SetTimer(hwnd, TIMER_TICK, 1000, None);
        SetTimer(hwnd, TIMER_EYES, 120, None);

        let mut msg = MSG::default();
        loop {
            if with_state(|s| s.animating).unwrap_or(false) {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return Ok(());
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                with_state(|s| {
                    if s.animating {
                        s.step_anim();
                    }
                });
                // Wait for the compositor so each frame lands once per refresh instead of on a coarse timer.
                if DwmFlush().is_err() {
                    std::thread::sleep(Duration::from_millis(16));
                }
            } else if GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                break;
            }
        }
        Ok(())
    }
}
