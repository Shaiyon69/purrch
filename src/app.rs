use crate::cat::Cat;
use crate::lyrics;
use crate::media::{self, LyricState, Media};
use crate::pomodoro::{Mode, Pomodoro};
use crate::status::{self, Net, Power};
use crate::shelf::{self, Shelf};
use crate::store;
use crate::todo::{Task, Todo};
use crate::ui::{Align, Color, Font, Painter, Rect, rgba};
use crate::water::{self, Prompt, Water};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::core::w;

pub const CANVAS_W: f32 = 600.0;
pub const CANVAS_H: f32 = 170.0;
const OPEN_W: f32 = 420.0;
/// The panel widens for lyrics: player on the left, scrolling lines on the right.
const LYRICS_W: f32 = 520.0;
const OPEN_H: f32 = 140.0;
const PILL_H: f32 = 30.0;
/// Widest concave fillet where the notch meets the top edge.
const SHOULDER: f32 = 10.0;
const SETTINGS: &str = "settings.txt";

// Pure black so the notch disappears into an OLED panel and the bezel.
const BG: Color = rgba(0.0, 0.0, 0.0, 1.0);
/// Inset surfaces (list rows, the to-do card).
const CARD: Color = rgba(0.1, 0.1, 0.11, 1.0);
/// Buttons, tracks and steppers; CHIP_ON marks the selected one.
const CHIP: Color = rgba(0.17, 0.17, 0.18, 1.0);
const CHIP_ON: Color = rgba(0.3, 0.3, 0.32, 1.0);
const DOT: Color = rgba(0.38, 0.38, 0.4, 1.0);
const TEXT: Color = rgba(0.96, 0.96, 0.97, 1.0);
const MUTED: Color = rgba(0.62, 0.62, 0.66, 1.0);
const WATER: Color = rgba(0.33, 0.66, 1.0, 1.0);
const WATER_DEEP: Color = rgba(0.08, 0.2, 0.36, 1.0);
const FOCUS: Color = rgba(1.0, 0.32, 0.32, 1.0);
const FOCUS_DEEP: Color = rgba(0.3, 0.08, 0.08, 1.0);
const TASK: Color = rgba(0.3, 0.85, 0.45, 1.0);
const SHELF: Color = rgba(0.78, 0.6, 1.0, 1.0);
const LYRIC_GLOW: Color = rgba(0.23, 0.48, 0.82, 0.95);
const SHELF_GLOW: Color = rgba(0.18, 0.36, 1.0, 0.45);
const SHELF_COLS: usize = 5;
/// Carousel order of the panel's pages.
const PAGES: [Tab; 5] = [Tab::Music, Tab::Focus, Tab::Water, Tab::Shelf, Tab::Todo];

#[derive(Clone, Copy)]
enum Shape {
    Play,
    Pause,
    Prev,
    Next,
}

#[derive(Clone, Copy)]
enum Ico {
    Glyph(&'static str),
    Drop,
    None,
}
const TOAST_SECS: u64 = 3;
/// Page slide length; the outgoing page is gone by 60% of it.
const PAGE_SECS: f32 = 0.24;
const TODO_ROWS: usize = 3;

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Water,
    Focus,
    Todo,
    Shelf,
    Music,
}

#[derive(Clone, PartialEq)]
pub enum Alert {
    Water(Prompt),
    Pomodoro(Mode),
    Task(String),
}

/// A short status line shown in the closed pill.
struct Toast {
    title: String,
    detail: String,
    color: Color,
    until: Instant,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Tab(Tab),
    Lyrics,
    WaterChart,
    Sip,
    Refill,
    Snooze,
    Dismiss,
    WaterSettings(bool),
    Bottle(i32),
    Goal(u32),
    Start(i32),
    End(i32),
    ToggleReminders,
    Reset,
    PomoMode(Mode),
    PomoToggle,
    PomoReset,
    PomoStartNext,
    TodoAdd,
    TodoToggle(usize),
    TodoDelete(usize),
    TodoSave,
    ShelfOpen(usize),
    ShelfRemove(usize),
    ShelfClear,
    Media(media::Cmd),
    Mute,
    OutputNext,
    /// Dragged rather than clicked: see `App::press`.
    Slider(Slider),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Slider {
    Seek,
    Volume,
}

struct Clock {
    day: u32,
    hour: usize,
    minute: u16,
    /// Minutes since midnight, with seconds as a fraction.
    now: f32,
}

fn clock() -> Clock {
    let t = unsafe { GetLocalTime() };
    Clock {
        day: t.wYear as u32 * 10000 + t.wMonth as u32 * 100 + t.wDay as u32,
        hour: t.wHour as usize,
        minute: t.wHour * 60 + t.wMinute,
        now: (t.wHour * 60 + t.wMinute) as f32 + t.wSecond as f32 / 60.0,
    }
}

fn hhmm(minutes: f32) -> String {
    let m = minutes as u32;
    format!("{:02}:{:02}", m / 60 % 24, m % 60)
}

fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Elapsed time, a progress bar and time left, between `left` and `right`, centred on `cy`.
/// Returns the bar's hit area, if the track has a length to seek in.
fn progress(p: &Painter, left: f32, right: f32, cy: f32, track: &media::Track, pos: Duration) -> Option<Rect> {
    if track.length == Duration::ZERO {
        return None;
    }
    p.text(&mmss(pos), Rect::new(left, cy - 7.0, 44.0, 14.0), Font::Small, TEXT, Align::Left);
    let frac = (pos.as_secs_f32() / track.length.as_secs_f32()).min(1.0);
    let hit = slider(p, Rect::new(left + 48.0, cy - 1.0, right - left - 96.0, 3.0), frac);
    let rest = format!("-{}", mmss(track.length.saturating_sub(pos)));
    p.text(&rest, Rect::new(right - 48.0, cy - 7.0, 48.0, 14.0), Font::Small, TEXT, Align::Right);
    Some(hit)
}

/// A thin bar filled to `frac`; returns a taller hit area, padded by SLIDER_PAD either side.
fn slider(p: &Painter, bar: Rect, frac: f32) -> Rect {
    p.round(bar, 1.5, CHIP);
    p.round(Rect { w: bar.w * frac, ..bar }, 1.5, TEXT);
    Rect::new(bar.x - SLIDER_PAD, bar.y + bar.h / 2.0 - 8.0, bar.w + 2.0 * SLIDER_PAD, 16.0)
}

const SLIDER_PAD: f32 = 4.0;

/// Where `x` falls along a slider's hit area from `slider`, 0..1.
fn slider_frac(hit: Rect, x: f32) -> f32 {
    ((x - hit.x - SLIDER_PAD) / (hit.w - 2.0 * SLIDER_PAD).max(1.0)).clamp(0.0, 1.0)
}

fn chime() {
    unsafe {
        let _ = PlaySoundW(w!("SystemAsterisk"), None, SND_ALIAS | SND_ASYNC);
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub struct App {
    pub water: Water,
    pub pomo: Pomodoro,
    pub cat: bool,
    pub compact: bool,
    tab: Tab,
    water_settings: bool,
    reset_armed: Option<Instant>,
    alert: Option<Alert>,
    pub hover: bool,
    /// 0 = collapsed pill, 1 = fully open panel.
    anim: f32,
    vel: f32,
    anim_at: Option<Instant>,
    pub mascot: Cat,
    hits: Vec<(Rect, Action)>,
    pub todo: Todo,
    /// Text being typed for a new task; Some while the keyboard is ours.
    pub editing: Option<String>,
    todo_scroll: usize,
    toast: Option<Toast>,
    power: Option<Power>,
    net: Option<Net>,
    caps: Option<bool>,
    ticks: u32,
    pub shelf: Shelf,
    shelf_scroll: usize,
    /// Something is being dragged over the notch.
    pub drag_hover: bool,
    pub media: Option<Media>,
    art: Option<(u64, ID2D1Bitmap, Color)>,
    now_playing: String,
    show_lyrics: bool,
    water_chart: bool,
    /// Page being slid away: which one, slide direction (-1 or 1), and when it started.
    page_from: Option<(Tab, f32, Instant)>,
    /// Water progress as drawn; eases toward the real value so a sip visibly fills the gauge.
    water_shown: f32,
    /// Cursor position over the notch, and the hit rect under it for hover feedback.
    pub mouse: Option<(f32, f32)>,
    pub hot: Option<Rect>,
    /// Windows "Show animations" is off: pages fade instead of sliding.
    reduced_motion: bool,
    has_track: bool,
    /// 0 = normal player, 1 = widened lyrics view.
    wide: f32,
    wide_vel: f32,
    /// The slider under a held mouse button, with its hit area to map the cursor onto.
    scrub: Option<(Slider, Rect)>,
    /// Where the progress bar is being dragged to; sent as a seek on release.
    seek_to: Option<Duration>,
    volume: Option<media::Volume>,
    /// Lyric list scroll in DIPs from the first line, springing toward `ly_target`.
    ly_pos: f32,
    ly_vel: f32,
    ly_target: f32,
    /// Wrapped height of each lyric line, measured once per song (keyed by song).
    ly_heights: (String, Vec<f32>),
    /// Drop-zone highlight as drawn, easing toward `drag_hover`.
    drag_shown: f32,
    drag_since: Instant,
    /// Size labels and shell thumbnails, filled the first time a card is drawn.
    shelf_cache: HashMap<PathBuf, (String, Option<ID2D1Bitmap>)>,
    /// When each just-dropped item starts falling in.
    shelf_added: Vec<(PathBuf, Instant)>,
}

/// Moves `v` toward `target` at `rate` per second (exponential), snapping when close.
fn approach(v: f32, target: f32, dt: f32, rate: f32) -> f32 {
    let next = v + (target - v) * (1.0 - (-dt * rate).exp());
    if (target - next).abs() < 0.002 { target } else { next }
}

impl App {
    pub fn load() -> App {
        let s = store::load(SETTINGS);
        let water = std::fs::read_to_string(store::dir().join(water::FILE))
            .map(|t| Water::from_text(&t))
            .unwrap_or_default();
        let mut app = App {
            water,
            pomo: Pomodoro::default(),
            cat: store::get(&s, "cat", 1u8) != 0,
            compact: store::get(&s, "compact", 0u8) != 0,
            tab: Tab::Music,
            water_settings: false,
            reset_armed: None,
            alert: None,
            hover: false,
            anim: 0.0,
            vel: 0.0,
            anim_at: None,
            mascot: Cat::new(Instant::now()),
            hits: Vec::new(),
            todo: Todo::load(),
            editing: None,
            todo_scroll: 0,
            toast: None,
            power: status::power(),
            net: None,
            caps: None,
            ticks: 0,
            shelf: Shelf::load(),
            shelf_scroll: 0,
            drag_hover: false,
            media: None,
            art: None,
            now_playing: String::new(),
            show_lyrics: false,
            water_chart: false,
            page_from: None,
            water_shown: 0.0,
            mouse: None,
            hot: None,
            reduced_motion: !animations_enabled(),
            has_track: false,
            wide: 0.0,
            wide_vel: 0.0,
            scrub: None,
            seek_to: None,
            volume: media::Volume::open(),
            ly_pos: 0.0,
            ly_vel: 0.0,
            ly_target: 0.0,
            ly_heights: (String::new(), Vec::new()),
            drag_shown: 0.0,
            drag_since: Instant::now(),
            shelf_cache: HashMap::new(),
            shelf_added: Vec::new(),
        };
        app.water.new_day(clock().day);
        app
    }

    pub fn save_settings(&self) {
        let _ = store::save(SETTINGS, &format!("cat={}\ncompact={}\n", self.cat as u8, self.compact as u8));
    }

    fn save_water(&self) {
        let _ = store::save(water::FILE, &self.water.to_text());
    }

    fn open(&self) -> bool {
        self.hover || self.alert.is_some() || self.editing.is_some() || self.drag_hover
    }

    pub fn on_bluetooth(&mut self, changes: Vec<crate::bluetooth::Change>) {
        use crate::bluetooth::Change;
        // Several devices at once: the last one wins the pill, which is fine for a passing note.
        for change in changes {
            match change {
                Change::Connected(name) => self.toast(name, "connected".into(), WATER),
                Change::Disconnected(name) => self.toast(name, "disconnected".into(), MUTED),
            }
        }
    }

    fn media_snapshot(&self) -> media::Snapshot {
        self.media.as_ref().map(Media::snapshot).unwrap_or_default()
    }

    /// The media worker posted an update; announce a new song in the pill.
    pub fn on_media(&mut self) {
        let track = self.media_snapshot().track;
        self.has_track = track.is_some();
        let Some(track) = track else { return };
        let key = format!("{} {}", track.title, track.artist);
        if track.playing && key != self.now_playing && !track.title.is_empty() {
            self.now_playing = key;
            self.toast(track.title, track.artist, SHELF);
        }
    }

    pub fn drag(&mut self, event: shelf::DragEvent) {
        match event {
            shelf::DragEvent::Enter => {
                self.drag_hover = true;
                self.drag_since = Instant::now();
                self.alert = None;
                self.tab = Tab::Shelf;
            }
            shelf::DragEvent::Leave => self.drag_hover = false,
            shelf::DragEvent::Drop(paths) => {
                self.drag_hover = false;
                // Several files land one after another, like Nusa's cards.
                let now = Instant::now();
                let new: Vec<PathBuf> = paths.iter().filter(|p| !self.shelf.items.contains(p)).cloned().collect();
                let landed = !new.is_empty();
                for (i, p) in new.into_iter().enumerate() {
                    self.shelf_added.push((p, now + Duration::from_millis(120 * i as u64)));
                }
                self.shelf.add(paths);
                // New cards go at the end; bring them into view.
                if landed {
                    self.shelf_scroll = self.shelf.items.len().saturating_sub(SHELF_COLS);
                }
            }
        }
    }

    /// Shelf item under the cursor, for starting a drag-out.
    pub fn shelf_item_at(&self, x: f32, y: f32) -> Option<std::path::PathBuf> {
        self.hits.iter().rev().find_map(|(r, a)| match a {
            Action::ShelfOpen(i) if r.contains(x, y) => self.shelf.items.get(*i).cloned(),
            _ => None,
        })
    }

    pub fn scroll(&mut self, delta: i32) {
        // Over the volume bar the wheel turns the volume (wheel up = louder).
        let over_volume = self.mouse.is_some_and(|(x, y)| self.hits.iter().any(|(r, a)| *a == Action::Slider(Slider::Volume) && r.contains(x, y)));
        if over_volume {
            self.volume = media::Volume::open();
            if let Some(v) = &self.volume {
                v.set(v.level() - 0.05 * delta as f32);
            }
            return;
        }
        // Lists scroll first; at either end the wheel flips the page.
        let before = (self.todo_scroll, self.shelf_scroll);
        match self.tab {
            Tab::Todo => self.scroll_todo(delta),
            Tab::Shelf => self.scroll_shelf(delta),
            _ => {}
        }
        if (self.todo_scroll, self.shelf_scroll) == before {
            self.turn(delta);
        }
    }

    /// Moves `d` pages along the carousel, wrapping at the ends.
    fn turn(&mut self, d: i32) {
        let i = PAGES.iter().position(|t| *t == self.tab).unwrap_or(0) as i32;
        self.go(PAGES[(i + d).rem_euclid(PAGES.len() as i32) as usize], d.signum() as f32);
    }

    /// Switches page, sliding the old one out toward `-dir`.
    fn go(&mut self, tab: Tab, dir: f32) {
        if tab != self.tab {
            self.page_from = Some((self.tab, dir, Instant::now()));
            self.reduced_motion = !animations_enabled();
        }
        self.tab = tab;
        self.water_settings = false;
    }

    fn page_index(tab: Tab) -> i32 {
        PAGES.iter().position(|t| *t == tab).unwrap_or(0) as i32
    }

    fn scroll_shelf(&mut self, delta: i32) {
        let max = self.shelf.items.len().saturating_sub(SHELF_COLS);
        self.shelf_scroll = (self.shelf_scroll as i32 + delta).clamp(0, max as i32) as usize;
    }

    fn toast(&mut self, title: String, detail: String, color: Color) {
        self.toast = Some(Toast { title, detail, color, until: Instant::now() + Duration::from_secs(TOAST_SECS) });
    }

    /// Polled often; returns true when the Caps Lock state changed.
    pub fn poll_caps(&mut self) -> bool {
        let on = status::caps_lock();
        let changed = self.caps.is_some_and(|was| was != on);
        if changed {
            let state = if on { "on" } else { "off" };
            self.toast("Caps Lock".into(), state.into(), if on { TASK } else { MUTED });
        }
        self.caps = Some(on);
        changed
    }

    fn poll_status(&mut self) {
        if let (Some(old), Some(new)) = (self.power, status::power()) {
            if let Some((title, detail)) = status::power_message(old, new) {
                self.toast(title, detail, TASK);
            }
            self.power = Some(new);
        }
        // Network lookups cost more than the rest, so check every few seconds.
        if self.ticks.is_multiple_of(3) {
            let net = status::network();
            if self.net.is_some_and(|old| old != net) {
                let (title, detail) = status::network_message(net);
                self.toast(title, detail, if net == Net::Online { WATER } else { FOCUS });
            }
            self.net = Some(net);
        }
    }

    pub fn type_char(&mut self, c: char) {
        if let Some(text) = &mut self.editing
            && !c.is_control() && text.chars().count() < 80 {
                text.push(c);
            }
    }

    pub fn backspace(&mut self) {
        if let Some(text) = &mut self.editing {
            text.pop();
        }
    }

    /// Enter (or losing focus) saves the task; Escape throws it away.
    pub fn finish_edit(&mut self, keep: bool) {
        let Some(text) = self.editing.take() else { return };
        if !keep {
            return;
        }
        if self.todo.write(None, &text) {
            self.todo.save();
        }
    }

    /// Steps the open/close spring by real elapsed time. Returns true while still moving.
    pub fn animate(&mut self) -> bool {
        let target = if self.open() { 1.0 } else { 0.0 };
        let now = Instant::now();
        // Cap the step so a stalled frame doesn't teleport the panel.
        let dt = self.anim_at.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).min(0.05);
        self.anim_at = Some(now);
        // Critically damped: ~90% of the way in 0.14 s, settled by ~0.26 s, and smooth when reversed mid-way.
        const OMEGA: f32 = 28.0;
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(0.002);
            self.vel += (OMEGA * OMEGA * (target - self.anim) - 2.0 * OMEGA * self.vel) * h;
            self.anim += self.vel * h;
            left -= h;
        }
        if (target - self.anim).abs() < 0.004 && self.vel.abs() < 0.05 {
            self.anim = target;
            self.vel = 0.0;
        }
        if self.page_from.is_some_and(|(_, _, at)| at.elapsed().as_secs_f32() >= PAGE_SECS) {
            self.page_from = None;
        }
        if self.cat {
            self.mascot.step(now, dt);
        }
        // Critically damped like the open spring, but slower: the player has to glide across.
        let wide = self.lyrics_mode() as u8 as f32;
        if self.reduced_motion {
            (self.wide, self.wide_vel) = (wide, 0.0);
        }
        const WIDE_OMEGA: f32 = 16.0;
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(0.002);
            self.wide_vel += (WIDE_OMEGA * WIDE_OMEGA * (wide - self.wide) - 2.0 * WIDE_OMEGA * self.wide_vel) * h;
            self.wide = (self.wide + self.wide_vel * h).clamp(0.0, 1.0);
            left -= h;
        }
        if (wide - self.wide).abs() < 0.002 && self.wide_vel.abs() < 0.05 {
            (self.wide, self.wide_vel) = (wide, 0.0);
        }
        self.drag_shown = approach(self.drag_shown, self.drag_hover as u8 as f32, dt, 10.0);
        // Slightly underdamped, so a new line settles with a small overshoot.
        const LY_OMEGA: f32 = 11.0;
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(0.002);
            self.ly_vel += (LY_OMEGA * LY_OMEGA * (self.ly_target - self.ly_pos) - 1.4 * LY_OMEGA * self.ly_vel) * h;
            self.ly_pos += self.ly_vel * h;
            left -= h;
        }
        if (self.ly_target - self.ly_pos).abs() < 0.2 && self.ly_vel.abs() < 1.0 {
            self.ly_pos = self.ly_target;
            self.ly_vel = 0.0;
        }
        self.shelf_added.retain(|(_, at)| now < *at + Duration::from_secs(1));
        let water = self.water_target();
        self.water_shown += (water - self.water_shown) * (1.0 - (-dt * 10.0).exp());
        if (water - self.water_shown).abs() < 0.002 {
            self.water_shown = water;
        }
        let moving = self.needs_animation();
        if !moving {
            self.anim_at = None;
        }
        moving
    }

    pub fn needs_animation(&self) -> bool {
        self.vel != 0.0
            || self.anim != if self.open() { 1.0 } else { 0.0 }
            || self.page_from.is_some()
            || self.water_shown != self.water_target()
            || (self.cat && self.mascot.busy(Instant::now()))
            || self.wide != self.lyrics_mode() as u8 as f32
            || self.wide_vel != 0.0
            || self.ly_vel != 0.0
            || self.ly_pos != self.ly_target
            // Lines change at their own times, and the drop arrow bobs while a file hovers.
            || (self.lyrics_mode() && self.open())
            || self.drag_hover
            || self.drag_shown != self.drag_hover as u8 as f32
            || !self.shelf_added.is_empty()
    }

    /// Music page with lyrics switched on: the widened, split panel.
    fn lyrics_mode(&self) -> bool {
        self.show_lyrics && self.has_track && self.tab == Tab::Music && self.alert.is_none()
    }

    fn open_w(&self) -> f32 {
        lerp(OPEN_W, LYRICS_W, self.wide)
    }

    fn water_target(&self) -> f32 {
        (self.water.taken as f32 / self.water.goal.max(1) as f32).min(1.0)
    }

    /// Hit rect under the cursor, for hover feedback and the hand cursor.
    pub fn hot_rect(&self) -> Option<Rect> {
        let (x, y) = self.mouse?;
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(r, _)| *r)
    }

    /// Fully collapsed and not moving.
    pub fn closed(&self) -> bool {
        self.anim == 0.0 && self.vel == 0.0
    }

    /// What the closed pill shows; idle ticks only repaint when this changes.
    pub fn pill_key(&self) -> String {
        let label = if self.pomo.running() { mmss(self.pomo.remaining(Instant::now())) } else { hhmm(clock().now) };
        let toast = self.toast.as_ref().map_or("", |t| t.title.as_str());
        format!("{label}|{toast}|{}|{:?}|{}|{}", self.water.taken, self.mascot.mood, self.cat, self.compact)
    }

    /// Area the notch covers right now, shoulders included, for partial repaints.
    pub fn bounds(&self) -> Rect {
        let (x, w, h) = self.panel();
        Rect::new(x - SHOULDER, 0.0, w + 2.0 * SHOULDER, h)
    }

    /// Cat head centre in canvas DIPs, used to aim its eyes at the cursor.
    pub fn cat_pos(&self) -> (f32, f32) {
        let (x, y, _) = self.cat_place();
        (x, y)
    }

    /// The cat walks from the pill's left end to the panel's top-right corner as it opens.
    fn cat_place(&self) -> (f32, f32, f32) {
        let t = self.anim.clamp(0.0, 1.0);
        let pill_x = (CANVAS_W - self.pill_width()) / 2.0 + 22.0;
        let open_x = (CANVAS_W + self.open_w()) / 2.0 - 32.0;
        // A small swell mid-journey so it reads as a hop, not a slide.
        let s = 0.7 * (1.0 + 0.18 * (t * std::f32::consts::PI).sin());
        (lerp(pill_x, open_x, t), lerp(17.0, 19.0, t), s)
    }

    fn pill_width(&self) -> f32 {
        match (&self.toast, self.compact) {
            (Some(_), _) => 320.0,
            (None, true) => 120.0,
            (None, false) => 150.0,
        }
    }

    pub fn tick(&mut self, idle_ms: u32) {
        let c = clock();
        let before = self.water.day;
        self.water.new_day(c.day);
        if before != self.water.day {
            self.save_water();
        }
        if let Some(p) = self.water.check(c.now) {
            self.save_water();
            self.raise(Alert::Water(p));
        }
        if let Some(next) = self.pomo.tick(Instant::now()) {
            self.raise(Alert::Pomodoro(next));
        }
        if self.reset_armed.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            self.reset_armed = None;
        }
        if let Some(title) = self.todo.due(c.day, c.minute) {
            self.todo.save();
            self.raise(Alert::Task(title));
        }
        self.ticks = self.ticks.wrapping_add(1);
        self.poll_status();
        if self.toast.as_ref().is_some_and(|t| Instant::now() >= t.until) {
            self.toast = None;
        }
        let open = self.open();
        self.mascot.idle(idle_ms, open, Instant::now());
    }

    fn raise(&mut self, alert: Alert) {
        // A pomodoro alert outranks a water reminder that is already showing.
        if self.alert.is_none() || matches!(alert, Alert::Pomodoro(_)) {
            self.alert = Some(alert);
            chime();
        }
    }

    /// Mouse button down: grabs a slider if one is under the cursor. True if it did.
    pub fn press(&mut self, x: f32, y: f32) -> bool {
        let Some(&(r, Action::Slider(s))) = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)) else { return false };
        if s == Slider::Volume {
            // Reopen so a device switch since launch (headphones, ...) is followed.
            self.volume = media::Volume::open();
        }
        self.scrub = Some((s, r));
        self.scrub_to(x);
        true
    }

    pub fn scrubbing(&self) -> bool {
        self.scrub.is_some()
    }

    /// Volume follows the cursor live; a seek is only previewed until release.
    pub fn scrub_to(&mut self, x: f32) {
        let Some((s, r)) = self.scrub else { return };
        let frac = slider_frac(r, x);
        match s {
            Slider::Volume => {
                if let Some(v) = &self.volume {
                    v.set(frac);
                }
            }
            Slider::Seek => {
                let length = self.media_snapshot().track.map_or(Duration::ZERO, |t| t.length);
                self.seek_to = Some(length.mul_f32(frac));
            }
        }
    }

    /// Mouse button up after `press` grabbed a slider. True if one was held.
    pub fn release(&mut self) -> bool {
        let Some((s, _)) = self.scrub.take() else { return false };
        if let (Slider::Seek, Some(at), Some(m)) = (s, self.seek_to.take(), &self.media) {
            m.send(media::Cmd::Seek(at));
        }
        true
    }

    pub fn click(&mut self, x: f32, y: f32) {
        let Some(&(_, action)) = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)) else { return };
        let c = clock();
        let now = Instant::now();
        match action {
            Action::Tab(t) => {
                let dir = (Self::page_index(t) - Self::page_index(self.tab)).signum() as f32;
                // Leaving an alert for a page is a jump, not a slide.
                if self.alert.take().is_some() {
                    self.tab = t;
                    self.water_settings = false;
                } else {
                    self.go(t, dir);
                }
            }
            Action::Lyrics => self.show_lyrics = !self.show_lyrics,
            Action::WaterChart => self.water_chart = !self.water_chart,
            Action::Sip => {
                self.water.sip(c.hour);
                self.alert = None;
            }
            Action::Refill => {
                self.water.refill();
                // Refilling answers "your bottle is empty"; the sip is still due.
                self.alert = (self.alert == Some(Alert::Water(Prompt::Refill))).then_some(Alert::Water(Prompt::Drink));
            }
            Action::Snooze => {
                self.water.snooze(c.minute);
                self.alert = None;
            }
            Action::Dismiss => self.alert = None,
            Action::WaterSettings(on) => self.water_settings = on,
            Action::Bottle(d) => self.water.step_bottle(d),
            Action::Goal(g) => self.water.set_goal(g),
            Action::Start(d) => self.water.step_start(d),
            Action::End(d) => self.water.step_end(d),
            Action::ToggleReminders => self.water.on = !self.water.on,
            Action::Reset => {
                if self.reset_armed.take().is_some() {
                    self.water.reset();
                } else {
                    self.reset_armed = Some(now);
                }
            }
            Action::PomoMode(m) => self.pomo.set_mode(m),
            Action::PomoToggle => self.pomo.toggle(now),
            Action::PomoReset => self.pomo.reset(),
            Action::PomoStartNext => {
                self.pomo.toggle(now);
                self.alert = None;
                self.tab = Tab::Focus;
            }
            Action::TodoAdd => self.editing = Some(String::new()),
            Action::TodoSave => self.finish_edit(true),
            Action::TodoToggle(i) => {
                if let Some(t) = self.todo.tasks.get_mut(i) {
                    t.done = !t.done;
                    self.todo.save();
                }
            }
            Action::TodoDelete(i) => {
                if i < self.todo.tasks.len() {
                    self.todo.tasks.remove(i);
                    self.todo.save();
                }
            }
            Action::ShelfOpen(i) => {
                if let Some(path) = self.shelf.items.get(i) {
                    shelf::open(path);
                }
            }
            Action::ShelfRemove(i) => {
                self.shelf.remove(i);
                self.scroll_shelf(0);
            }
            Action::ShelfClear => {
                self.shelf.clear();
                self.shelf_cache.clear();
                self.shelf_scroll = 0;
            }
            Action::Media(cmd) => {
                if let Some(m) = &self.media {
                    m.send(cmd);
                }
            }
            Action::Mute => {
                self.volume = media::Volume::open();
                if let Some(v) = &self.volume {
                    v.toggle_mute();
                }
            }
            Action::OutputNext => match media::next_output() {
                Some(name) => {
                    self.volume = media::Volume::open();
                    self.toast("Output".into(), name, SHELF);
                }
                None => self.toast("Output".into(), "No other device".into(), MUTED),
            },
            Action::Slider(_) => {}
        }
        if matches!(
            action,
            Action::Sip
                | Action::Refill
                | Action::Snooze
                | Action::Bottle(_)
                | Action::Goal(_)
                | Action::Start(_)
                | Action::End(_)
                | Action::ToggleReminders
                | Action::Reset
        ) {
            self.save_water();
        }
    }

    pub fn scroll_todo(&mut self, delta: i32) {
        let max = self.todo.tasks.len().saturating_sub(TODO_ROWS);
        self.todo_scroll = (self.todo_scroll as i32 + delta).clamp(0, max as i32) as usize;
    }

    /// Current panel position and size, interpolated between pill and open panel.
    fn panel(&self) -> (f32, f32, f32) {
        // The spring already eases; clamp in case a reversal nudges it past the ends.
        let t = self.anim.clamp(0.0, 1.0);
        let pill_w = self.pill_width();
        let w = lerp(pill_w, self.open_w(), t);
        ((CANVAS_W - w) / 2.0, w, lerp(PILL_H, OPEN_H, t))
    }

    pub fn draw(&mut self, p: &Painter) {
        self.hits.clear();
        let t = self.anim.clamp(0.0, 1.0);
        let (x, w, h) = self.panel();
        let radius = lerp(PILL_H / 2.0, 24.0, t);
        // Start above the screen edge so only the bottom corners look rounded.
        let shape = Rect::new(x, -radius, w, h + radius);
        p.round(shape, radius, BG);
        p.shoulders(x, w, lerp(8.0, SHOULDER, t), BG);
        let open_x = (CANVAS_W - self.open_w()) / 2.0;
        if t == 0.0 {
            self.draw_pill(p, x, w);
        } else if t == 1.0 {
            self.draw_open(p, open_x);
        } else {
            // Mid-morph the pill content fades out early and the panel content fades in,
            // already at its final place and revealed by the growing shape.
            let pill = 1.0 - (t / 0.35).min(1.0);
            if pill > 0.0 {
                p.push_clip(shape, radius, pill);
                self.draw_pill(p, x, w);
                p.pop_clip();
            }
            let open = ((t - 0.4) / 0.6).clamp(0.0, 1.0);
            if open > 0.0 {
                p.push_clip(shape, radius, open);
                self.draw_open(p, open_x);
                p.pop_clip();
            }
        }
        // One cat for both states, drawn on top so it can travel between them.
        // Minimal mode keeps the pill catless, so there it only fades in with the panel.
        if !self.compact || t > 0.6 {
            let (cx, cy, s) = self.cat_place();
            // The waving paw rises from below, so keep it inside the notch.
            let clip = self.cat && self.mascot.waving();
            if clip {
                p.push_clip(shape, radius, 1.0);
            }
            self.draw_cat(p, cx, cy, s);
            if clip {
                p.pop_clip();
            }
        }
    }

    /// Pill button with an optional leading icon, centred as one group.
    #[allow(clippy::too_many_arguments)]
    fn chip(&mut self, p: &Painter, r: Rect, icon: Ico, label: &str, action: Action, fill: Color, fg: Color) {
        p.round(r, r.h / 2.0, self.lit(r, fill));
        let lw = p.measure(label, Font::Small);
        let iw = if matches!(icon, Ico::None) { 0.0 } else { 16.0 };
        let sx = r.x + (r.w - iw - lw) / 2.0;
        match icon {
            Ico::Glyph(g) => p.text(g, Rect::new(sx, r.y, 12.0, r.h), Font::Icon, fg, Align::Center),
            Ico::Drop => drop_icon(p, sx + 6.0, r.y + r.h / 2.0, 1.0, fg),
            Ico::None => {}
        }
        p.text(label, Rect::new(sx + iw, r.y, lw + 1.0, r.h), Font::Small, fg, Align::Left);
        self.hits.push((r, action));
    }

    #[allow(clippy::too_many_arguments)]
    fn icon_button(&mut self, p: &Painter, r: Rect, glyph: &str, font: Font, action: Action, fill: Color, fg: Color) {
        p.round(r, r.h / 2.0, self.lit(r, fill));
        p.text(glyph, r, font, fg, Align::Center);
        self.hits.push((r, action));
    }

    /// Media-style button with a drawn, filled glyph.
    #[allow(clippy::too_many_arguments)]
    fn shape_button(&mut self, p: &Painter, r: Rect, shape: Shape, size: f32, action: Action, fill: Color, fg: Color) {
        p.round(r, r.h / 2.0, self.lit(r, fill));
        draw_shape(p, shape, r.x + r.w / 2.0, r.y + r.h / 2.0, size, fg);
        self.hits.push((r, action));
    }

    /// `fill`, brightened while the cursor is over `r`.
    fn lit(&self, r: Rect, fill: Color) -> Color {
        if self.hot != Some(r) {
            return fill;
        }
        if fill.a == 0.0 {
            return rgba(1.0, 1.0, 1.0, 0.12);
        }
        let up = |c: f32| (c + 0.1).min(1.0);
        rgba(up(fill.r), up(fill.g), up(fill.b), fill.a.max(1.0))
    }

    fn draw_cat(&self, p: &Painter, cx: f32, cy: f32, s: f32) {
        if self.cat {
            self.mascot.draw(p, cx, cy, s, Instant::now());
        }
    }

    fn draw_pill(&mut self, p: &Painter, x: f32, w: f32) {
        if let Some(t) = &self.toast {
            p.ellipse(x + 46.0, PILL_H / 2.0, 4.0, 4.0, t.color);
            p.text(&t.title, Rect::new(x + 56.0, 0.0, 130.0, PILL_H), Font::Body, TEXT, Align::Left);
            p.text(&t.detail, Rect::new(x + 190.0, 0.0, w - 206.0, PILL_H), Font::Small, MUTED, Align::Right);
            return;
        }
        let (label, color) = if self.pomo.running() {
            (mmss(self.pomo.remaining(Instant::now())), FOCUS)
        } else {
            (hhmm(clock().now), TEXT)
        };
        let align = if self.compact { Align::Center } else { Align::Right };
        p.text(&label, Rect::new(x + 16.0, 0.0, w - 34.0, PILL_H), Font::Body, color, align);
    }

    fn draw_open(&mut self, p: &Painter, x: f32) {
        let alert = self.alert.clone();
        if let Some(alert) = alert {
            self.draw_header(p, x);
            return self.draw_alert(p, x, alert);
        }
        // Switching lyrics on or off morphs the player into the other layout while the panel resizes.
        let lyr = if self.tab == Tab::Music && self.page_from.is_none() && self.has_track { self.wide } else { 0.0 };
        if lyr == 0.0 {
            return self.draw_pages(p, x);
        }
        // The header and page dots only exist without lyrics, so they fade out as the player moves.
        let keep = self.hits.len();
        if lyr < 1.0 {
            let mx = x + (self.open_w() - OPEN_W) / 2.0;
            p.push_clip(Rect::new(x, 0.0, self.open_w(), OPEN_H), 0.0, (1.0 - lyr * 2.0).max(0.0));
            self.draw_header(p, mx);
            self.draw_dots(p, mx);
            p.pop_clip();
        }
        if lyr >= 0.5 {
            self.hits.truncate(keep);
        }
        self.draw_lyrics(p, x, lyr);
    }

    /// Header, the current page (sliding if it just changed) and the page dots.
    fn draw_pages(&mut self, p: &Painter, x: f32) {
        self.draw_header(p, x);
        match self.page_from {
            Some((from, dir, at)) => {
                // The new page slides in from the arrow's side while the old one leaves the other way,
                // both clipped to the space between the arrows.
                let t = (at.elapsed().as_secs_f32() / PAGE_SECS).min(1.0);
                let ease = 1.0 - (1.0 - t).powi(3);
                let shift = if self.reduced_motion { 0.0 } else { 36.0 * dir };
                let lane = Rect::new(x + 34.0, 22.0, OPEN_W - 68.0, 100.0);
                let out = 1.0 - (t / 0.6).min(1.0);
                if out > 0.0 {
                    let keep = self.hits.len();
                    p.push_clip(lane, 0.0, out);
                    p.offset(-shift * ease);
                    self.draw_page(p, x, from);
                    p.offset(0.0);
                    p.pop_clip();
                    self.hits.truncate(keep);
                }
                p.push_clip(lane, 0.0, ease);
                p.offset(shift * (1.0 - ease));
                self.draw_page(p, x, self.tab);
                p.offset(0.0);
                p.pop_clip();
            }
            None => self.draw_page(p, x, self.tab),
        }
        self.draw_dots(p, x);
    }

    /// Page dots: the current page is a short bar.
    fn draw_dots(&mut self, p: &Painter, x: f32) {
        let gaps = (PAGES.len() - 1) as f32;
        let mut dx = x + (OPEN_W - (14.0 + gaps * 4.0 + gaps * 5.0)) / 2.0;
        for tab in PAGES {
            let on = tab == self.tab;
            let dw = if on { 14.0 } else { 4.0 };
            p.round(Rect::new(dx, 128.0, dw, 4.0), 2.0, if on { TEXT } else { DOT });
            self.hits.push((Rect::new(dx - 2.5, 122.0, dw + 5.0, 16.0), Action::Tab(tab)));
            dx += dw + 5.0;
        }
    }

    fn draw_page(&mut self, p: &Painter, x: f32, tab: Tab) {
        match tab {
            Tab::Water if self.water_settings => self.draw_water_settings(p, x),
            Tab::Water => self.draw_water(p, x),
            Tab::Focus => self.draw_focus(p, x),
            Tab::Todo => self.draw_todo(p, x),
            Tab::Shelf => self.draw_shelf(p, x),
            Tab::Music => self.draw_music(p, x),
        }
    }

    /// Time in the top-left corner, battery and date in the middle; the cat sits on the right.
    fn draw_header(&mut self, p: &Painter, x: f32) {
        let date = date_label();
        let time = hhmm(clock().now);
        let tw = p.measure(&time, Font::Body);
        p.text(&time, Rect::new(x + 16.0, 8.0, tw + 1.0, 20.0), Font::Body, TEXT, Align::Left);
        let battery = self.power.and_then(|pw| pw.percent.map(|pct| (pct, pw.plugged)));
        let bw = if battery.is_some() { 22.0 } else { 0.0 };
        let dw = p.measure(&date, Font::Small);
        let sx = x + (OPEN_W - bw - dw) / 2.0;
        if let Some((pct, plugged)) = battery {
            // Battery ring: green when charging or healthy, red when low on battery.
            let color = if !plugged && pct <= 20 { FOCUS } else { TASK };
            let (cx, cy) = (sx + 8.0, 18.0);
            p.arc(cx, cy, 7.0, 0.0, 360.0, CHIP, 1.8);
            p.arc(cx, cy, 7.0, -90.0, 3.6 * pct as f32, color, 1.8);
            if plugged {
                p.text("\u{E945}", Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0), Font::Small, color, Align::Center);
            }
        }
        p.text(&date, Rect::new(sx + bw, 8.0, dw + 1.0, 20.0), Font::Small, TEXT, Align::Left);
    }

    fn draw_music(&mut self, p: &Painter, x: f32) {
        let snap = self.media_snapshot();
        let Some(track) = snap.track.clone() else {
            p.text("Nothing playing", Rect::new(x, 50.0, OPEN_W, 22.0), Font::Title, TEXT, Align::Center);
            p.text("Play something in Spotify, a browser or any media app", Rect::new(x + 44.0, 74.0, OPEN_W - 88.0, 16.0), Font::Small, MUTED, Align::Center);
            return;
        };

        // Sits below the header's clock with room to breathe.
        self.draw_art(p, &snap, Rect::new(x + 44.0, 31.0, 48.0, 48.0));
        let (tx, tw) = (x + 110.0, OPEN_W - 154.0);
        p.text(&track.title, Rect::new(tx, 36.0, tw, 20.0), Font::Body, TEXT, Align::Left);
        p.text(&track.artist.to_uppercase(), Rect::new(tx, 55.0, tw, 16.0), Font::Small, MUTED, Align::Left);
        self.draw_progress(p, x + 44.0, x + OPEN_W - 44.0, 86.0, &track);

        let lyric_color = if self.show_lyrics { TEXT } else { MUTED };
        self.icon_button(p, Rect::new(x + 46.0, 96.0, 28.0, 24.0), "\u{E90A}", Font::Icon, Action::Lyrics, BG, lyric_color);
        let cx = x + OPEN_W / 2.0;
        let play = if track.playing { Shape::Pause } else { Shape::Play };
        self.shape_button(p, Rect::new(cx - 64.0, 96.0, 36.0, 24.0), Shape::Prev, 1.0, Action::Media(media::Cmd::Prev), BG, TEXT);
        self.shape_button(p, Rect::new(cx - 18.0, 96.0, 36.0, 24.0), play, 1.0, Action::Media(media::Cmd::PlayPause), BG, TEXT);
        self.shape_button(p, Rect::new(cx + 28.0, 96.0, 36.0, 24.0), Shape::Next, 1.0, Action::Media(media::Cmd::Next), BG, TEXT);
        self.draw_volume(p, Rect::new(cx + 72.0, 96.0, 104.0, 24.0), true);
    }

    /// The progress bar, showing where it is being dragged to while scrubbed.
    fn draw_progress(&mut self, p: &Painter, left: f32, right: f32, cy: f32, track: &media::Track) {
        let pos = self.seek_to.unwrap_or_else(|| track.position());
        if let Some(hit) = progress(p, left, right, cy, track, pos) {
            self.hits.push((hit, Action::Slider(Slider::Seek)));
        }
    }

    /// Mute button, a bar for the system volume and, if `output`, a next-device button, filling `r`.
    fn draw_volume(&mut self, p: &Painter, r: Rect, output: bool) {
        let Some((level, muted)) = self.volume.as_ref().map(|v| (v.level(), v.muted())) else { return };
        let silent = muted || level == 0.0;
        let glyph = if silent { "\u{E74F}" } else { "\u{E767}" };
        self.icon_button(p, Rect::new(r.x, r.y, 22.0, r.h), glyph, Font::Icon, Action::Mute, BG, if muted { FOCUS } else { MUTED });
        let ow = if output { 26.0 } else { 0.0 };
        let hit = slider(p, Rect::new(r.x + 28.0, r.y + r.h / 2.0 - 1.5, r.w - 28.0 - ow, 3.0), if muted { 0.0 } else { level });
        self.hits.push((hit, Action::Slider(Slider::Volume)));
        if output {
            self.icon_button(p, Rect::new(r.x + r.w - 22.0, r.y, 22.0, r.h), "\u{E7F5}", Font::Icon, Action::OutputNext, BG, MUTED);
        }
    }

    /// Album art, decoded once per song, or a placeholder note.
    fn draw_art(&mut self, p: &Painter, snap: &media::Snapshot, art: Rect) {
        self.load_art(p, snap);
        match &self.art {
            Some((_, bitmap, _)) => {
                p.push_clip(art, 10.0, 1.0);
                p.image(bitmap, art);
                p.pop_clip();
            }
            None => {
                p.round(art, 10.0, CHIP);
                p.text("\u{E8D6}", art, Font::IconBig, MUTED, Align::Center);
            }
        }
    }

    fn load_art(&mut self, p: &Painter, snap: &media::Snapshot) {
        if self.art.as_ref().map(|(id, ..)| *id) != Some(snap.art_id) {
            self.art = snap.art.as_ref().and_then(|bytes| p.load_image(bytes)).map(|(b, c)| (snap.art_id, b, c));
        }
    }

    /// Nusa's lyrics view: a compact player left of a divider, the lyric list on the right.
    /// The list scrolls so the sung line sits centred and lit; the rest are dimmed and a touch smaller.
    /// `k` runs 0..1 from the plain player to this layout: shared parts glide between their two spots.
    fn draw_lyrics(&mut self, p: &Painter, x: f32, k: f32) {
        let snap = self.media_snapshot();
        let Some(track) = snap.track.clone() else { return };
        self.load_art(p, &snap);
        let fade = |c: Color, a: f32| Color { a: c.a * a, ..c };
        let glow = self.art.as_ref().map_or(LYRIC_GLOW, |(.., c)| *c);
        // Clip to the open panel's shape so the glow keeps its rounded bottom corners.
        // Use the live width: mid-toggle the panel is narrower than LYRICS_W.
        let w = self.open_w();
        p.push_clip(Rect::new(x, -24.0, w, OPEN_H + 24.0), 24.0, 1.0);
        p.glow(Rect::new(x, OPEN_H * 0.18, w, OPEN_H * 0.82), fade(glow, k));
        p.pop_clip();

        // Where each part sits on the plain music page (centred as the panel resizes) and here.
        let mx = x + (w - OPEN_W) / 2.0;
        let at = |a: Rect, b: Rect| Rect::new(lerp(a.x, b.x, k), lerp(a.y, b.y, k), lerp(a.w, b.w, k), lerp(a.h, b.h, k));
        self.draw_art(p, &snap, at(Rect::new(mx + 44.0, 24.0, 52.0, 52.0), Rect::new(x + 22.0, 12.0, 62.0, 62.0)));
        // Fonts and casing differ between the layouts, so the text cross-fades as it moves.
        let title = at(Rect::new(mx + 110.0, 32.0, OPEN_W - 154.0, 20.0), Rect::new(x + 100.0, 37.0, 144.0, 20.0));
        p.text(&track.title, title, Font::Body, fade(TEXT, 1.0 - k), Align::Left);
        p.text(&track.title, title, Font::Title, fade(TEXT, k), Align::Left);
        let artist = at(Rect::new(mx + 110.0, 51.0, OPEN_W - 154.0, 16.0), Rect::new(x + 100.0, 56.0, 144.0, 14.0));
        p.text(&track.artist.to_uppercase(), artist, Font::Small, fade(MUTED, 1.0 - k), Align::Left);
        p.text(&track.artist, artist, Font::Small, fade(MUTED, k), Align::Left);
        self.draw_progress(p, lerp(mx + 44.0, x + 25.0, k), lerp(mx + OPEN_W - 44.0, x + 236.0, k), lerp(86.0, 91.0, k), &track);

        let fill = fade(BG, 1.0 - k);
        let lyric_color = if self.show_lyrics { TEXT } else { MUTED };
        let (cx, play) = (mx + OPEN_W / 2.0, if track.playing { Shape::Pause } else { Shape::Play });
        let lyric = at(Rect::new(mx + 46.0, 96.0, 28.0, 24.0), Rect::new(x + 39.0, 106.0, 28.0, 24.0));
        self.icon_button(p, lyric, "\u{E90A}", Font::Icon, Action::Lyrics, fill, lyric_color);
        let prev = at(Rect::new(cx - 64.0, 96.0, 36.0, 24.0), Rect::new(x + 73.0, 106.0, 36.0, 24.0));
        self.shape_button(p, prev, Shape::Prev, 1.0, Action::Media(media::Cmd::Prev), fill, TEXT);
        let pause = at(Rect::new(cx - 18.0, 96.0, 36.0, 24.0), Rect::new(x + 113.0, 106.0, 36.0, 24.0));
        self.shape_button(p, pause, play, 1.0, Action::Media(media::Cmd::PlayPause), fill, TEXT);
        let next = at(Rect::new(cx + 28.0, 96.0, 36.0, 24.0), Rect::new(x + 153.0, 106.0, 36.0, 24.0));
        self.shape_button(p, next, Shape::Next, 1.0, Action::Media(media::Cmd::Next), fill, TEXT);
        // No room for the output button beside the lyrics; it shows again on the plain player.
        self.draw_volume(p, at(Rect::new(cx + 72.0, 96.0, 104.0, 24.0), Rect::new(x + 192.0, 106.0, 50.0, 24.0)), false);
        p.round(Rect::new(x + 252.0, 22.0, 1.0, 102.0), 0.0, rgba(1.0, 1.0, 1.0, 0.12 * k));

        // The list waits until the player has mostly moved aside, then drifts in from the right.
        let list = ((k - 0.4) / 0.6).clamp(0.0, 1.0);
        if list > 0.0 {
            let slide = if self.reduced_motion { 0.0 } else { 28.0 * (1.0 - list).powi(2) };
            p.push_clip(Rect::new(x + 256.0, 0.0, w - 256.0, OPEN_H), 0.0, list);
            p.offset(slide);
            self.draw_lyric_list(p, x, &snap, &track, list >= 0.5);
            p.offset(0.0);
            p.pop_clip();
        }
    }

    fn draw_lyric_list(&mut self, p: &Painter, x: f32, snap: &media::Snapshot, track: &media::Track, clickable: bool) {
        let (bx, top, bh, lw) = (x + 264.0, 16.0, 120.0, 234.0);
        let lines = match &snap.lyrics {
            LyricState::Found(lines) => lines,
            state => {
                let msg = if *state == LyricState::Loading { "Loading lyrics…" } else { "Lyrics not available" };
                p.text(msg, Rect::new(bx, top, lw, bh), Font::Body, MUTED, Align::Left);
                return;
            }
        };
        let key = format!("{}\u{1}{}\u{1}{}", track.title, track.artist, lines.len());
        let fresh = self.ly_heights.0 != key;
        if fresh {
            // 4.5 DIPs of padding above and below each line, as in Nusa.
            let heights = lines.iter().map(|(_, l)| p.measure_height(l, Font::Lyric, lw) + 9.0).collect();
            self.ly_heights = (key, heights);
        }
        let heights = &self.ly_heights.1;
        let centers = lyrics::centers(heights);
        let cur = lyrics::current(lines, track.position());
        self.ly_target = centers[cur.unwrap_or(0)];
        if fresh {
            self.ly_pos = self.ly_target;
            self.ly_vel = 0.0;
        }
        for (i, (at, line)) in lines.iter().enumerate() {
            let (h, cy) = (heights[i], top + 56.0 + centers[i] - self.ly_pos);
            if cy + h / 2.0 < top || cy - h / 2.0 > top + bh {
                continue;
            }
            let fade = lyrics::edge_fade((cy - top) / bh);
            let on = if cur.is_some() { (1.0 - (centers[i] - self.ly_pos).abs() / h).clamp(0.0, 1.0) } else { 0.0 };
            let color = rgba(1.0, 1.0, 1.0, lerp(0.3, 1.0, on) * fade);
            let r = Rect::new(bx, cy - h / 2.0, lw, h);
            p.scaled(lerp(0.93, 1.0, on), bx, cy, 0.0, || {
                p.text(line, Rect::new(bx, r.y + 4.5, lw, h - 9.0), Font::Lyric, color, Align::Left)
            });
            if clickable && fade > 0.5 {
                self.hits.push((r, Action::Media(media::Cmd::Seek(*at))));
            }
        }
    }

    fn draw_focus(&mut self, p: &Painter, x: f32) {
        let left = self.pomo.remaining(Instant::now());
        let frac = left.as_secs_f32() / self.pomo.mode.length().as_secs_f32().max(1.0);
        let (x0, right) = (x + 44.0, x + OPEN_W - 44.0);
        // Breaks read green so a glance tells focus from rest.
        let (accent, deep) = if self.pomo.mode == Mode::Focus { (FOCUS, FOCUS_DEEP) } else { (TASK, CHIP) };
        let color = if self.pomo.running() { TEXT } else { MUTED };
        p.text(&mmss(left), Rect::new(x0, 26.0, 160.0, 36.0), Font::Big, color, Align::Left);

        // Session dots: finished focus sessions filled, the current one in the accent.
        let dots_x = right - 3.0 * 11.0 - 3.0;
        for i in 0..4u8 {
            let c = if i < self.pomo.cycles { TEXT } else if i == self.pomo.cycles { accent } else { DOT };
            p.ellipse(dots_x + i as f32 * 11.0, 45.0, 3.0, 3.0, c);
        }
        p.text(self.pomo.mode.label(), Rect::new(dots_x - 110.0, 37.0, 100.0, 16.0), Font::Small, MUTED, Align::Right);

        // Fills up as the session runs.
        let bar = Rect::new(x0, 71.0, right - x0, 3.0);
        p.round(bar, 1.5, deep);
        p.round(Rect { w: bar.w * (1.0 - frac), ..bar }, 1.5, accent);

        for (i, (m, label)) in [(Mode::Focus, "Focus"), (Mode::Short, "Short"), (Mode::Long, "Long")].into_iter().enumerate() {
            let (fill, fg) = if self.pomo.mode == m { (CHIP_ON, TEXT) } else { (CHIP, MUTED) };
            self.chip(p, Rect::new(x0 + i as f32 * 62.0, 90.0, 58.0, 26.0), Ico::None, label, Action::PomoMode(m), fill, fg);
        }
        self.icon_button(p, Rect::new(right - 76.0, 90.0, 26.0, 26.0), "\u{E72C}", Font::Icon, Action::PomoReset, CHIP, TEXT);
        let play = if self.pomo.running() { Shape::Pause } else { Shape::Play };
        self.shape_button(p, Rect::new(right - 44.0, 90.0, 44.0, 26.0), play, 0.75, Action::PomoToggle, accent, TEXT);
    }

    /// One dot per planned sip, filled as they're drunk; the chart button swaps the dots for today's hours.
    fn draw_water(&mut self, p: &Painter, x: f32) {
        let (x0, right) = (x + 44.0, x + OPEN_W - 44.0);
        let (taken, goal, on) = (self.water.taken, self.water.goal, self.water.on);
        let frac = self.water_shown;
        let liters = format!("{:.1}", taken as f32 / 1000.0);
        let lw = p.measure(&liters, Font::Big);
        p.text(&liters, Rect::new(x0, 24.0, lw + 2.0, 34.0), Font::Big, TEXT, Align::Left);
        p.text(&format!("of {:.1} L", goal as f32 / 1000.0), Rect::new(x0 + lw + 6.0, 34.0, 70.0, 20.0), Font::Small, MUTED, Align::Left);

        let next = if taken >= goal {
            "Goal reached".to_string()
        } else if !on {
            "Reminders off".to_string()
        } else {
            self.water.next_at(clock().now).map_or("Done for today".to_string(), |t| format!("Next {}", hhmm(t)))
        };
        let next_color = if taken >= goal { WATER } else { MUTED };
        p.text(&next, Rect::new(right - 150.0, 34.0, 116.0, 20.0), Font::Small, next_color, Align::Right);
        self.icon_button(p, Rect::new(right - 26.0, 31.0, 26.0, 26.0), "\u{E713}", Font::Icon, Action::WaterSettings(true), CHIP, TEXT);

        let lane = Rect::new(x0, 62.0, right - x0, 22.0);
        if self.water_chart {
            let max = self.water.hours.iter().copied().max().unwrap_or(0).max(1) as f32;
            let hour = clock().hour;
            let step = lane.w / 24.0;
            for (i, &ml) in self.water.hours.iter().enumerate() {
                let bx = lane.x + i as f32 * step + (step - 4.0) / 2.0;
                p.round(Rect::new(bx, lane.y, 4.0, lane.h), 2.0, CHIP);
                let bh = lane.h * ml as f32 / max;
                p.round(Rect::new(bx, lane.y + lane.h - bh, 4.0, bh), 2.0, if i == hour { TEXT } else { WATER });
            }
        } else {
            let count = self.water.schedule().count.max(1);
            let step = (lane.w / count as f32).min(16.0);
            let r = (step * 0.32).min(4.0);
            let filled = frac * count as f32;
            let cy = lane.y + lane.h / 2.0;
            for i in 0..count {
                let cx = lane.x + r + i as f32 * step;
                p.ellipse(cx, cy, r, r, CHIP);
                // The dot being drunk grows in, so a sip visibly fills it.
                let part = (filled - i as f32).clamp(0.0, 1.0);
                if part > 0.0 {
                    p.ellipse(cx, cy, r * part, r * part, WATER);
                }
            }
        }
        self.hits.push((lane, Action::WaterChart));

        let ml = self.water.schedule().ml_each;
        self.chip(p, Rect::new(x0, 92.0, 120.0, 26.0), Ico::Drop, &format!("Drink {ml} ml"), Action::Sip, WATER_DEEP, WATER);
        let switch = Rect::new(right - 70.0, 97.0, 30.0, 16.0);
        p.round(switch, 8.0, if on { WATER } else { CHIP });
        p.ellipse(if on { switch.x + 22.0 } else { switch.x + 8.0 }, switch.y + 8.0, 6.0, 6.0, TEXT);
        self.hits.push((Rect { y: switch.y - 5.0, h: switch.h + 10.0, ..switch }, Action::ToggleReminders));
        let chart_fill = if self.water_chart { CHIP_ON } else { CHIP };
        self.icon_button(p, Rect::new(right - 26.0, 92.0, 26.0, 26.0), "\u{E9D2}", Font::Icon, Action::WaterChart, chart_fill, TEXT);
    }

    fn stepper(&mut self, p: &Painter, x: f32, y: f32, value: &str, minus: Action, plus: Action) {
        self.icon_button(p, Rect::new(x, y, 22.0, 22.0), "\u{E738}", Font::Icon, minus, CHIP, TEXT);
        p.text(value, Rect::new(x + 22.0, y, 56.0, 22.0), Font::Small, TEXT, Align::Center);
        self.icon_button(p, Rect::new(x + 78.0, y, 22.0, 22.0), "\u{E710}", Font::Icon, plus, CHIP, TEXT);
    }

    fn draw_water_settings(&mut self, p: &Painter, x: f32) {
        let x0 = x + 44.0;
        let label = |text: &str, x: f32, y: f32| p.text(text, Rect::new(x, y, 40.0, 22.0), Font::Small, MUTED, Align::Left);
        label("Bottle", x0, 34.0);
        let bottle = format!("{} ml", self.water.bottle);
        self.stepper(p, x0 + 40.0, 34.0, &bottle, Action::Bottle(-50), Action::Bottle(50));
        label("Goal", x0 + 150.0, 34.0);
        for (i, g) in water::GOALS.into_iter().enumerate() {
            let on = self.water.goal == g;
            let r = Rect::new(x0 + 180.0 + i as f32 * 38.0, 34.0, 34.0, 22.0);
            let (fill, fg) = if on { (WATER_DEEP, WATER) } else { (CHIP, TEXT) };
            self.chip(p, r, Ico::None, &format!("{:.1}", g as f32 / 1000.0), Action::Goal(g), fill, fg);
        }

        label("Hours", x0, 62.0);
        let (start, end) = (hhmm(self.water.start as f32), hhmm(self.water.end as f32));
        self.stepper(p, x0 + 40.0, 62.0, &start, Action::Start(-30), Action::Start(30));
        p.text("to", Rect::new(x0 + 140.0, 62.0, 20.0, 22.0), Font::Small, MUTED, Align::Center);
        self.stepper(p, x0 + 160.0, 62.0, &end, Action::End(-30), Action::End(30));
        let left = format!("{} ml left", self.water.in_bottle);
        p.text(&left, Rect::new(x0 + 262.0, 62.0, 70.0, 22.0), Font::Small, MUTED, Align::Right);

        self.chip(p, Rect::new(x0, 92.0, 76.0, 26.0), Ico::Glyph("\u{E72C}"), "Refilled", Action::Refill, CHIP, TEXT);
        let reset = if self.reset_armed.is_some() { "Tap to confirm" } else { "Reset today" };
        self.chip(p, Rect::new(x0 + 82.0, 92.0, 100.0, 26.0), Ico::None, reset, Action::Reset, CHIP, TEXT);
        self.chip(p, Rect::new(x0 + 252.0, 92.0, 80.0, 26.0), Ico::Glyph("\u{E73E}"), "Done", Action::WaterSettings(false), WATER_DEEP, WATER);
    }

    /// Nusa's shelf: files as cards (thumbnail, name, size) that fall in when dropped.
    /// The drop zone fills the page when empty and fades in over the cards while a file hovers.
    fn draw_shelf(&mut self, p: &Painter, x: f32) {
        let zone = Rect::new(x + 44.0, 33.0, OPEN_W - 84.0, 90.0);
        let d = self.drag_shown;
        if d > 0.0 {
            p.glow(Rect::new(x, OPEN_H * 0.15, OPEN_W, OPEN_H * 0.85), Color { a: SHELF_GLOW.a * d, ..SHELF_GLOW });
        }
        let empty = self.shelf.items.is_empty();
        if !empty {
            self.draw_cards(p, zone.x);
        }
        let show = if empty { 1.0 } else { d };
        if show == 0.0 {
            return;
        }
        // Over cards the zone is a dark veil so the hint stays readable.
        let fill = if empty { rgba(1.0, 1.0, 1.0, lerp(0.03, 0.08, d)) } else { rgba(0.04, 0.055, 0.094, 0.82) };
        let border = rgba(1.0, 1.0, 1.0, lerp(0.3, 0.8, d));
        let bob = if self.drag_hover { 1.5 - 1.5 * (self.drag_since.elapsed().as_secs_f32() * std::f32::consts::PI / 0.7).cos() } else { 0.0 };
        let fade = |c: Color, a: f32| Color { a, ..c };
        p.scaled(1.0 + 0.02 * d, zone.x + zone.w / 2.0, zone.y + zone.h / 2.0, 0.0, || {
            if show < 1.0 {
                p.push_clip(Rect::new(zone.x - 2.0, zone.y - 2.0, zone.w + 4.0, zone.h + 4.0), 0.0, show);
            }
            p.round(zone, 16.0, fill);
            p.stroke_round(zone, 16.0, border, 1.4, true);
            p.text("\u{E896}", Rect::new(zone.x, 44.0 + 6.0 * d + bob, zone.w, 26.0), Font::IconBig, TEXT, Align::Center);
            p.text("Drop files here", Rect::new(zone.x, 82.0 - 5.0 * d, zone.w, 18.0), Font::Body, fade(TEXT, 1.0 - d), Align::Center);
            p.text("Release to add", Rect::new(zone.x, 82.0 + 5.0 * (1.0 - d), zone.w, 18.0), Font::Body, fade(TEXT, d), Align::Center);
            p.text("Drag out anytime · click to open", Rect::new(zone.x, 99.0, zone.w, 15.0), Font::Small, fade(MUTED, 1.0 - d), Align::Center);
            if show < 1.0 {
                p.pop_clip();
            }
        });
    }

    fn draw_cards(&mut self, p: &Painter, x0: f32) {
        let count = self.shelf.items.len();
        self.chip(p, Rect::new(x0 + 18.0, 10.0, 50.0, 18.0), Ico::None, "Clear", Action::ShelfClear, CHIP, MUTED);
        // Cards hidden either side (the wheel scrolls them); the header has no room for a counter.
        let after = count.saturating_sub(self.shelf_scroll + SHELF_COLS);
        if self.shelf_scroll > 0 {
            p.text(&format!("+{}", self.shelf_scroll), Rect::new(x0 - 40.0, 50.0, 36.0, 18.0), Font::Small, MUTED, Align::Right);
        }
        if after > 0 {
            p.text(&format!("+{after}"), Rect::new(x0 + SHELF_COLS as f32 * 68.0 - 2.0, 50.0, 36.0, 18.0), Font::Small, MUTED, Align::Left);
        }
        let now = Instant::now();
        let cards: Vec<(usize, PathBuf)> = self.shelf.items.iter().cloned().enumerate().skip(self.shelf_scroll).take(SHELF_COLS).collect();
        for (col, (i, path)) in cards.into_iter().enumerate() {
            if !self.shelf_cache.contains_key(&path) {
                // shortcut: thumbnails load on the UI thread the first time a card shows; move to a worker if big folders stall.
                let entry = (shelf::size_label(&path), p.load_thumb(&path, 96));
                self.shelf_cache.insert(path.clone(), entry);
            }
            let (size, thumb) = &self.shelf_cache[&path];
            let card = Rect::new(x0 + col as f32 * 68.0, 31.0, 62.0, 84.0);
            let t = self.shelf_added.iter().find(|(q, _)| *q == path).map_or(1.0, |(_, at)| now.saturating_duration_since(*at).as_secs_f32() / 0.75);
            let (dy, scale, alpha) = shelf::fall(t);
            let th = Rect::new(card.x + 9.0, 37.0, 44.0, 44.0);
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            p.scaled(scale, th.x + 22.0, th.y + 22.0, dy, || {
                if alpha < 1.0 {
                    p.push_clip(Rect::new(card.x - 4.0, card.y - 4.0, card.w + 8.0, card.h + 8.0), 0.0, alpha);
                }
                match thumb {
                    Some(bitmap) => {
                        p.push_clip(th, 9.0, 1.0);
                        p.image(bitmap, th);
                        p.pop_clip();
                    }
                    None => {
                        p.round(th, 9.0, CHIP);
                        let glyph = if path.is_dir() { "\u{E8B7}" } else { "\u{E8A5}" };
                        p.text(glyph, th, Font::IconBig, MUTED, Align::Center);
                    }
                }
                p.text(&name, Rect::new(card.x - 3.0, 86.0, card.w + 6.0, 14.0), Font::Small, TEXT, Align::Center);
                p.text(size, Rect::new(card.x, 100.0, card.w, 12.0), Font::Small, MUTED, Align::Center);
                if alpha < 1.0 {
                    p.pop_clip();
                }
            });
            self.hits.push((card, Action::ShelfOpen(i)));
            // The remove badge shows on the card under the cursor.
            if self.mouse.is_some_and(|(mx, my)| card.contains(mx, my)) {
                let (bx, by) = (th.x + th.w - 2.0, th.y + 2.0);
                let badge = Rect::new(bx - 9.0, by - 9.0, 18.0, 18.0);
                p.ellipse(bx, by, 8.0, 8.0, self.lit(badge, CHIP_ON));
                p.text("\u{E711}", badge, Font::Icon, TEXT, Align::Center);
                self.hits.push((badge, Action::ShelfRemove(i)));
            }
        }
    }

    /// Summary line with a progress bar and the add button, then full-width task rows.
    fn draw_todo(&mut self, p: &Painter, x: f32) {
        let (x0, right) = (x + 44.0, x + OPEN_W - 44.0);
        let total = self.todo.tasks.len();
        let done = self.todo.tasks.iter().filter(|t| t.done).count();
        let summary = if total == 0 { "Nothing to do".to_string() } else { format!("{done} of {total} done") };
        let sw = p.measure(&summary, Font::Body);
        p.text(&summary, Rect::new(x0, 26.0, sw + 2.0, 26.0), Font::Body, TEXT, Align::Left);
        let bar = Rect::new(x0 + sw + 12.0, 38.0, right - 36.0 - (x0 + sw + 12.0), 3.0);
        p.round(bar, 1.5, CHIP);
        p.round(Rect { w: bar.w * done as f32 / total.max(1) as f32, ..bar }, 1.5, TASK);
        self.icon_button(p, Rect::new(right - 26.0, 26.0, 26.0, 26.0), "\u{E710}", Font::Icon, Action::TodoAdd, CHIP, TEXT);

        let mut y = 56.0;
        let mut rows = TODO_ROWS;
        if let Some(text) = &self.editing {
            let field = Rect::new(x0, y, right - x0, 21.0);
            p.round(field, 10.5, CHIP);
            // A caret even when empty, so the box reads as something to type into.
            let (shown, color) = if text.is_empty() { ("|  Type a task, @14:30 to remind".to_string(), MUTED) } else { (format!("{text}|"), TEXT) };
            p.text(&shown, Rect::new(field.x + 12.0, field.y, field.w - 40.0, field.h), Font::Small, color, Align::Left);
            let ok = if text.trim().is_empty() { MUTED } else { TASK };
            self.icon_button(p, Rect::new(field.x + field.w - 26.0, field.y + 1.5, 24.0, 18.0), "\u{E73E}", Font::Icon, Action::TodoSave, CHIP_ON, ok);
            y += 22.0;
            rows -= 1;
        } else if total == 0 {
            p.text("Tap + to add one. Add @14:30 for a reminder.", Rect::new(x0, 72.0, right - x0, 18.0), Font::Small, MUTED, Align::Center);
            return;
        }

        let tasks: Vec<(usize, Task)> = self.todo.tasks.iter().cloned().enumerate().skip(self.todo_scroll).take(rows).collect();
        for (i, task) in tasks {
            let tap = Rect::new(x0, y, right - x0 - 24.0, 21.0);
            p.round(Rect { w: right - x0, ..tap }, 10.5, self.lit(tap, CARD));
            let (cx, cy) = (x0 + 12.0, y + 10.5);
            if task.done {
                p.ellipse(cx, cy, 6.5, 6.5, TASK);
                p.text("\u{E73E}", Rect::new(cx - 7.0, cy - 7.0, 14.0, 14.0), Font::Icon, BG, Align::Center);
            } else {
                p.arc(cx, cy, 6.0, 0.0, 360.0, MUTED, 1.2);
            }
            let time = task.at.map(|at| hhmm(at as f32)).unwrap_or_default();
            let tw = if time.is_empty() { 0.0 } else { 40.0 };
            p.text(&task.title, Rect::new(x0 + 26.0, y, tap.w - 30.0 - tw, 21.0), Font::Small, if task.done { MUTED } else { TEXT }, Align::Left);
            p.text(&time, Rect::new(tap.x + tap.w - tw, y, tw - 4.0, 21.0), Font::Small, if task.done { MUTED } else { TASK }, Align::Right);
            self.hits.push((tap, Action::TodoToggle(i)));
            let del = Rect::new(right - 24.0, y, 24.0, 21.0);
            p.text("\u{E711}", del, Font::Icon, if self.hot == Some(del) { TEXT } else { MUTED }, Align::Center);
            self.hits.push((del, Action::TodoDelete(i)));
            y += 22.0;
        }
    }

    fn draw_alert(&mut self, p: &Painter, x: f32, alert: Alert) {
        let ml = self.water.schedule().ml_each;
        let (title, body, accent, yes, no) = match alert {
            Alert::Water(Prompt::Drink) => (
                "Sip o'clock".to_string(),
                format!("Have about {ml} ml of water"),
                WATER,
                (format!("Sipped {ml} ml"), Action::Sip),
                ("Snooze 10 min".to_string(), Action::Snooze),
            ),
            Alert::Water(Prompt::Refill) => (
                "Bottle check".to_string(),
                "Your bottle is probably empty. Top it up?".to_string(),
                WATER,
                ("Refilled".to_string(), Action::Refill),
                ("Later".to_string(), Action::Snooze),
            ),
            Alert::Pomodoro(next) => (
                if next == Mode::Focus { "Break's over".to_string() } else { "Nice focus session".to_string() },
                format!("{} is next, {} min", next.label(), next.length().as_secs() / 60),
                FOCUS,
                (format!("Start {}", next.label().to_lowercase()), Action::PomoStartNext),
                ("Later".to_string(), Action::Dismiss),
            ),
            Alert::Task(title) => (
                "Task time".to_string(),
                title,
                TASK,
                ("Got it".to_string(), Action::Dismiss),
                ("Show list".to_string(), Action::Tab(Tab::Todo)),
            ),
        };
        p.text(&title, Rect::new(x, 36.0, OPEN_W, 24.0), Font::Title, TEXT, Align::Center);
        p.text(&body, Rect::new(x + 44.0, 60.0, OPEN_W - 88.0, 16.0), Font::Small, MUTED, Align::Center);
        let cx = x + OPEN_W / 2.0;
        self.chip(p, Rect::new(cx - 128.0, 88.0, 124.0, 28.0), Ico::None, &yes.0, yes.1, accent, BG);
        self.chip(p, Rect::new(cx + 4.0, 88.0, 124.0, 28.0), Ico::None, &no.0, no.1, CHIP, TEXT);
    }
}

/// A water drop: round bottom, pointed top. `s` scales the 7 DIP base size.
fn drop_icon(p: &Painter, cx: f32, cy: f32, s: f32, c: Color) {
    p.ellipse(cx, cy + 1.5 * s, 3.3 * s, 3.3 * s, c);
    p.triangle([(cx - 3.1 * s, cy + 0.6 * s), (cx + 3.1 * s, cy + 0.6 * s), (cx, cy - 4.6 * s)], c);
}

fn date_label() -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let t = unsafe { GetLocalTime() };
    format!("{} {} {} {}", DAYS[t.wDayOfWeek as usize % 7], t.wDay, MONTHS[(t.wMonth as usize + 11) % 12], t.wYear)
}

/// Filled media glyph centred on (cx, cy); `s` scales the 14 DIP base height.
fn draw_shape(p: &Painter, shape: Shape, cx: f32, cy: f32, s: f32, c: Color) {
    let h = 6.0 * s;
    match shape {
        Shape::Play => p.triangle([(cx - 0.8 * h, cy - h), (cx - 0.8 * h, cy + h), (cx + h, cy)], c),
        Shape::Pause => {
            p.round(Rect::new(cx - 0.9 * h, cy - h, 0.6 * h, 2.0 * h), 0.2 * h, c);
            p.round(Rect::new(cx + 0.3 * h, cy - h, 0.6 * h, 2.0 * h), 0.2 * h, c);
        }
        Shape::Prev | Shape::Next => {
            // Two chevron triangles, like a rewind or fast-forward key.
            let d = if matches!(shape, Shape::Next) { 1.0 } else { -1.0 };
            for off in [-0.85 * h, 0.15 * h] {
                let back = cx + d * off;
                p.triangle([(back, cy - 0.8 * h), (back, cy + 0.8 * h), (back + d * 1.1 * h, cy)], c);
            }
        }
    }
}

/// Windows "Show animations in Windows" (Settings > Accessibility > Visual effects).
fn animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut on = windows::Win32::Foundation::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}
